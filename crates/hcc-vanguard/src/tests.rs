use std::collections::HashSet;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use hcc_iedb_stele::stele_digest;
use hcc_seal::{charter_digest, SealConstants};

use std::net::SocketAddr;
use std::time::Duration;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHARTER: &[u8] = b"vanguard test charter\n";

fn sealed() -> Result<(SystemCharter<RoyalSealed>, RoyalSeal), String> {
    let key = SigningKey::from_bytes(blake3::hash(b"vanguard in-test key").as_bytes());
    let digest = charter_digest(CHARTER);
    let royal = RoyalSeal {
        operator_pubkey: key.verifying_key().to_bytes(),
        signature: key.sign(&digest).to_bytes(),
    };
    let constants = SealConstants {
        byte_len: CHARTER.len(),
        blake3: digest,
        royal: Some(royal),
    };
    let charter = SystemCharter::load(CHARTER.to_vec())
        .verify(Some(&constants))
        .map_err(|r| r.error.to_string())?;
    Ok((charter, royal))
}

fn run(json: &str) -> Result<(Result<Value, VanguardError>, Stele), Box<dyn std::error::Error>> {
    run_in(json, None)
}

fn run_in(
    json: &str,
    workspace_root: Option<&Path>,
) -> Result<(Result<Value, VanguardError>, Stele), Box<dyn std::error::Error>> {
    let (charter, royal) = sealed()?;
    let mut stele = Stele::open_in_memory()?;
    stele.ensure_genesis(&charter, &royal)?;
    let handoff = Handoff::from_json(json)?;
    let outcome = dispatch(
        &handoff,
        &mut VanguardContext {
            charter: &charter,
            royal: &royal,
            stele: &mut stele,
            workspace_root,
            vault_path: None,
            rmk: None,
        },
    );
    Ok((outcome, stele))
}

/// The live set is pinned: a job goes live only with a handler and a test in the same commit.
pub(crate) const LIVE_JOBS: [&str; 21] = [
    "scribe.stele.append",
    "seed.scaffold.emit",
    "inspect.gate.validate",
    "krypt.stratum.init",
    "krypt.stele.verify",
    "vault.store.lesson",
    "healer.diagnose.heap",
    "wasi.sandbox.lock",
    "seed.manifest.init",
    "ziggurat.daemon.boot",
    "genesis.charter.version",
    "zerotrace.key.destroy",
    "krypt.death.keyDestroy",
    "genie.prompt.seal",
    "genie.dispatch.local",
    "baton.trace.record",
    "baton.state.replay",
    "lense.pillar.status",
    "lense.daemon.health",
    "neural.retrieve.lesson",
    "neural.index.upsert",
];

#[test]
fn live_set_is_pinned_and_names_are_unique() {
    let live: Vec<&str> = VANGUARD_TABLE
        .iter()
        .filter(|j| j.status == JobStatus::Live)
        .map(|j| j.name)
        .collect();
    assert_eq!(live, LIVE_JOBS);
    let names: HashSet<&str> = VANGUARD_TABLE.iter().map(|j| j.name).collect();
    assert_eq!(names.len(), VANGUARD_TABLE.len());
    for job in VANGUARD_TABLE
        .iter()
        .filter(|j| j.status == JobStatus::Typed)
    {
        assert_eq!(job.exec, Exec::Ledger, "{}", job.name);
    }
}

#[test]
fn every_job_is_named_in_the_index() -> Result<(), std::io::Error> {
    let index =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../INDEX.md"))?;
    for job in VANGUARD_TABLE {
        assert!(
            index.contains(&format!("`{}`", job.name)),
            "INDEX.md does not name {}",
            job.name
        );
    }
    Ok(())
}

#[test]
fn scribe_appends_a_chained_stele_row() -> TestResult {
    let (outcome, stele) = run(
        r#"{"job":"scribe.stele.append","bsmc_namespace":"scribe.stele.append","terrace_level":1,"payload":{"text":"operator note"}}"#,
    )?;
    let out = outcome?;
    assert_eq!(out["stele_id"], 2);
    let genesis = stele.first()?.ok_or("no genesis")?;
    assert_eq!(out["prev_digest_hex"], digest_hex(&genesis.blake3_digest));
    assert_eq!(
        out["blake3_hex"],
        digest_hex(&stele_digest(&genesis.blake3_digest, b"operator note"))
    );
    assert_eq!(
        out["charter_blake3_hex"],
        digest_hex(&charter_digest(CHARTER))
    );
    assert_eq!(stele.verify_chain()?, 2);
    Ok(())
}

fn repo_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

#[test]
fn gate_validate_runs_in_process_gates_and_records_verdicts() -> TestResult {
    // Gates 1, 3, 4 and 5 spawn cargo; a test must not recurse into `cargo test`.
    let (outcome, stele) = run_in(
        r#"{"job":"inspect.gate.validate","bsmc_namespace":"inspect.gate.validate","terrace_level":4,"payload":{"gates":[2,6,7,8,10,11]}}"#,
        Some(repo_root()),
    )?;
    let out = outcome?;
    assert_eq!(out["total"], 6);
    let failed: Vec<u64> = out["gates"]
        .as_array()
        .ok_or("no gates")?
        .iter()
        .filter(|g| g["pass"] == false)
        .filter_map(|g| g["gate"].as_u64())
        .collect();
    // Gate 8 (Tauri CSP) must fail honestly until src-tauri exists.
    let tauri_built = repo_root().join("src-tauri/tauri.conf.json").exists();
    assert_eq!(failed, if tauri_built { vec![] } else { vec![8] });
    assert_eq!(out["all_passed"], tauri_built);
    let record = String::from_utf8(stele.last()?.ok_or("no row")?.payload)?;
    assert!(record.starts_with("HCC-GATES-v1\ngate=02 PASS\n"));
    assert!(record.ends_with(&format!("passed={}/6\n", 6 - failed.len())));
    assert_eq!(stele.verify_chain()?, 2);
    Ok(())
}

#[test]
fn gate_validate_refuses_bad_selection_or_missing_workspace() -> TestResult {
    let (outcome, stele) = run(
        r#"{"job":"inspect.gate.validate","bsmc_namespace":"inspect.gate.validate","terrace_level":4,"payload":{"gates":[2]}}"#,
    )?;
    assert!(matches!(outcome, Err(VanguardError::NoWorkspace)));
    assert_eq!(stele.len()?, 1);
    for gates in ["[]", "[12]", "[\"2\"]", "2"] {
        let (outcome, stele) = run_in(
            &format!(
                r#"{{"job":"inspect.gate.validate","bsmc_namespace":"inspect.gate.validate","terrace_level":4,"payload":{{"gates":{gates}}}}}"#
            ),
            Some(repo_root()),
        )?;
        assert!(
            matches!(outcome, Err(VanguardError::BadPayload(_))),
            "{gates}"
        );
        assert_eq!(stele.len()?, 1);
    }
    Ok(())
}

const SEED_HANDOFF: &str = r#"{"job":"seed.scaffold.emit","bsmc_namespace":"seed.scaffold.emit","terrace_level":3,"payload":{"blueprint":{"name":"seeded-probe","summary":"vanguard seed probe"}}}"#;

/// A throwaway workspace with the same `[workspace.package]` keys the generator relies on.
fn scratch_workspace(tag: &str) -> Result<std::path::PathBuf, std::io::Error> {
    let root = std::env::temp_dir().join(format!("hcc-vanguard-{tag}-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::create_dir_all(root.join("crates"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\n]\n\n[workspace.package]\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"LicenseRef-HCC-Sovereign-Perpetual\"\npublish = false\n\n[workspace.lints.rust]\nunsafe_code = \"forbid\"\n",
    )?;
    Ok(root)
}

#[test]
fn seed_emits_checks_and_records_a_stele_row() -> TestResult {
    let root = scratch_workspace("seed")?;
    let (outcome, stele) = run_in(SEED_HANDOFF, Some(&root))?;
    let out = outcome?;
    assert_eq!(out["member"], "crates/seeded-probe");
    assert_eq!(out["cargo_check"], "passed");
    assert_eq!(out["stele_id"], 2);
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))?;
    assert!(manifest.contains("\"crates/seeded-probe\","));
    let row = stele.last()?.ok_or("no row")?;
    let record = String::from_utf8(row.payload)?;
    assert!(record.starts_with("HCC-SEED-v1\nname=seeded-probe\n"));
    assert!(record.contains("file=crates/seeded-probe/src/lib.rs "));
    assert_eq!(stele.verify_chain()?, 2);
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn seed_refuses_without_workspace_or_on_repeat() -> TestResult {
    let (outcome, stele) = run(SEED_HANDOFF)?;
    assert!(matches!(outcome, Err(VanguardError::NoWorkspace)));
    assert_eq!(stele.len()?, 1);

    let root = scratch_workspace("repeat")?;
    run_in(SEED_HANDOFF, Some(&root))?.0?;
    let (again, stele) = run_in(SEED_HANDOFF, Some(&root))?;
    assert!(matches!(
        again,
        Err(VanguardError::Seed(hcc_seed::SeedError::AlreadyExists(_)))
    ));
    assert_eq!(stele.len()?, 1, "a refused seed writes no row");
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn every_typed_job_returns_unimplemented() -> TestResult {
    for job in VANGUARD_TABLE
        .iter()
        .filter(|j| j.status == JobStatus::Typed)
    {
        let (outcome, _) = run(&format!(
            r#"{{"job":"{0}","bsmc_namespace":"{0}","terrace_level":2}}"#,
            job.name
        ))?;
        assert!(
            matches!(outcome, Err(VanguardError::Unimplemented { job: j }) if j == job.name),
            "{}",
            job.name
        );
    }
    Ok(())
}

#[test]
fn handoff_is_validated_before_any_job_runs() -> TestResult {
    let cases = [
        (
            r#"{"job":"no.such.job","bsmc_namespace":"a.b","terrace_level":1}"#,
            "unknown job",
        ),
        (
            r#"{"job":"scribe.stele.append","bsmc_namespace":"nodot","terrace_level":1}"#,
            "dot",
        ),
        (
            r#"{"job":"scribe.stele.append","bsmc_namespace":"a.b","terrace_level":6}"#,
            "outside 1..=5",
        ),
        (
            r#"{"job":"scribe.stele.append","bsmc_namespace":"a.b","terrace_level":1,"payload":{}}"#,
            "bad payload",
        ),
    ];
    for (json, expect) in cases {
        let (outcome, stele) = run(json)?;
        let message = outcome.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(message.contains(expect), "{json} -> {message}");
        assert_eq!(stele.len()?, 1);
    }
    assert!(matches!(
        Handoff::from_json(r#"{"job":"x.y","bsmc_namespace":"x.y","terrace_level":1,"extra":1}"#),
        Err(VanguardError::BadHandoff(_))
    ));
    Ok(())
}

/// A vault file with genesis, its stele, the sealed charter and an RMK.
struct Vault {
    dir: std::path::PathBuf,
    charter: SystemCharter<RoyalSealed>,
    royal: RoyalSeal,
    stele: Stele,
    rmk: RootMasterKey,
}

impl Vault {
    fn new(tag: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let dir =
            std::env::temp_dir().join(format!("hcc-vanguard-vault-{tag}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        let (charter, royal) = sealed()?;
        let mut stele = Stele::open(&dir.join("eduba_vault.db"))?;
        stele.ensure_genesis(&charter, &royal)?;
        Ok(Self {
            dir,
            charter,
            royal,
            stele,
            rmk: RootMasterKey::generate()?,
        })
    }

    fn path(&self) -> std::path::PathBuf {
        self.dir.join("eduba_vault.db")
    }

    fn run(
        &mut self,
        json: &str,
        rmk: bool,
    ) -> Result<Result<Value, VanguardError>, Box<dyn std::error::Error>> {
        let handoff = Handoff::from_json(json)?;
        let path = self.path();
        Ok(dispatch(
            &handoff,
            &mut VanguardContext {
                charter: &self.charter,
                royal: &self.royal,
                stele: &mut self.stele,
                workspace_root: Some(repo_root()),
                vault_path: Some(&path),
                rmk: rmk.then_some(&self.rmk),
            },
        ))
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn handoff(job: &str, terrace: u8, payload: Value) -> String {
    json!({ "job": job, "bsmc_namespace": job, "terrace_level": terrace, "payload": payload })
        .to_string()
}

#[test]
fn stratum_init_needs_the_rmk_then_records_the_key_on_the_stele() -> TestResult {
    let mut vault = Vault::new("stratum")?;
    let init = handoff("krypt.stratum.init", 1, json!({ "label": "lessons.v1" }));
    assert!(matches!(
        vault.run(&init, false)?,
        Err(VanguardError::NoRmk)
    ));
    assert!(matches!(
        vault.run(
            &handoff("krypt.stratum.init", 1, json!({ "label": "no spaces" })),
            true
        )?,
        Err(VanguardError::Keyring(
            hcc_zerotrace::ZeroTraceError::BadLabel
        ))
    ));
    assert_eq!(vault.stele.len()?, 1, "refusals write nothing");
    let out = vault.run(&init, true)??;
    assert_eq!(out["job"], "krypt.stratum.init");
    assert_eq!(out["stele_id"], 2);
    let key_id = out["key_id"].as_str().ok_or("no key_id")?;
    assert_eq!(key_id.len(), 32);
    let row = vault.stele.row(2)?.ok_or("no row")?;
    assert_eq!(row.module_namespace, "krypt.stratum.init");
    assert_eq!(out["blake3_hex"], digest_hex(&row.blake3_digest));
    let text = String::from_utf8(row.payload)?;
    assert!(text.starts_with(&format!(
        "HCC-KRYPT-STRATUM-v1\nkey_id={key_id}\nlabel=lessons.v1\n"
    )));
    // The FEK really opens under this RMK.
    let keyring = Keyring::open(&vault.path())?;
    let fek = keyring.open_key(&vault.rmk, key_id)?;
    let blob = hcc_krypt::encrypt_file(&fek, b"probe", b"sealed")?;
    assert_eq!(
        hcc_krypt::decrypt_file(&fek, b"probe", &blob)?.as_slice(),
        b"sealed"
    );
    drop(keyring);
    Ok(())
}

#[test]
fn both_destroy_markers_shred_and_certify() -> TestResult {
    let mut vault = Vault::new("destroy")?;
    for (n, job) in ["zerotrace.key.destroy", "krypt.death.keyDestroy"]
        .into_iter()
        .enumerate()
    {
        let made = vault.run(
            &handoff("krypt.stratum.init", 1, json!({ "label": format!("k{n}") })),
            true,
        )??;
        let key_id = made["key_id"].as_str().ok_or("no key_id")?.to_owned();
        let out = vault.run(&handoff(job, 1, json!({ "key_id": key_id })), false)??;
        assert_eq!(out["key_status"], "DESTROYED");
        assert_eq!(out["envelope_blake3_hex"], made["envelope_blake3_hex"]);
        let cert_id = out["certificate_stele_id"].as_i64().ok_or("no cert id")?;
        let row = vault.stele.row(cert_id)?.ok_or("no certificate row")?;
        assert_eq!(row.module_namespace, "zerotrace.key.destroy");
        assert!(String::from_utf8(row.payload)?
            .starts_with(&format!("HCC-ZEROTRACE-v1\nkey_id={key_id}\n")));
        let again = vault.run(&handoff(job, 1, json!({ "key_id": key_id })), false)?;
        assert!(matches!(
            again,
            Err(VanguardError::Keyring(
                hcc_zerotrace::ZeroTraceError::Destroyed { .. }
            ))
        ));
        let keyring = Keyring::open(&vault.path())?;
        assert!(
            keyring.open_key(&vault.rmk, &key_id).is_err(),
            "shredded key must not open"
        );
    }
    for bad in [
        json!({}),
        json!({ "key_id": "../etc" }),
        json!({ "key_id": "ABCDEF0123456789ABCDEF0123456789" }),
    ] {
        assert!(matches!(
            vault.run(&handoff("zerotrace.key.destroy", 1, bad), false)?,
            Err(VanguardError::BadPayload(_))
        ));
    }
    assert!(matches!(
        vault.run(
            &handoff(
                "zerotrace.key.destroy",
                1,
                json!({ "key_id": "0123456789abcdef0123456789abcdef" })
            ),
            false
        )?,
        Err(VanguardError::Keyring(
            hcc_zerotrace::ZeroTraceError::NotFound(_)
        ))
    ));
    assert_eq!(vault.stele.verify_chain()?, 5);
    Ok(())
}

#[test]
fn stele_verify_recomputes_chain_and_seal() -> TestResult {
    let mut vault = Vault::new("verify")?;
    vault.run(
        &handoff("scribe.stele.append", 1, json!({ "text": "row two" })),
        false,
    )??;
    let out = vault.run(&handoff("krypt.stele.verify", 1, json!({})), false)??;
    assert_eq!(out["chain_rows_verified"], 2);
    assert_eq!(out["head_stele_id"], 2);
    let head = vault.stele.last()?.ok_or("no head")?;
    assert_eq!(out["head_blake3_hex"], digest_hex(&head.blake3_digest));
    assert_eq!(out["charter_version"], "1.0.0");
    Ok(())
}

#[test]
fn store_lesson_chains_hashes_only() -> TestResult {
    let mut vault = Vault::new("lesson")?;
    let body = "Never hold a std mutex across an await.";
    let out = vault.run(
        &handoff(
            "vault.store.lesson",
            3,
            json!({ "title": "await rule", "body": body }),
        ),
        false,
    )??;
    assert_eq!(out["lesson_id"], 1);
    assert_eq!(out["lesson_namespace"], "vault.store.lesson");
    assert_eq!(
        out["body_blake3_hex"],
        digest_hex(blake3::hash(body.as_bytes()).as_bytes())
    );
    let row = vault.stele.row(2)?.ok_or("no row")?;
    assert_eq!(row.module_namespace, "vault.store.lesson");
    assert!(
        !String::from_utf8(row.payload)?.contains(body),
        "stele keeps hashes only"
    );
    let dup = vault.run(
        &handoff(
            "vault.store.lesson",
            3,
            json!({ "title": "again", "body": body }),
        ),
        false,
    )?;
    assert!(matches!(
        dup,
        Err(VanguardError::Neural(NeuralError::Duplicate {
            lesson_id: 1
        }))
    ));
    assert!(matches!(
        vault.run(
            &handoff("vault.store.lesson", 3, json!({ "title": "no body" })),
            false
        )?,
        Err(VanguardError::BadPayload(_))
    ));
    Ok(())
}

#[test]
fn healer_diagnoses_the_committed_fixture_under_the_lock() -> TestResult {
    let mut vault = Vault::new("healer")?;
    let out = vault.run(&handoff("healer.diagnose.heap", 2, json!({})), false)??;
    let fixture = std::fs::read(repo_root().join("fixtures/healer_empty.wasm"))?;
    assert_eq!(out["fixture"], "fixtures/healer_empty.wasm");
    assert_eq!(out["wasm_byte_len"], fixture.len());
    assert_eq!(
        out["wasm_blake3_hex"],
        digest_hex(blake3::hash(&fixture).as_bytes())
    );
    assert_eq!(out["fuel_cap"], SandboxPolicy::LOCKED.fuel);
    for bad in [
        "../Cargo.toml",
        "x/../../a.wasm",
        ".hidden.wasm",
        "healer_empty.wat",
    ] {
        assert!(
            matches!(
                vault.run(
                    &handoff("healer.diagnose.heap", 2, json!({ "fixture": bad })),
                    false
                )?,
                Err(VanguardError::BadPayload(_))
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        vault.run(
            &handoff(
                "healer.diagnose.heap",
                2,
                json!({ "fixture": "missing.wasm" })
            ),
            false
        )?,
        Err(VanguardError::Io(_))
    ));
    let lock = vault.run(&handoff("wasi.sandbox.lock", 2, json!({})), false)??;
    assert_eq!(lock["fuel"], SandboxPolicy::LOCKED.fuel);
    assert_eq!(lock["memory_bytes"], SandboxPolicy::LOCKED.memory_bytes);
    assert_eq!(lock["declared_paths"], json!([]));
    assert_eq!(vault.stele.len()?, 1, "read-only jobs write no stele row");
    Ok(())
}

#[test]
fn ledger_jobs_without_a_vault_are_refused() -> TestResult {
    for json in [
        handoff("krypt.stratum.init", 1, json!({ "label": "x" })),
        handoff(
            "vault.store.lesson",
            3,
            json!({ "title": "t", "body": "b" }),
        ),
        handoff(
            "zerotrace.key.destroy",
            1,
            json!({ "key_id": "0123456789abcdef0123456789abcdef" }),
        ),
    ] {
        let (outcome, stele) = run(&json)?;
        assert!(matches!(outcome, Err(VanguardError::NoVault)), "{json}");
        assert_eq!(stele.len()?, 1);
    }
    Ok(())
}

#[test]
fn runner_jobs_refuse_the_ledger_executor_and_vice_versa() -> TestResult {
    let (outcome, _) = run(&handoff(
        "neural.retrieve.lesson",
        3,
        json!({ "model": "m", "query": "q" }),
    ))?;
    assert!(matches!(
        outcome,
        Err(VanguardError::WrongExec {
            exec: Exec::Runner,
            ..
        })
    ));
    let vault = Vault::new("wrongexec")?;
    let path = vault.path();
    let ctx = RunnerContext {
        charter: &vault.charter,
        vault_path: &path,
        runner_addr: "127.0.0.1:9".parse()?,
        timeout: Duration::from_secs(2),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let h = Handoff::from_json(&handoff("scribe.stele.append", 1, json!({ "text": "x" })))?;
    assert!(matches!(
        rt.block_on(dispatch_runner(&h, &ctx)),
        Err(VanguardError::WrongExec {
            exec: Exec::Ledger,
            ..
        })
    ));
    Ok(())
}

/// Fake runner `/api/embed`: maps each input to a fixed 3-d vector by its first byte.
async fn fake_embed() -> Result<SocketAddr, std::io::Error> {
    use axum::routing::post;
    use axum::{Json, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = Router::new().route(
        "/api/embed",
        post(|Json(req): Json<Value>| async move {
            let vectors: Vec<Value> = req["input"]
                .as_array()
                .map(|inputs| {
                    inputs
                        .iter()
                        .map(|t| match t.as_str().and_then(|t| t.bytes().next()) {
                            Some(b'A') => json!([1.0, 0.0, 0.0]),
                            Some(b'B') => json!([0.0, 1.0, 0.0]),
                            _ => json!([0.0, 0.0, 1.0]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Json(json!({ "embeddings": vectors }))
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(addr)
}

#[test]
fn neural_jobs_index_then_retrieve_through_the_runner() -> TestResult {
    let mut vault = Vault::new("neural")?;
    for (title, body) in [
        ("alpha", "A lesson about seals"),
        ("beta", "B lesson about BATON"),
    ] {
        vault.run(
            &handoff(
                "vault.store.lesson",
                3,
                json!({ "title": title, "body": body }),
            ),
            false,
        )??;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let path = vault.path();
    let down = RunnerContext {
        charter: &vault.charter,
        vault_path: &path,
        runner_addr: rt.block_on(async {
            tokio::net::TcpListener::bind("127.0.0.1:0")
                .await?
                .local_addr()
        })?,
        timeout: Duration::from_secs(2),
    };
    let upsert = Handoff::from_json(&handoff(
        "neural.index.upsert",
        3,
        json!({ "model": "nomic-embed-text" }),
    ))?;
    assert!(matches!(
        rt.block_on(dispatch_runner(&upsert, &down)),
        Err(VanguardError::Neural(NeuralError::Embed(
            hcc_neural::EmbedError::Unreachable
        )))
    ));
    let up = RunnerContext {
        runner_addr: rt.block_on(fake_embed())?,
        ..down
    };
    let out = rt.block_on(dispatch_runner(&upsert, &up))?;
    assert_eq!(out["indexed"], 2);
    assert_eq!(out["dim"], 3);
    assert_eq!(
        rt.block_on(dispatch_runner(&upsert, &up))?["indexed"],
        0,
        "nothing pending"
    );
    let retrieve = Handoff::from_json(&handoff(
        "neural.retrieve.lesson",
        3,
        json!({ "model": "nomic-embed-text", "query": "B?", "k": 1 }),
    ))?;
    let hits = rt.block_on(dispatch_runner(&retrieve, &up))?;
    assert_eq!(hits["hits"][0]["title"], "beta");
    assert_eq!(hits["hits"].as_array().map(Vec::len), Some(1));
    for bad in [
        json!({ "model": "m", "query": "q", "k": 0 }),
        json!({ "model": "bad model", "query": "q" }),
        json!({ "model": "m" }),
    ] {
        let h = Handoff::from_json(&handoff("neural.retrieve.lesson", 3, bad))?;
        assert!(matches!(
            rt.block_on(dispatch_runner(&h, &up)),
            Err(VanguardError::BadPayload(_))
        ));
    }
    Ok(())
}

#[test]
fn index_upsert_embeds_more_than_one_runner_batch() -> TestResult {
    let vault = Vault::new("batches")?;
    let path = vault.path();
    {
        let mut store = LessonStore::open(&path)?;
        for n in 0..300 {
            store.store(
                "vault.store.lesson",
                &format!("l{n}"),
                &format!("A body {n}"),
                &[1u8; 32],
            )?;
        }
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let ctx = RunnerContext {
        charter: &vault.charter,
        vault_path: &path,
        runner_addr: rt.block_on(fake_embed())?,
        timeout: Duration::from_secs(10),
    };
    let upsert = Handoff::from_json(&handoff("neural.index.upsert", 3, json!({ "model": "m" })))?;
    let out = rt.block_on(dispatch_runner(&upsert, &ctx))?;
    assert_eq!(out["indexed"], 300);
    assert!(LessonStore::open(&path)?.pending("m")?.is_empty());
    Ok(())
}

#[test]
fn manifest_init_renders_without_writing() -> TestResult {
    let root = scratch_workspace("manifest")?;
    let manifest_before = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let json = SEED_HANDOFF.replace("seed.scaffold.emit", "seed.manifest.init");
    let (outcome, stele) = run_in(&json, Some(&root))?;
    let out = outcome?;
    assert_eq!(out["member"], "crates/seeded-probe");
    assert_eq!(out["written"], false);
    assert_eq!(out["crate_dir_taken"], false);
    let files = out["files"].as_array().ok_or("no files")?;
    assert!(files
        .iter()
        .any(|f| f["path"] == "crates/seeded-probe/src/lib.rs"));
    assert!(!root.join("crates/seeded-probe").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("Cargo.toml"))?,
        manifest_before
    );
    assert_eq!(stele.len()?, 1, "no row of its own");

    // The digests match what emit then writes.
    let emitted = run_in(SEED_HANDOFF, Some(&root))?.0?;
    let emitted_files = emitted["files"].as_array().ok_or("no emitted files")?;
    for file in emitted_files {
        let planned = files
            .iter()
            .find(|f| f["path"] == file["path"])
            .ok_or("emit wrote a file the manifest did not list")?;
        assert_eq!(planned["blake3_hex"], file["blake3_hex"]);
    }
    assert_eq!(files.len(), emitted_files.len());
    let (again, _) = run_in(&json, Some(&root))?;
    assert_eq!(again?["crate_dir_taken"], true);

    let bad = handoff(
        "seed.manifest.init",
        2,
        json!({ "blueprint": { "name": "hcc-x" } }),
    );
    assert!(matches!(
        run_in(&bad, Some(&root))?.0,
        Err(VanguardError::Seed(_))
    ));
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn daemon_boot_confirms_genesis_and_recomputes_the_chain() -> TestResult {
    let (outcome, stele) = run(&handoff("ziggurat.daemon.boot", 1, json!({})))?;
    let out = outcome?;
    assert_eq!(out["genesis"], "PRESENT");
    assert_eq!(out["genesis_stele_id"], 1);
    assert_eq!(out["genesis_matches_running_charter"], true);
    assert_eq!(out["stele_rows_verified"], 1);
    assert_eq!(out["charter_byte_len"], CHARTER.len());
    assert_eq!(stele.len()?, 1, "boot on a booted vault appends nothing");

    // A vault opened under another charter is refused, not re-genesised.
    let (charter, royal) = sealed()?;
    let mut other = Stele::open_in_memory()?;
    other.append(
        hcc_iedb_stele::GENESIS_NAMESPACE,
        &hcc_iedb_stele::genesis_payload(3, &[7u8; 32]),
        &royal.operator_pubkey,
        Some(&royal.signature),
    )?;
    let h = Handoff::from_json(&handoff("ziggurat.daemon.boot", 1, json!({})))?;
    let refused = dispatch(
        &h,
        &mut VanguardContext {
            charter: &charter,
            royal: &royal,
            stele: &mut other,
            workspace_root: None,
            vault_path: None,
            rmk: None,
        },
    );
    assert!(matches!(
        refused,
        Err(VanguardError::Stele(SteleError::GenesisMismatch))
    ));
    Ok(())
}

#[test]
fn charter_version_lists_the_lineage_read_only() -> TestResult {
    let (outcome, stele) = run(&handoff("genesis.charter.version", 1, json!({})))?;
    let out = outcome?;
    let epochs = out["epochs"].as_array().ok_or("no epochs")?;
    assert_eq!(epochs.len(), 1);
    assert_eq!(epochs[0]["version"], "1.0.0");
    assert_eq!(epochs[0]["stele_id"], 1);
    assert_eq!(
        epochs[0]["charter_blake3_hex"],
        digest_hex(&charter_digest(CHARTER))
    );
    assert_eq!(out["tip_is_running_charter"], true);
    assert_eq!(stele.len()?, 1);
    Ok(())
}

/// `genie.prompt.seal` under the committed charter, sealed with the in-test key.
fn prompt_seal(
    root: Option<&Path>,
) -> Result<Result<Value, VanguardError>, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(repo_root().join("charters/ZIGGURAT_v1.0.0.txt"))?;
    let key = SigningKey::from_bytes(blake3::hash(b"vanguard in-test key").as_bytes());
    let digest = charter_digest(&bytes);
    let royal = RoyalSeal {
        operator_pubkey: key.verifying_key().to_bytes(),
        signature: key.sign(&digest).to_bytes(),
    };
    let constants = SealConstants {
        byte_len: bytes.len(),
        blake3: digest,
        royal: Some(royal),
    };
    let charter = SystemCharter::load(bytes)
        .verify(Some(&constants))
        .map_err(|r| r.error.to_string())?;
    let mut stele = Stele::open_in_memory()?;
    stele.ensure_genesis(&charter, &royal)?;
    let h = Handoff::from_json(&handoff("genie.prompt.seal", 1, json!({})))?;
    Ok(dispatch(
        &h,
        &mut VanguardContext {
            charter: &charter,
            royal: &royal,
            stele: &mut stele,
            workspace_root: root,
            vault_path: None,
            rmk: None,
        },
    ))
}

#[test]
fn prompt_seal_checks_the_committed_modelfile() -> TestResult {
    let out = prompt_seal(Some(repo_root()))??;
    assert_eq!(out["royal_seal"], "VERIFIED");
    assert_eq!(out["charter_byte_len"], 4660);
    assert_eq!(
        out["charter_blake3_hex"],
        "cd380eae2bf4ef632316f7720d0aad77b2b916f86232c394ca2b6192004e3b96"
    );
    assert_eq!(out["modelfile"]["present"], true);
    assert_eq!(out["modelfile"]["from"], "cogito:latest");
    assert_eq!(out["modelfile"]["matches_sealed_charter"], true);

    // A tampered Modelfile is reported as not matching; a missing one as absent.
    let root = scratch_workspace("promptseal")?;
    std::fs::create_dir_all(root.join("models"))?;
    let committed =
        std::fs::read_to_string(repo_root().join(hcc_xtask::modelfile::MODELFILE_PATH))?;
    assert_eq!(prompt_seal(Some(&root))??["modelfile"]["present"], false);
    std::fs::write(
        root.join(hcc_xtask::modelfile::MODELFILE_PATH),
        committed.replacen("SYSTEM", "PARAMETER temperature 0\nSYSTEM", 1),
    )?;
    let out = prompt_seal(Some(&root))??;
    assert_eq!(out["modelfile"]["present"], true);
    assert_eq!(out["modelfile"]["matches_sealed_charter"], false);
    assert!(matches!(
        prompt_seal(None)?,
        Err(VanguardError::NoWorkspace)
    ));
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn daemon_jobs_refuse_the_ledger_executor() -> TestResult {
    for job in [
        "genie.dispatch.local",
        "baton.trace.record",
        "baton.state.replay",
        "lense.pillar.status",
        "lense.daemon.health",
    ] {
        let (outcome, stele) = run(&handoff(job, 1, json!({})))?;
        assert!(
            matches!(
                outcome,
                Err(VanguardError::WrongExec {
                    exec: Exec::Daemon,
                    ..
                })
            ),
            "{job}"
        );
        assert_eq!(stele.len()?, 1);
    }
    Ok(())
}
