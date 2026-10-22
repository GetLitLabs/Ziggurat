//! Vanguard job table (M05/M09). A capability table, not processes.
//!
//! Every job name comes from `INDEX.md` §4.1 or §11. Three are live in v1.0.0:
//! `scribe.stele.append`, `seed.scaffold.emit`, `inspect.gate.validate`.
//! All three execute.
//! Every TYPED job returns `Err(Unimplemented)`. Nothing reports success it did not earn.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::Path;

use hcc_iedb_stele::{Stele, SteleError};
use hcc_seal::{digest_hex, RoyalSeal, RoyalSealed, SystemCharter};
use hcc_types::{BsmcNamespace, TerraceLevel};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// v1.0.0 live job.
    Live,
    /// Named in the constitution; returns `Err(Unimplemented)`.
    Typed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VanguardJob {
    pub name: &'static str,
    pub module: &'static str,
    pub status: JobStatus,
}

const fn live(name: &'static str, module: &'static str) -> VanguardJob {
    VanguardJob {
        name,
        module,
        status: JobStatus::Live,
    }
}

const fn typed(name: &'static str, module: &'static str) -> VanguardJob {
    VanguardJob {
        name,
        module,
        status: JobStatus::Typed,
    }
}

/// The full table. Add a row here and in `INDEX.md` in the same commit.
pub const VANGUARD_TABLE: [VanguardJob; 35] = [
    live("scribe.stele.append", "M03"),
    live("seed.scaffold.emit", "M05"),
    live("inspect.gate.validate", "M11"),
    typed("accordion.fold.expand", "M02"),
    typed("accordion.persist.sqlite", "M02"),
    typed("krypt.stratum.init", "M03"),
    typed("krypt.ke_key.derive", "M03"),
    typed("krypt.de_key.wrap", "M03"),
    typed("krypt.fe_key.encrypt", "M03"),
    typed("krypt.stele.verify", "M03"),
    typed("vault.ast.parse", "M03"),
    typed("vault.store.lesson", "M03"),
    typed("healer.diagnose.heap", "M04"),
    typed("healer.patch.apply", "M04"),
    typed("wasi.sandbox.lock", "M04"),
    typed("seed.manifest.init", "M05"),
    typed("ziggurat.daemon.boot", "M05"),
    typed("polymorph.workspace.init", "M06"),
    typed("polymorph.emit.native", "M06"),
    typed("genesis.charter.version", "M07"),
    typed("chare.provenance.tag", "M07"),
    typed("zerotrace.key.destroy", "M08"),
    typed("krypt.death.keyDestroy", "M08"),
    typed("genie.prompt.seal", "M09"),
    typed("genie.dispatch.local", "M09"),
    typed("baton.trace.record", "M10"),
    typed("baton.state.replay", "M10"),
    typed("konjure.pipeline.init", "M10"),
    typed("curie_monet.verify", "M10"),
    typed("lense.pillar.status", "M11"),
    typed("origami.fold.assemble", "M12"),
    typed("sue.cockpit.mount", "M12"),
    typed("tailor.burst.balance", "M12"),
    typed("neural.retrieve.lesson", "M01"),
    typed("neural.index.upsert", "M01"),
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

/// What a live job may touch. Requires a royal-sealed charter by type.
pub struct VanguardContext<'a> {
    pub charter: &'a SystemCharter<RoyalSealed>,
    pub royal: &'a RoyalSeal,
    pub stele: &'a mut Stele,
    /// Workspace that `seed.scaffold.emit` writes into. `None` refuses the job.
    pub workspace_root: Option<&'a Path>,
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
    Stele(SteleError),
    NoWorkspace,
    Seed(hcc_seed::SeedError),
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
            Self::Stele(err) => write!(f, "stele: {err}"),
            Self::NoWorkspace => f.write_str("this job needs a workspace root"),
            Self::Seed(err) => write!(f, "seed: {err}"),
        }
    }
}

impl std::error::Error for VanguardError {}

/// Validate a handoff and run its job.
pub fn dispatch(handoff: &Handoff, ctx: &mut VanguardContext<'_>) -> Result<Value, VanguardError> {
    let job = lookup(&handoff.job).ok_or_else(|| VanguardError::UnknownJob(handoff.job.clone()))?;
    let namespace =
        BsmcNamespace::parse(&handoff.bsmc_namespace).map_err(|_| VanguardError::BadNamespace)?;
    let terrace = TerraceLevel::try_from(handoff.terrace_level)
        .map_err(|_| VanguardError::BadTerrace(handoff.terrace_level))?;
    match (job.status, job.name) {
        (JobStatus::Typed, name) => Err(VanguardError::Unimplemented { job: name }),
        (JobStatus::Live, "scribe.stele.append") => {
            scribe_stele_append(&namespace, terrace, &handoff.payload, ctx)
        }
        (JobStatus::Live, "seed.scaffold.emit") => {
            seed_scaffold_emit(&namespace, terrace, &handoff.payload, ctx)
        }
        (JobStatus::Live, "inspect.gate.validate") => {
            inspect_gate_validate(&namespace, terrace, &handoff.payload, ctx)
        }
        (JobStatus::Live, name) => Err(VanguardError::Unwired {
            job: name,
            needs: "a handler",
        }),
    }
}

/// `scribe.stele.append`: payload `{ "text": "<utf-8>" }` becomes one stele row.
fn scribe_stele_append(
    namespace: &BsmcNamespace<'_>,
    terrace: TerraceLevel,
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
        "job": "scribe.stele.append",
        "bsmc_namespace": namespace.as_str(),
        "terrace_level": terrace.as_u8(),
        "charter_blake3_hex": ctx.charter.digest_hex(),
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
    terrace: TerraceLevel,
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
        "job": "seed.scaffold.emit",
        "bsmc_namespace": namespace.as_str(),
        "terrace_level": terrace.as_u8(),
        "charter_blake3_hex": ctx.charter.digest_hex(),
        "member": report.member,
        "blueprint_blake3_hex": digest_hex(&report.emit.blueprint_blake3),
        "files": files,
        "cargo_check": "passed",
        "stele_id": row.stele_id,
        "blake3_hex": digest_hex(&row.blake3_digest),
    }))
}

/// `inspect.gate.validate`: payload `{ "gates": [n, ...] }` (default: all eleven, INDEX §14).
/// Runs the M11 audit against the workspace and records one stele row with every verdict.
/// Failing gates are reported as failures in the result; the job itself only errors when it
/// cannot run or record.
fn inspect_gate_validate(
    namespace: &BsmcNamespace<'_>,
    terrace: TerraceLevel,
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
        "job": "inspect.gate.validate",
        "bsmc_namespace": namespace.as_str(),
        "terrace_level": terrace.as_u8(),
        "charter_blake3_hex": ctx.charter.digest_hex(),
        "passed": passed,
        "total": results.len(),
        "all_passed": passed == results.len(),
        "gates": gates,
        "stele_id": row.stele_id,
        "blake3_hex": digest_hex(&row.blake3_digest),
    }))
}

#[cfg(test)]
mod tests;
