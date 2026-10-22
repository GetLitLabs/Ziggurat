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
fn spec_live_but_unwired_jobs_return_err() -> TestResult {
    for job in ["seed.scaffold.emit", "inspect.gate.validate"] {
        let (outcome, stele) = run(&format!(
            r#"{{"job":"{job}","bsmc_namespace":"{job}","terrace_level":3,"payload":{{}}}}"#
        ))?;
        assert!(
            matches!(outcome, Err(VanguardError::Unwired { .. })),
            "{job}"
        );
        assert_eq!(stele.len()?, 1, "{job} must not touch the stele");
    }
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
