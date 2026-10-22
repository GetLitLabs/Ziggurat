use hcc_seal::charter_digest;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn charter() -> Digest {
    charter_digest(b"test charter\n")
}

#[test]
fn envelope_follows_the_frozen_formula() -> TestResult {
    let ns = BsmcNamespace::parse("seed.scaffold.emit")?;
    let p = *blake3::hash(b"prompt").as_bytes();
    let r = *blake3::hash(b"response").as_bytes();
    let mut joined = b"HCC-BATON-v1\0".to_vec();
    joined.extend_from_slice(&charter());
    joined.extend_from_slice(b"seed.scaffold.emit");
    joined.push(3);
    joined.extend_from_slice(&p);
    joined.extend_from_slice(&r);
    assert_eq!(
        baton_envelope(&charter(), &ns, TerraceLevel::Workforce, &p, &r),
        *blake3::hash(&joined).as_bytes()
    );
    Ok(())
}

#[test]
fn envelope_changes_with_every_input() -> TestResult {
    let ns = BsmcNamespace::parse("seed.scaffold.emit")?;
    let other_ns = BsmcNamespace::parse("seed.scaffold.emiT")?;
    let p = *blake3::hash(b"prompt").as_bytes();
    let r = *blake3::hash(b"response").as_bytes();
    let base = baton_envelope(&charter(), &ns, TerraceLevel::Workforce, &p, &r);
    assert_ne!(
        base,
        baton_envelope(
            &charter_digest(b"other\n"),
            &ns,
            TerraceLevel::Workforce,
            &p,
            &r
        )
    );
    assert_ne!(
        base,
        baton_envelope(&charter(), &other_ns, TerraceLevel::Workforce, &p, &r)
    );
    assert_ne!(
        base,
        baton_envelope(&charter(), &ns, TerraceLevel::Crucible, &p, &r)
    );
    assert_ne!(
        base,
        baton_envelope(&charter(), &ns, TerraceLevel::Workforce, &r, &p)
    );
    Ok(())
}

#[test]
fn record_chains_and_replays() -> TestResult {
    let mut ledger = BatonLedger::open_in_memory()?;
    assert_eq!(ledger.replay(&charter())?, 0);
    let a = ledger.record(
        &charter(),
        "seed.scaffold.emit",
        TerraceLevel::Workforce,
        b"p1",
        b"r1",
    )?;
    let b = ledger.record(
        &charter(),
        "inspect.gate.validate",
        TerraceLevel::Crucible,
        b"p2",
        b"r2",
    )?;
    assert_eq!(a.prev_digest, [0u8; 32]);
    assert_eq!(a.sequence_number, 1);
    assert_eq!(b.prev_digest, a.envelope);
    assert_eq!(b.sequence_number, 2);
    assert_eq!(a.input_state_hash, *blake3::hash(b"p1").as_bytes());
    assert_eq!(ledger.last()?.map(|t| t.trace_id), Some(2));
    assert_eq!(ledger.replay(&charter())?, 2);
    Ok(())
}

#[test]
fn replay_under_another_charter_diverges() -> TestResult {
    let mut ledger = BatonLedger::open_in_memory()?;
    ledger.record(
        &charter(),
        "seed.scaffold.emit",
        TerraceLevel::Workforce,
        b"p",
        b"r",
    )?;
    assert!(matches!(
        ledger.replay(&charter_digest(b"other\n")),
        Err(BatonError::ReplayMismatch { trace_id: 1 })
    ));
    Ok(())
}

#[test]
fn bad_namespace_is_refused() -> TestResult {
    let mut ledger = BatonLedger::open_in_memory()?;
    assert!(matches!(
        ledger.record(&charter(), "seed", TerraceLevel::Workforce, b"p", b"r"),
        Err(BatonError::BadNamespace)
    ));
    Ok(())
}

#[test]
fn update_delete_and_out_of_range_terrace_abort() -> TestResult {
    let mut ledger = BatonLedger::open_in_memory()?;
    ledger.record(
        &charter(),
        "seed.scaffold.emit",
        TerraceLevel::Workforce,
        b"p",
        b"r",
    )?;
    let conn = ledger.connection();
    for sql in [
        "UPDATE hcc_baton_trace_ledger SET terrace_level = 4",
        "DELETE FROM hcc_baton_trace_ledger",
    ] {
        let message = conn
            .execute(sql, [])
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(message.contains("append-only"), "{message}");
    }
    let head = ledger.last()?.ok_or("no head")?;
    let bad = conn.execute(
        "INSERT INTO hcc_baton_trace_ledger (prev_digest, module_namespace, sequence_number,
           terrace_level, input_state_hash, output_state_hash, baton_envelope_sig)
         VALUES (?1, 'seed.scaffold.emit', 2, 6, zeroblob(32), zeroblob(32), randomblob(32))",
        params![&head.envelope[..]],
    );
    assert!(bad.is_err(), "terrace 6 must be refused");
    Ok(())
}

#[test]
fn insert_that_skips_the_head_aborts() -> TestResult {
    let mut ledger = BatonLedger::open_in_memory()?;
    ledger.record(
        &charter(),
        "seed.scaffold.emit",
        TerraceLevel::Workforce,
        b"p",
        b"r",
    )?;
    let result = ledger.connection().execute(
        "INSERT INTO hcc_baton_trace_ledger (prev_digest, module_namespace, sequence_number,
           terrace_level, input_state_hash, output_state_hash, baton_envelope_sig)
         VALUES (zeroblob(32), 'seed.scaffold.emit', 2, 3, zeroblob(32), zeroblob(32), randomblob(32))",
        [],
    );
    let message = result.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(message.contains("ledger head"), "{message}");
    Ok(())
}

#[test]
fn replay_catches_tampering_when_the_seatbelt_is_cut() -> TestResult {
    let mut ledger = BatonLedger::open_in_memory()?;
    for i in 0..3u8 {
        ledger.record(
            &charter(),
            "seed.scaffold.emit",
            TerraceLevel::Workforce,
            &[i],
            b"r",
        )?;
    }
    let conn = ledger.connection();
    conn.execute_batch("DROP TRIGGER hcc_baton_trace_ledger_no_update;")?;
    conn.execute(
        "UPDATE hcc_baton_trace_ledger SET output_state_hash = zeroblob(32) WHERE trace_id = 2",
        [],
    )?;
    assert!(matches!(
        ledger.replay(&charter()),
        Err(BatonError::ReplayMismatch { trace_id: 2 })
    ));
    Ok(())
}

#[test]
fn shares_the_vault_file_with_the_stele_in_wal() -> TestResult {
    let dir = std::env::temp_dir().join(format!("hcc-baton-wal-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("eduba_vault.db");
    {
        let mut ledger = BatonLedger::open(&path)?;
        ledger.record(
            &charter(),
            "seed.scaffold.emit",
            TerraceLevel::Workforce,
            b"p",
            b"r",
        )?;
    }
    let ledger = BatonLedger::open(&path)?;
    let mode: String = ledger
        .connection()
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    assert_eq!(mode, "wal");
    assert_eq!(ledger.replay(&charter())?, 1);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn record_then_rolls_back_the_trace_when_the_follow_up_fails() -> TestResult {
    let mut ledger = BatonLedger::open_in_memory()?;
    let terrace = TerraceLevel::try_from(3u8)?;
    let failed = ledger.record_then(
        &charter(),
        "genie.dispatch.local",
        terrace,
        b"p",
        b"r",
        |_, _| Err::<(), BatonError>(BatonError::BadNamespace),
    );
    assert!(failed.is_err());
    assert!(
        ledger.last()?.is_none(),
        "a failed follow-up must not leave a trace"
    );

    let (trace, seen) = ledger.record_then(
        &charter(),
        "genie.dispatch.local",
        terrace,
        b"p",
        b"r",
        |tx, trace| {
            let n: i64 = tx.query_row("SELECT count(*) FROM hcc_baton_trace_ledger", [], |r| {
                r.get(0)
            })?;
            Ok::<_, BatonError>((n, trace.trace_id))
        },
    )?;
    assert_eq!(
        seen,
        (1, trace.trace_id),
        "the follow-up sees the uncommitted trace"
    );
    assert_eq!(trace.sequence_number, 1);
    assert_eq!(ledger.replay(&charter())?, 1);
    Ok(())
}
