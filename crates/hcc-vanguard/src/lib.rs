//! Vanguard job table (M05/M09). A capability table, not processes.
//!
//! Every job name comes from `INDEX.md` §4.1 or §11. A LIVE job runs real code against the
//! vault, the workspace or the local runner and returns what it did; every TYPED job returns
//! `Err(Unimplemented)`. Nothing reports success it did not earn.
//!
//! Where a job runs is part of its row ([`Exec`]): `Ledger` jobs run in [`dispatch`] while the
//! caller holds the stele, `Runner` jobs run in [`dispatch_runner`] (they wait on the local model
//! runner and open their own vault connections), and `Daemon` jobs need the daemon itself
//! (`hcc-daemon` serves all three at `POST /v1/ramgenie/vanguard`).
#![forbid(unsafe_code)]

use std::fmt;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use hcc_iedb_stele::{Stele, SteleError};
use hcc_krypt::RootMasterKey;
use hcc_neural::{LessonStore, NeuralError};
use hcc_seal::{digest_hex, RoyalSeal, RoyalSealed, SystemCharter};
use hcc_types::{BsmcNamespace, TerraceLevel};
use hcc_wasi_healer::{HealerError, SandboxPolicy};
use hcc_zerotrace::{KeyStatus, Keyring, ZeroTraceError};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// v1.0.0 live job.
    Live,
    /// Named in the constitution; returns `Err(Unimplemented)`.
    Typed,
}

/// Where a live job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exec {
    /// In [`dispatch`], with the stele held by the caller.
    Ledger,
    /// In [`dispatch_runner`]: waits on the local runner, holds no ledger lock.
    Runner,
    /// Only inside `hcc-daemon`, which owns the runner wire and the live status.
    Daemon,
}

impl Exec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ledger => "ledger",
            Self::Runner => "runner",
            Self::Daemon => "daemon",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VanguardJob {
    pub name: &'static str,
    pub module: &'static str,
    pub status: JobStatus,
    pub exec: Exec,
}

const fn live(name: &'static str, module: &'static str, exec: Exec) -> VanguardJob {
    VanguardJob {
        name,
        module,
        status: JobStatus::Live,
        exec,
    }
}

const fn typed(name: &'static str, module: &'static str) -> VanguardJob {
    VanguardJob {
        name,
        module,
        status: JobStatus::Typed,
        exec: Exec::Ledger,
    }
}

/// The full table. Add a row here and in `INDEX.md` in the same commit.
pub const VANGUARD_TABLE: [VanguardJob; 36] = [
    live("scribe.stele.append", "M03", Exec::Ledger),
    live("seed.scaffold.emit", "M05", Exec::Ledger),
    live("inspect.gate.validate", "M11", Exec::Ledger),
    typed("accordion.fold.expand", "M02"),
    typed("accordion.persist.sqlite", "M02"),
    live("krypt.stratum.init", "M03", Exec::Ledger),
    typed("krypt.ke_key.derive", "M03"),
    typed("krypt.de_key.wrap", "M03"),
    typed("krypt.fe_key.encrypt", "M03"),
    live("krypt.stele.verify", "M03", Exec::Ledger),
    typed("vault.ast.parse", "M03"),
    live("vault.store.lesson", "M03", Exec::Ledger),
    live("healer.diagnose.heap", "M04", Exec::Ledger),
    typed("healer.patch.apply", "M04"),
    live("wasi.sandbox.lock", "M04", Exec::Ledger),
    live("seed.manifest.init", "M05", Exec::Ledger),
    live("ziggurat.daemon.boot", "M05", Exec::Ledger),
    typed("polymorph.workspace.init", "M06"),
    typed("polymorph.emit.native", "M06"),
    live("genesis.charter.version", "M07", Exec::Ledger),
    typed("chare.provenance.tag", "M07"),
    live("zerotrace.key.destroy", "M08", Exec::Ledger),
    live("krypt.death.keyDestroy", "M08", Exec::Ledger),
    live("genie.prompt.seal", "M09", Exec::Ledger),
    live("genie.dispatch.local", "M09", Exec::Daemon),
    live("baton.trace.record", "M10", Exec::Daemon),
    live("baton.state.replay", "M10", Exec::Daemon),
    typed("konjure.pipeline.init", "M10"),
    typed("curie_monet.verify", "M10"),
    live("lense.pillar.status", "M11", Exec::Daemon),
    live("lense.daemon.health", "M11", Exec::Daemon),
    typed("origami.fold.assemble", "M12"),
    typed("sue.cockpit.mount", "M12"),
    typed("tailor.burst.balance", "M12"),
    live("neural.retrieve.lesson", "M01", Exec::Runner),
    live("neural.index.upsert", "M01", Exec::Runner),
];

pub fn lookup(name: &str) -> Option<&'static VanguardJob> {
    VANGUARD_TABLE.iter().find(|job| job.name == name)
}

/// JSON handoff: `{ "job", "bsmc_namespace", "terrace_level", "payload" }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub job: String,
    pub bsmc_namespace: String,
    pub terrace_level: u8,
    #[serde(default)]
    pub payload: Value,
}

impl Handoff {
    pub fn from_json(text: &str) -> Result<Self, VanguardError> {
        serde_json::from_str(text).map_err(|e| VanguardError::BadHandoff(e.to_string()))
    }
}

/// What a ledger job may touch. Requires a royal-sealed charter by type.
pub struct VanguardContext<'a> {
    pub charter: &'a SystemCharter<RoyalSealed>,
    pub royal: &'a RoyalSeal,
    pub stele: &'a mut Stele,
    /// Workspace that `seed.scaffold.emit`, `inspect.gate.validate` and `healer.diagnose.heap`
    /// read or write. `None` refuses those jobs.
    pub workspace_root: Option<&'a Path>,
    /// The vault file `stele` lives in; the keyring and lesson tables open their own
    /// connections to it. `None` refuses those jobs.
    pub vault_path: Option<&'a Path>,
    /// Krypt stratum I, loaded by the operator. `None` refuses `krypt.stratum.init`.
    pub rmk: Option<&'a RootMasterKey>,
}

/// What a runner job may touch. It holds no stele: it opens its own vault connections
/// only after the runner has answered.
pub struct RunnerContext<'a> {
    pub charter: &'a SystemCharter<RoyalSealed>,
    pub vault_path: &'a Path,
    /// Local runner, loopback literal only (`hcc_neural::embed` refuses anything else).
    pub runner_addr: SocketAddr,
    pub timeout: Duration,
}

#[derive(Debug)]
pub enum VanguardError {
    BadHandoff(String),
    UnknownJob(String),
    BadNamespace,
    BadTerrace(u8),
    BadPayload(&'static str),
    Unimplemented {
        job: &'static str,
    },
    Unwired {
        job: &'static str,
        needs: &'static str,
    },
    /// The job is live but runs in another executor (see [`Exec`]).
    WrongExec {
        job: &'static str,
        exec: Exec,
    },
    Stele(SteleError),
    NoWorkspace,
    NoVault,
    /// `krypt.stratum.init` needs the operator's RMK file.
    NoRmk,
    Seed(hcc_seed::SeedError),
    Keyring(ZeroTraceError),
    Neural(NeuralError),
    Healer(HealerError),
    Io(String),
}

impl fmt::Display for VanguardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadHandoff(err) => write!(f, "bad handoff JSON: {err}"),
            Self::UnknownJob(job) => write!(f, "unknown job {job}"),
            Self::BadNamespace => f.write_str("bsmc_namespace must be dot-separated"),
            Self::BadTerrace(level) => write!(f, "terrace_level {level} is outside 1..=5"),
            Self::BadPayload(why) => write!(f, "bad payload: {why}"),
            Self::Unimplemented { job } => write!(f, "{job} is TYPED: unimplemented in v1.0.0"),
            Self::Unwired { job, needs } => write!(
                f,
                "{job} is live in the spec but not wired yet: needs {needs}"
            ),
            Self::WrongExec { job, exec } => write!(
                f,
                "{job} runs in the {} executor; send it to hcc-daemon POST /v1/ramgenie/vanguard",
                exec.as_str()
            ),
            Self::Stele(err) => write!(f, "stele: {err}"),
            Self::NoWorkspace => f.write_str("this job needs a workspace root"),
            Self::NoVault => f.write_str("this job needs the vault file"),
            Self::NoRmk => f.write_str(
                "no Krypt RMK loaded: create one with `cargo run -p hcc-xtask -- krypt-rmk keys/krypt.rmk` and restart the daemon",
            ),
            Self::Seed(err) => write!(f, "seed: {err}"),
            Self::Keyring(err) => write!(f, "keyring: {err}"),
            Self::Neural(err) => write!(f, "neural: {err}"),
            Self::Healer(err) => write!(f, "healer: {err}"),
            Self::Io(err) => write!(f, "io: {err}"),
        }
    }
}

impl std::error::Error for VanguardError {}

/// A handoff checked against the table: known job, dotted namespace, terrace in 1..=5.
pub struct Checked<'h> {
    pub job: &'static VanguardJob,
    pub namespace: BsmcNamespace<'h>,
    pub terrace: TerraceLevel,
}

/// Validate a handoff without running it. TYPED jobs are refused here.
pub fn check(handoff: &Handoff) -> Result<Checked<'_>, VanguardError> {
    let job = lookup(&handoff.job).ok_or_else(|| VanguardError::UnknownJob(handoff.job.clone()))?;
    let namespace =
        BsmcNamespace::parse(&handoff.bsmc_namespace).map_err(|_| VanguardError::BadNamespace)?;
    let terrace = TerraceLevel::try_from(handoff.terrace_level)
        .map_err(|_| VanguardError::BadTerrace(handoff.terrace_level))?;
    if job.status == JobStatus::Typed {
        return Err(VanguardError::Unimplemented { job: job.name });
    }
    Ok(Checked {
        job,
        namespace,
        terrace,
    })
}

/// Validate a handoff and run its ledger job.
pub fn dispatch(handoff: &Handoff, ctx: &mut VanguardContext<'_>) -> Result<Value, VanguardError> {
    let Checked {
        job,
        namespace,
        terrace,
    } = check(handoff)?;
    if job.exec != Exec::Ledger {
        return Err(VanguardError::WrongExec {
            job: job.name,
            exec: job.exec,
        });
    }
    let payload = &handoff.payload;
    let mut out = match job.name {
        "scribe.stele.append" => scribe_stele_append(&namespace, payload, ctx),
        "seed.scaffold.emit" => seed_scaffold_emit(&namespace, payload, ctx),
        "inspect.gate.validate" => inspect_gate_validate(&namespace, payload, ctx),
        "krypt.stratum.init" => krypt_stratum_init(payload, ctx),
        "krypt.stele.verify" => krypt_stele_verify(ctx),
        "vault.store.lesson" => vault_store_lesson(&namespace, payload, ctx),
        "healer.diagnose.heap" => healer_diagnose_heap(payload, ctx),
        "wasi.sandbox.lock" => Ok(wasi_sandbox_lock()),
        "seed.manifest.init" => seed_manifest_init(payload, ctx),
        "ziggurat.daemon.boot" => ziggurat_daemon_boot(ctx),
        "genesis.charter.version" => genesis_charter_version(ctx),
        "genie.prompt.seal" => genie_prompt_seal(ctx),
        "zerotrace.key.destroy" | "krypt.death.keyDestroy" => zerotrace_key_destroy(payload, ctx),
        name => Err(VanguardError::Unwired {
            job: name,
            needs: "a ledger handler",
        }),
    }?;
    stamp(&mut out, job, &namespace, terrace, ctx.charter);
    Ok(out)
}

/// Validate a handoff and run its runner job (M01 embeddings on the local runner).
pub async fn dispatch_runner(
    handoff: &Handoff,
    ctx: &RunnerContext<'_>,
) -> Result<Value, VanguardError> {
    let Checked {
        job,
        namespace,
        terrace,
    } = check(handoff)?;
    if job.exec != Exec::Runner {
        return Err(VanguardError::WrongExec {
            job: job.name,
            exec: job.exec,
        });
    }
    let payload = &handoff.payload;
    let mut out = match job.name {
        "neural.index.upsert" => neural_index_upsert(payload, ctx).await,
        "neural.retrieve.lesson" => neural_retrieve_lesson(payload, ctx).await,
        name => Err(VanguardError::Unwired {
            job: name,
            needs: "a runner handler",
        }),
    }?;
    stamp(&mut out, job, &namespace, terrace, ctx.charter);
    Ok(out)
}

/// Every result names its job, namespace, terrace and the charter it ran under.
fn stamp(
    out: &mut Value,
    job: &VanguardJob,
    namespace: &BsmcNamespace<'_>,
    terrace: TerraceLevel,
    charter: &SystemCharter<RoyalSealed>,
) {
    if let Some(map) = out.as_object_mut() {
        map.insert("job".into(), json!(job.name));
        map.insert("bsmc_namespace".into(), json!(namespace.as_str()));
        map.insert("terrace_level".into(), json!(terrace.as_u8()));
        map.insert("charter_blake3_hex".into(), json!(charter.digest_hex()));
    }
}

fn text<'p>(payload: &'p Value, key: &str, why: &'static str) -> Result<&'p str, VanguardError> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .ok_or(VanguardError::BadPayload(why))
}

/// `scribe.stele.append`: payload `{ "text": "<utf-8>" }` becomes one stele row.
fn scribe_stele_append(
    namespace: &BsmcNamespace<'_>,
    payload: &Value,
    ctx: &mut VanguardContext<'_>,
) -> Result<Value, VanguardError> {
    let text = payload
        .get("text")
        .and_then(Value::as_str)
        .ok_or(VanguardError::BadPayload(
            "scribe.stele.append needs {\"text\": string}",
        ))?;
    let row = ctx
        .stele
        .append(
            namespace.as_str(),
            text.as_bytes(),
            &ctx.royal.operator_pubkey,
            None,
        )
        .map_err(VanguardError::Stele)?;
    Ok(json!({
        "stele_id": row.stele_id,
        "prev_digest_hex": digest_hex(&row.prev_digest),
        "blake3_hex": digest_hex(&row.blake3_digest),
    }))
}

/// `seed.scaffold.emit`: payload `{ "blueprint": {...} }` (the `hcc-seed` blueprint).
/// Emits `crates/<name>`, adds the workspace member, runs `cargo check --offline`
/// (rolled back on failure), then records one stele row naming every file digest.
fn seed_scaffold_emit(
    namespace: &BsmcNamespace<'_>,
    payload: &Value,
    ctx: &mut VanguardContext<'_>,
) -> Result<Value, VanguardError> {
    let root = ctx.workspace_root.ok_or(VanguardError::NoWorkspace)?;
    let blueprint_json = payload.get("blueprint").ok_or(VanguardError::BadPayload(
        "seed.scaffold.emit needs {\"blueprint\": object}",
    ))?;
    let blueprint =
        hcc_seed::Blueprint::from_json(&blueprint_json.to_string()).map_err(VanguardError::Seed)?;
    let report = hcc_seed::seed(&blueprint, root).map_err(VanguardError::Seed)?;
    let mut record = format!(
        "HCC-SEED-v1\nname={}\nblueprint_blake3_hex={}\n",
        blueprint.name,
        digest_hex(&report.emit.blueprint_blake3)
    );
    for (file, hash) in &report.emit.files {
        record.push_str(&format!("file={file} {}\n", digest_hex(hash)));
    }
    let row = ctx
        .stele
        .append(
            namespace.as_str(),
            record.as_bytes(),
            &ctx.royal.operator_pubkey,
            None,
        )
        .map_err(VanguardError::Stele)?;
    let files: Vec<Value> = report
        .emit
        .files
        .iter()
        .map(|(file, hash)| json!({ "path": file, "blake3_hex": digest_hex(hash) }))
        .collect();
    Ok(json!({
        "member": report.member,
        "blueprint_blake3_hex": digest_hex(&report.emit.blueprint_blake3),
        "files": files,
        "cargo_check": "passed",
        "stele_id": row.stele_id,
        "blake3_hex": digest_hex(&row.blake3_digest),
    }))
}

/// `seed.manifest.init`: payload `{ "blueprint": {...} }`. Parses and validates the blueprint
/// and renders every file `seed.scaffold.emit` would write, without writing anything.
/// Reports whether the crate directory is already taken (emit would refuse it).
fn seed_manifest_init(payload: &Value, ctx: &VanguardContext<'_>) -> Result<Value, VanguardError> {
    let root = ctx.workspace_root.ok_or(VanguardError::NoWorkspace)?;
    let blueprint_json = payload.get("blueprint").ok_or(VanguardError::BadPayload(
        "seed.manifest.init needs {\"blueprint\": object}",
    ))?;
    let blueprint =
        hcc_seed::Blueprint::from_json(&blueprint_json.to_string()).map_err(VanguardError::Seed)?;
    let files = hcc_seed::render(&blueprint).map_err(VanguardError::Seed)?;
    let digest = blueprint.digest().map_err(VanguardError::Seed)?;
    let member = blueprint.member_path();
    let files: Vec<Value> = files
        .iter()
        .map(|file| {
            json!({
                "path": format!("{member}/{}", file.path),
                "bytes": file.contents.len(),
                "blake3_hex": digest_hex(blake3::hash(file.contents.as_bytes()).as_bytes()),
            })
        })
        .collect();
    Ok(json!({
        "name": blueprint.name,
        "crate_ident": blueprint.crate_ident(),
        "member": member,
        "blueprint_blake3_hex": digest_hex(&digest),
        "tables": blueprint.tables.len(),
        "files": files,
        "crate_dir_taken": root.join(&member).exists(),
        "written": false,
    }))
}

/// `ziggurat.daemon.boot`: the boot step that opens the vault, run again on demand.
/// `Stele::ensure_genesis` re-verifies the royal seal and either confirms the genesis row was
/// written under the running charter or refuses; the whole chain is then recomputed.
fn ziggurat_daemon_boot(ctx: &mut VanguardContext<'_>) -> Result<Value, VanguardError> {
    let genesis = ctx
        .stele
        .ensure_genesis(ctx.charter, ctx.royal)
        .map_err(VanguardError::Stele)?;
    let (created, row) = match &genesis {
        hcc_iedb_stele::Genesis::Created(row) => (true, row),
        hcc_iedb_stele::Genesis::Present(row) => (false, row),
    };
    let rows = ctx.stele.verify_chain().map_err(VanguardError::Stele)?;
    Ok(json!({
        "ziggurat_version": hcc_iedb_stele::VERSION_TAG,
        "genesis": if created { "CREATED" } else { "PRESENT" },
        "genesis_stele_id": row.stele_id,
        "genesis_blake3_hex": digest_hex(&row.blake3_digest),
        "genesis_committed_at_utc": row.committed_at_utc,
        "genesis_matches_running_charter": ctx
            .stele
            .genesis_matches(ctx.charter.byte_len(), ctx.charter.digest())
            .map_err(VanguardError::Stele)?,
        "charter_byte_len": ctx.charter.byte_len(),
        "operator_pubkey_hex": digest_hex(&ctx.royal.operator_pubkey),
        "stele_rows_verified": rows,
    }))
}

/// `genesis.charter.version`: the charter lineage the vault has run under, oldest first, and
/// whether its tip is the running charter. Read-only: recording a new version stays the
/// operator's `xtask` ceremony (it needs the private key).
fn genesis_charter_version(ctx: &VanguardContext<'_>) -> Result<Value, VanguardError> {
    let lineage = ctx.stele.lineage().map_err(VanguardError::Stele)?;
    let tip_is_running = lineage
        .last()
        .is_some_and(|tip| tip.matches(ctx.charter.byte_len(), ctx.charter.digest()));
    let epochs: Vec<Value> = lineage
        .iter()
        .map(|epoch| {
            json!({
                "version": epoch.version,
                "charter_byte_len": epoch.byte_len,
                "charter_blake3_hex": digest_hex(&epoch.digest),
                "stele_id": epoch.stele_id,
            })
        })
        .collect();
    Ok(json!({
        "epochs": epochs,
        "tip_is_running_charter": tip_is_running,
    }))
}

/// `genie.prompt.seal`: the sealed system prompt every dispatch injects. Re-verifies the
/// royal seal over the running charter and checks that the committed
/// `models/ziggurat-1.0.0.Modelfile` is exactly what `xtask modelfile` renders from it
/// (with the base on its own `FROM` line). A missing Modelfile is reported, not invented.
fn genie_prompt_seal(ctx: &VanguardContext<'_>) -> Result<Value, VanguardError> {
    let root = ctx.workspace_root.ok_or(VanguardError::NoWorkspace)?;
    let charter = ctx.charter;
    hcc_seal::verify_royal_seal(ctx.royal, charter.digest())
        .map_err(|e| VanguardError::Stele(SteleError::Seal(e)))?;
    let path = root.join(hcc_xtask::modelfile::MODELFILE_PATH);
    let modelfile = match std::fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|_| VanguardError::Io("Modelfile is not UTF-8".into()))?;
            let seal = hcc_seal::SealConstants {
                byte_len: charter.byte_len(),
                blake3: *charter.digest(),
                royal: Some(*ctx.royal),
            };
            let base = hcc_xtask::modelfile::from_line(&text);
            let expected = base.map(|base| {
                hcc_xtask::modelfile::render(
                    charter.verified_prompt().as_bytes(),
                    Some(&seal),
                    base,
                )
            });
            let matches = matches!(&expected, Some(Ok(rendered)) if *rendered == text);
            json!({
                "path": hcc_xtask::modelfile::MODELFILE_PATH,
                "present": true,
                "from": base,
                "blake3_hex": digest_hex(blake3::hash(text.as_bytes()).as_bytes()),
                "matches_sealed_charter": matches,
            })
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => json!({
            "path": hcc_xtask::modelfile::MODELFILE_PATH,
            "present": false,
        }),
        Err(err) => return Err(VanguardError::Io(err.to_string())),
    };
    Ok(json!({
        "charter_byte_len": charter.byte_len(),
        "royal_seal": "VERIFIED",
        "operator_pubkey_hex": digest_hex(&ctx.royal.operator_pubkey),
        "model": hcc_xtask::modelfile::MODEL_NAME,
        "modelfile": modelfile,
    }))
}

/// `inspect.gate.validate`: payload `{ "gates": [n, ...] }` (default: all eleven, INDEX §14).
/// Runs the M11 audit against the workspace and records one stele row with every verdict.
/// Failing gates are reported as failures in the result; the job itself only errors when it
/// cannot run or record.
fn inspect_gate_validate(
    namespace: &BsmcNamespace<'_>,
    payload: &Value,
    ctx: &mut VanguardContext<'_>,
) -> Result<Value, VanguardError> {
    let root = ctx.workspace_root.ok_or(VanguardError::NoWorkspace)?;
    let selection: Vec<u8> = match payload.get("gates") {
        None => hcc_xtask::gates::ALL.to_vec(),
        Some(list) => list
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .map(|g| g.as_u64().and_then(|n| u8::try_from(n).ok()))
                    .collect::<Option<Vec<u8>>>()
            })
            .filter(|gates| {
                !gates.is_empty() && gates.iter().all(|g| hcc_xtask::gates::ALL.contains(g))
            })
            .ok_or(VanguardError::BadPayload(
                "inspect.gate.validate takes {\"gates\": [1..=11, ...]}",
            ))?,
    };
    let results = hcc_xtask::gates::run(root, &selection);
    let passed = results.iter().filter(|r| r.pass).count();
    let mut record = String::from("HCC-GATES-v1\n");
    for result in &results {
        record.push_str(&format!(
            "gate={:02} {}\n",
            result.gate,
            if result.pass { "PASS" } else { "FAIL" }
        ));
    }
    record.push_str(&format!("passed={passed}/{}\n", results.len()));
    let row = ctx
        .stele
        .append(
            namespace.as_str(),
            record.as_bytes(),
            &ctx.royal.operator_pubkey,
            None,
        )
        .map_err(VanguardError::Stele)?;
    let gates: Vec<Value> = results
        .iter()
        .map(|r| json!({ "gate": r.gate, "name": r.name, "pass": r.pass, "detail": r.detail }))
        .collect();
    Ok(json!({
        "passed": passed,
        "total": results.len(),
        "all_passed": passed == results.len(),
        "gates": gates,
        "stele_id": row.stele_id,
        "blake3_hex": digest_hex(&row.blake3_digest),
    }))
}

/// `krypt.stratum.init`: payload `{ "label": "<1..=64 of [A-Za-z0-9._-]>" }`. Fresh salt, DEK
/// and FEK wrapped down from the operator RMK into `hcc_krypt_keyring`, plus one
/// `HCC-KRYPT-STRATUM-v1` stele row, in one transaction. Returns ids and the fingerprint, never a key.
fn krypt_stratum_init(
    payload: &Value,
    ctx: &mut VanguardContext<'_>,
) -> Result<Value, VanguardError> {
    let label = text(
        payload,
        "label",
        "krypt.stratum.init needs {\"label\": string}",
    )?;
    let vault = ctx.vault_path.ok_or(VanguardError::NoVault)?;
    let rmk = ctx.rmk.ok_or(VanguardError::NoRmk)?;
    let created = Keyring::open(vault)
        .and_then(|mut keyring| keyring.create_recorded(rmk, label, &ctx.royal.operator_pubkey))
        .map_err(VanguardError::Keyring)?;
    let row = stele_row(ctx.stele, created.stele_id)?;
    Ok(json!({
        "key_id": created.key_id,
        "label": created.label,
        "envelope_blake3_hex": digest_hex(&created.envelope_blake3),
        "strata": ["RMK (operator file, not stored)", "KEK salt", "DEK wrapped under KEK", "FEK wrapped under DEK"],
        "stele_id": created.stele_id,
        "blake3_hex": digest_hex(&row.blake3_digest),
    }))
}

fn stele_row(stele: &Stele, stele_id: i64) -> Result<hcc_iedb_stele::SteleRow, VanguardError> {
    stele
        .row(stele_id)
        .map_err(VanguardError::Stele)?
        .ok_or(VanguardError::Stele(SteleError::ChainBroken { stele_id }))
}

/// `krypt.stele.verify`: recompute the whole stele chain, check that row 1 is genesis, that the
/// charter lineage tip is the running charter, and re-verify the operator's Ed25519 seal over it.
fn krypt_stele_verify(ctx: &mut VanguardContext<'_>) -> Result<Value, VanguardError> {
    let rows = ctx.stele.verify_chain().map_err(VanguardError::Stele)?;
    let lineage = ctx.stele.lineage().map_err(VanguardError::Stele)?;
    let tip = lineage
        .last()
        .ok_or(VanguardError::Stele(SteleError::ChainBroken {
            stele_id: 1,
        }))?;
    if !tip.matches(ctx.charter.byte_len(), ctx.charter.digest()) {
        return Err(VanguardError::Stele(SteleError::GenesisMismatch));
    }
    hcc_seal::verify_royal_seal(ctx.royal, ctx.charter.digest())
        .map_err(|e| VanguardError::Stele(SteleError::Seal(e)))?;
    let head = ctx.stele.last().map_err(VanguardError::Stele)?;
    let genesis = ctx.stele.first().map_err(VanguardError::Stele)?;
    Ok(json!({
        "chain_rows_verified": rows,
        "genesis_blake3_hex": genesis.map(|r| digest_hex(&r.blake3_digest)),
        "head_stele_id": head.as_ref().map(|r| r.stele_id),
        "head_blake3_hex": head.map(|r| digest_hex(&r.blake3_digest)),
        "charter_version": tip.version,
        "lineage_versions": lineage.iter().map(|e| e.version.clone()).collect::<Vec<_>>(),
        "royal_seal": "Ed25519 signature over the charter digest verified",
    }))
}

/// `vault.store.lesson`: payload `{ "title", "body" }` under the handoff's namespace. The lesson
/// and its `HCC-LESSON-v1` stele row (hashes only) commit together.
fn vault_store_lesson(
    namespace: &BsmcNamespace<'_>,
    payload: &Value,
    ctx: &mut VanguardContext<'_>,
) -> Result<Value, VanguardError> {
    let why = "vault.store.lesson needs {\"title\": string, \"body\": string}";
    let title = text(payload, "title", why)?;
    let body = text(payload, "body", why)?;
    let vault = ctx.vault_path.ok_or(VanguardError::NoVault)?;
    let lesson = LessonStore::open(vault)
        .and_then(|mut store| {
            store.store(namespace.as_str(), title, body, &ctx.royal.operator_pubkey)
        })
        .map_err(VanguardError::Neural)?;
    let row = stele_row(ctx.stele, lesson.stele_id)?;
    Ok(json!({
        "lesson_id": lesson.lesson_id,
        "lesson_namespace": lesson.module_namespace,
        "title": lesson.title,
        "body_blake3_hex": digest_hex(&lesson.body_blake3),
        "stored_at_utc": lesson.stored_at_utc,
        "stele_id": lesson.stele_id,
        "blake3_hex": digest_hex(&row.blake3_digest),
    }))
}

/// Fixture names are plain file names under `fixtures/`: no separators, no `..`.
fn fixture_ok(name: &str) -> bool {
    (6..=64).contains(&name.len())
        && name.ends_with(".wasm")
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `healer.diagnose.heap`: payload `{ "fixture": "<name>.wasm" }` (default `healer_empty.wasm`)
/// from `<workspace>/fixtures/`. Runs it under `SandboxPolicy::LOCKED` and reports every exported
/// memory and the fuel used. Read-only: the module and the host are never patched.
fn healer_diagnose_heap(
    payload: &Value,
    ctx: &mut VanguardContext<'_>,
) -> Result<Value, VanguardError> {
    let root = ctx.workspace_root.ok_or(VanguardError::NoWorkspace)?;
    let fixture = match payload.get("fixture") {
        None => "healer_empty.wasm",
        Some(v) => v
            .as_str()
            .filter(|n| fixture_ok(n))
            .ok_or(VanguardError::BadPayload(
                "healer.diagnose.heap takes {\"fixture\": \"<name>.wasm\"} from fixtures/",
            ))?,
    };
    let rel = format!("fixtures/{fixture}");
    let policy = SandboxPolicy::LOCKED;
    let meta =
        std::fs::metadata(root.join(&rel)).map_err(|e| VanguardError::Io(format!("{rel}: {e}")))?;
    if meta.len() > policy.max_module_bytes as u64 {
        return Err(VanguardError::Healer(HealerError::TooLarge {
            byte_len: usize::try_from(meta.len()).unwrap_or(usize::MAX),
            cap: policy.max_module_bytes,
        }));
    }
    let wasm =
        std::fs::read(root.join(&rel)).map_err(|e| VanguardError::Io(format!("{rel}: {e}")))?;
    let report = hcc_wasi_healer::diagnose(&wasm, &policy).map_err(VanguardError::Healer)?;
    let memories: Vec<Value> = report
        .memories
        .iter()
        .map(|m| json!({ "export": m.export, "pages": m.pages, "bytes": m.bytes, "max_pages": m.max_pages }))
        .collect();
    Ok(json!({
        "fixture": rel,
        "wasm_byte_len": report.wasm_byte_len,
        "wasm_blake3_hex": digest_hex(&report.wasm_blake3),
        "exports": report.exports,
        "memories": memories,
        "ran_start_export": report.ran_start_export,
        "fuel_cap": report.fuel_cap,
        "fuel_consumed": report.fuel_consumed,
    }))
}

/// `wasi.sandbox.lock`: the policy every healer run is held to, read from the code that enforces it.
fn wasi_sandbox_lock() -> Value {
    let policy = SandboxPolicy::LOCKED;
    json!({
        "policy": "SandboxPolicy::LOCKED",
        "fuel": policy.fuel,
        "memory_bytes": policy.memory_bytes,
        "max_module_bytes": policy.max_module_bytes,
        "instances": 1,
        "declared_paths": policy.declared_paths,
        "host_imports": "none linked; any import is refused before instantiation",
    })
}

/// Key ids are the 32 lowercase hex characters `hcc-zerotrace` assigns.
fn key_id_ok(key_id: &str) -> bool {
    key_id.len() == 32
        && key_id
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `zerotrace.key.destroy` / `krypt.death.keyDestroy` (one function, two markers): payload
/// `{ "key_id" }`. Crypto-shreds the key and chains its deletion certificate in one transaction.
fn zerotrace_key_destroy(
    payload: &Value,
    ctx: &mut VanguardContext<'_>,
) -> Result<Value, VanguardError> {
    let key_id = text(
        payload,
        "key_id",
        "this job needs {\"key_id\": \"<32 hex>\"}",
    )?;
    if !key_id_ok(key_id) {
        return Err(VanguardError::BadPayload(
            "key_id must be 32 lowercase hex characters",
        ));
    }
    let vault = ctx.vault_path.ok_or(VanguardError::NoVault)?;
    let mut keyring = Keyring::open(vault).map_err(VanguardError::Keyring)?;
    let cert = keyring
        .destroy(key_id, &ctx.royal.operator_pubkey)
        .map_err(VanguardError::Keyring)?;
    let status = match keyring.status(key_id).map_err(VanguardError::Keyring)? {
        KeyStatus::Live => "LIVE",
        KeyStatus::Destroyed { .. } => "DESTROYED",
    };
    Ok(json!({
        "key_id": cert.key_id,
        "label": cert.label,
        "key_status": status,
        "envelope_blake3_hex": digest_hex(&cert.envelope_blake3),
        "certificate_stele_id": cert.stele_id,
        "certificate_blake3_hex": digest_hex(&cert.stele_blake3),
        "wal_truncated": cert.wal_truncated,
        "scope": "this vault file only; copies taken before the certificate still hold the envelope",
    }))
}

fn model_of<'p>(payload: &'p Value, job: &'static str) -> Result<&'p str, VanguardError> {
    let why = match job {
        "neural.index.upsert" => "neural.index.upsert needs {\"model\": string}",
        _ => "neural.retrieve.lesson needs {\"model\": string, \"query\": string, \"k\"?: 1..=64}",
    };
    let model = text(payload, "model", why)?;
    if !hcc_neural::model_ok(model) {
        return Err(VanguardError::BadPayload(
            "model must be 1-128 chars of [A-Za-z0-9_.:/-]",
        ));
    }
    Ok(model)
}

/// `neural.index.upsert`: payload `{ "model" }`. Embeds every lesson with no vector for that model
/// on the local runner and stores the vectors. Nothing is written if the runner fails.
async fn neural_index_upsert(
    payload: &Value,
    ctx: &RunnerContext<'_>,
) -> Result<Value, VanguardError> {
    let model = model_of(payload, "neural.index.upsert")?;
    // No connection is held across the runner call: open, read, close; embed; reopen, write.
    let pending = LessonStore::open(ctx.vault_path)
        .and_then(|store| store.pending(model))
        .map_err(VanguardError::Neural)?;
    if pending.is_empty() {
        return Ok(json!({ "model": model, "indexed": 0, "lesson_ids": [] }));
    }
    // The runner takes at most EMBED_MAX_INPUTS texts per call: embed in bounded batches, keep
    // every vector in memory, then write them all in one transaction (any error writes none).
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(pending.len());
    for batch in pending.chunks(hcc_neural::embed::EMBED_MAX_INPUTS) {
        let texts: Vec<&str> = batch.iter().map(|l| l.body.as_str()).collect();
        let got = hcc_neural::embed(ctx.runner_addr, model, &texts, ctx.timeout)
            .await
            .map_err(|e| VanguardError::Neural(e.into()))?;
        if got.len() != batch.len() {
            return Err(VanguardError::Neural(NeuralError::EmbedCount {
                sent: batch.len(),
                got: got.len(),
            }));
        }
        vectors.extend(got);
    }
    let rows: Vec<(i64, &[f32])> = pending
        .iter()
        .zip(&vectors)
        .map(|(lesson, vector)| (lesson.lesson_id, vector.as_slice()))
        .collect();
    LessonStore::open(ctx.vault_path)
        .and_then(|mut store| store.upsert_vectors(model, &rows))
        .map_err(VanguardError::Neural)?;
    Ok(json!({
        "model": model,
        "indexed": pending.len(),
        "dim": vectors.first().map(Vec::len),
        "lesson_ids": pending.iter().map(|l| l.lesson_id).collect::<Vec<_>>(),
    }))
}

/// `neural.retrieve.lesson`: payload `{ "model", "query", "k"? }` (k defaults to 5, at most 64).
/// Embeds the query on the local runner, then exact brute-force cosine over stored vectors.
async fn neural_retrieve_lesson(
    payload: &Value,
    ctx: &RunnerContext<'_>,
) -> Result<Value, VanguardError> {
    let model = model_of(payload, "neural.retrieve.lesson")?;
    let query = text(
        payload,
        "query",
        "neural.retrieve.lesson needs {\"model\": string, \"query\": string, \"k\"?: 1..=64}",
    )?;
    if query.is_empty() || query.len() > hcc_neural::BODY_MAX_BYTES {
        return Err(VanguardError::BadPayload("query must be 1..=65536 bytes"));
    }
    let k = match payload.get("k") {
        None => 5,
        Some(v) => v
            .as_u64()
            .and_then(|k| usize::try_from(k).ok())
            .filter(|k| (1..=hcc_neural::TOP_K_MAX).contains(k))
            .ok_or(VanguardError::BadPayload("k must be an integer in 1..=64"))?,
    };
    let mut vectors = hcc_neural::embed(ctx.runner_addr, model, &[query], ctx.timeout)
        .await
        .map_err(|e| VanguardError::Neural(e.into()))?;
    let vector = match (vectors.pop(), vectors.is_empty()) {
        (Some(v), true) => v,
        (got, _) => {
            return Err(VanguardError::Neural(NeuralError::EmbedCount {
                sent: 1,
                got: vectors.len() + usize::from(got.is_some()),
            }))
        }
    };
    let hits = LessonStore::open(ctx.vault_path)
        .and_then(|store| store.nearest(model, &vector, k))
        .map_err(VanguardError::Neural)?;
    let hits: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({
                "lesson_id": h.lesson.lesson_id,
                "score": h.score,
                "lesson_namespace": h.lesson.module_namespace,
                "title": h.lesson.title,
                "body": h.lesson.body,
                "body_blake3_hex": digest_hex(&h.lesson.body_blake3),
                "stele_id": h.lesson.stele_id,
            })
        })
        .collect();
    Ok(json!({
        "model": model,
        "k": k,
        "query_blake3_hex": digest_hex(blake3::hash(query.as_bytes()).as_bytes()),
        "method": "exact brute-force cosine over stored vectors",
        "hits": hits,
    }))
}

#[cfg(test)]
mod tests;
