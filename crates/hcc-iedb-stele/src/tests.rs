use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use hcc_seal::{charter_digest, SealConstants};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sealed(bytes: &[u8], seed: &[u8]) -> Result<(SystemCharter<RoyalSealed>, RoyalSeal), String> {
    let key = SigningKey::from_bytes(blake3::hash(seed).as_bytes());
    let digest = charter_digest(bytes);
    let royal = RoyalSeal {
        operator_pubkey: key.verifying_key().to_bytes(),
        signature: key.sign(&digest).to_bytes(),
    };
    let constants = SealConstants {
        byte_len: bytes.len(),
        blake3: digest,
        royal: Some(royal),
    };
    let charter = SystemCharter::load(bytes.to_vec())
        .verify(Some(&constants))
        .map_err(|r| r.error.to_string())?;
    Ok((charter, royal))
}

const CHARTER_A: &[u8] = b"charter A\n";
const CHARTER_B: &[u8] = b"charter B\n";

#[test]
fn empty_stele_verifies() -> TestResult {
    let stele = Stele::open_in_memory()?;
    assert_eq!(stele.verify_chain()?, 0);
    assert!(stele.is_empty()?);
    assert!(stele.first()?.is_none());
    Ok(())
}

#[test]
fn genesis_embeds_charter_and_starts_from_zero() -> TestResult {
    let mut stele = Stele::open_in_memory()?;
    let (charter, royal) = sealed(CHARTER_A, b"operator")?;
    let genesis = stele.ensure_genesis(&charter, &royal)?;
    assert!(matches!(genesis, Genesis::Created(_)));
    let row = genesis.row();
    assert_eq!(row.stele_id, 1);
    assert_eq!(row.prev_digest, [0u8; 32]);
    assert_eq!(row.module_namespace, GENESIS_NAMESPACE);
    let text = String::from_utf8(row.payload.clone())?;
    assert!(text.contains(&format!("charter_byte_len={}\n", CHARTER_A.len())));
    assert!(text.contains(&format!(
        "charter_blake3_hex={}\n",
        digest_hex(&charter_digest(CHARTER_A))
    )));
    let mut joined = STELE_DOMAIN.to_vec();
    joined.extend_from_slice(&[0u8; 32]);
    joined.extend_from_slice(&row.payload);
    assert_eq!(row.blake3_digest, *blake3::hash(&joined).as_bytes());
    assert_eq!(row.ed25519_pubkey, royal.operator_pubkey);
    assert_eq!(row.royal_seal_sig, Some(royal.signature));
    assert!(stele.genesis_matches(CHARTER_A.len(), &charter_digest(CHARTER_A))?);
    Ok(())
}

#[test]
fn genesis_digest_does_not_depend_on_the_key() -> TestResult {
    let (a1, r1) = sealed(CHARTER_A, b"operator one")?;
    let (a2, r2) = sealed(CHARTER_A, b"operator two")?;
    let mut s1 = Stele::open_in_memory()?;
    let mut s2 = Stele::open_in_memory()?;
    let g1 = s1.ensure_genesis(&a1, &r1)?;
    let g2 = s2.ensure_genesis(&a2, &r2)?;
    assert_eq!(g1.row().blake3_digest, g2.row().blake3_digest);
    Ok(())
}

#[test]
fn second_boot_finds_genesis_present() -> TestResult {
    let mut stele = Stele::open_in_memory()?;
    let (charter, royal) = sealed(CHARTER_A, b"operator")?;
    stele.ensure_genesis(&charter, &royal)?;
    assert!(matches!(
        stele.ensure_genesis(&charter, &royal)?,
        Genesis::Present(_)
    ));
    assert_eq!(stele.len()?, 1);
    Ok(())
}

#[test]
fn genesis_under_another_charter_is_refused() -> TestResult {
    let mut stele = Stele::open_in_memory()?;
    let (a, ra) = sealed(CHARTER_A, b"operator")?;
    let (b, rb) = sealed(CHARTER_B, b"operator")?;
    stele.ensure_genesis(&a, &ra)?;
    assert!(matches!(
        stele.ensure_genesis(&b, &rb),
        Err(SteleError::GenesisMismatch)
    ));
    assert!(!stele.genesis_matches(CHARTER_B.len(), &charter_digest(CHARTER_B))?);
    assert_eq!(stele.len()?, 1);
    Ok(())
}

#[test]
fn genesis_refuses_a_royal_seal_for_another_charter() -> TestResult {
    let mut stele = Stele::open_in_memory()?;
    let (a, _) = sealed(CHARTER_A, b"operator")?;
    let (_, rb) = sealed(CHARTER_B, b"operator")?;
    assert!(matches!(
        stele.ensure_genesis(&a, &rb),
        Err(SteleError::Seal(_))
    ));
    assert!(stele.is_empty()?);
    Ok(())
}

#[test]
fn appends_chain_onto_the_head() -> TestResult {
    let mut stele = Stele::open_in_memory()?;
    let (charter, royal) = sealed(CHARTER_A, b"operator")?;
    let genesis = stele.ensure_genesis(&charter, &royal)?.row().clone();
    let second = stele.append(
        "scribe.stele.append",
        b"row two",
        &royal.operator_pubkey,
        None,
    )?;
    let third = stele.append(
        "scribe.stele.append",
        b"row three",
        &royal.operator_pubkey,
        None,
    )?;
    assert_eq!(second.prev_digest, genesis.blake3_digest);
    assert_eq!(third.prev_digest, second.blake3_digest);
    assert_eq!(
        third.blake3_digest,
        stele_digest(&second.blake3_digest, b"row three")
    );
    assert_eq!(stele.last()?.map(|r| r.stele_id), Some(3));
    assert_eq!(stele.verify_chain()?, 3);
    Ok(())
}

#[test]
fn namespace_without_dot_is_refused() -> TestResult {
    let mut stele = Stele::open_in_memory()?;
    assert!(matches!(
        stele.append("scribe", b"x", &[7u8; 32], None),
        Err(SteleError::BadNamespace)
    ));
    Ok(())
}

#[test]
fn update_and_delete_abort() -> TestResult {
    let mut stele = Stele::open_in_memory()?;
    stele.append("scribe.stele.append", b"row", &[7u8; 32], None)?;
    let conn = stele.connection();
    let update = conn.execute("UPDATE hcc_precedent_stele SET payload = X'00'", []);
    let delete = conn.execute("DELETE FROM hcc_precedent_stele", []);
    for result in [update, delete] {
        let message = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(message.contains("append-only"), "{message}");
    }
    assert_eq!(stele.verify_chain()?, 1);
    Ok(())
}

#[test]
fn insert_that_skips_the_head_aborts() -> TestResult {
    let mut stele = Stele::open_in_memory()?;
    stele.append("scribe.stele.append", b"row", &[7u8; 32], None)?;
    let forged_prev = [9u8; 32];
    let result = stele.connection().execute(
        "INSERT INTO hcc_precedent_stele (prev_digest, module_namespace, version_tag, domain_separator,
           payload, payload_byte_len, blake3_digest, ed25519_pubkey)
         VALUES (?1, 'scribe.stele.append', '1.0.0', ?2, X'01', 1, ?3, ?4)",
        params![&forged_prev[..], &STELE_DOMAIN[..], &stele_digest(&forged_prev, &[1])[..], &[7u8; 32][..]],
    );
    let message = result.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(message.contains("chain head"), "{message}");
    Ok(())
}

#[test]
fn chain_catches_tampering_when_the_seatbelt_is_cut() -> TestResult {
    let mut stele = Stele::open_in_memory()?;
    for payload in [&b"one"[..], b"two", b"three"] {
        stele.append("scribe.stele.append", payload, &[7u8; 32], None)?;
    }
    let conn = stele.connection();
    conn.execute_batch("DROP TRIGGER hcc_precedent_stele_no_update;")?;
    conn.execute(
        "UPDATE hcc_precedent_stele SET payload = X'54574F', payload_byte_len = 3 WHERE stele_id = 2",
        [],
    )?;
    assert!(matches!(
        stele.verify_chain(),
        Err(SteleError::ChainBroken { stele_id: 2 })
    ));
    Ok(())
}

#[test]
fn file_stele_uses_wal_and_persists() -> TestResult {
    let dir = std::env::temp_dir().join(format!("hcc-stele-wal-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("eduba_vault.db");
    let (charter, royal) = sealed(CHARTER_A, b"operator")?;
    {
        let mut stele = Stele::open(&path)?;
        assert_eq!(stele.journal_mode()?, "wal");
        stele.ensure_genesis(&charter, &royal)?;
    }
    {
        let mut stele = Stele::open(&path)?;
        assert_eq!(stele.journal_mode()?, "wal");
        assert!(matches!(
            stele.ensure_genesis(&charter, &royal)?,
            Genesis::Present(_)
        ));
        assert_eq!(stele.verify_chain()?, 1);
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn genesis_for_the_frozen_charter() -> TestResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../charters/ZIGGURAT_v1.0.0.txt");
    let bytes = std::fs::read(path)?;
    let (charter, royal) = sealed(&bytes, b"throwaway in-test key")?;
    let mut stele = Stele::open_in_memory()?;
    let row = stele.ensure_genesis(&charter, &royal)?.row().clone();
    let text = String::from_utf8(row.payload.clone())?;
    assert_eq!(
        text,
        format!(
            "HCC-GENESIS-v1\nziggurat_version=1.0.0\ncharter_byte_len={}\ncharter_blake3_hex={}\n",
            bytes.len(),
            digest_hex(&charter_digest(&bytes))
        )
    );
    println!(
        "genesis payload:\n{text}genesis blake3_digest {}",
        digest_hex(&row.blake3_digest)
    );
    Ok(())
}
