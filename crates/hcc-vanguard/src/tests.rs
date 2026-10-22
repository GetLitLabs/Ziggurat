use std::collections::HashSet;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use hcc_iedb_stele::stele_digest;
use hcc_seal::{charter_digest, SealConstants};

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
        },
    );
    Ok((outcome, stele))
}

#[test]
fn exactly_three_live_jobs_and_no_duplicates() {
    let live: Vec<&str> = VANGUARD_TABLE
        .iter()
        .filter(|j| j.status == JobStatus::Live)
        .map(|j| j.name)
        .collect();
    assert_eq!(
        live,
        [
            "scribe.stele.append",
            "seed.scaffold.emit",
            "inspect.gate.validate"
        ]
    );
    let names: HashSet<&str> = VANGUARD_TABLE.iter().map(|j| j.name).collect();
    assert_eq!(names.len(), VANGUARD_TABLE.len());
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

#[test]
fn gate_validate_is_still_unwired() -> TestResult {
    let (outcome, stele) = run(
        r#"{"job":"inspect.gate.validate","bsmc_namespace":"inspect.gate.validate","terrace_level":3,"payload":{}}"#,
    )?;
    assert!(matches!(outcome, Err(VanguardError::Unwired { .. })));
    assert_eq!(stele.len()?, 1, "must not touch the stele");
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
