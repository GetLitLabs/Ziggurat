use std::path::PathBuf;

use ed25519_dalek::{Signer, SigningKey};
use hcc_iedb_stele::Stele;
use hcc_krypt::{decrypt_file, encrypt_file};
use hcc_seal::{charter_digest, RoyalSeal, SealConstants, SystemCharter};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHARTER: &[u8] = b"zerotrace test charter\n";

/// A scratch vault directory, removed on drop. Callers drop every connection first (Windows).
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Result<Self, std::io::Error> {
        let dir = std::env::temp_dir().join(format!("hcc-zerotrace-{tag}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn vault(&self) -> PathBuf {
        self.0.join("eduba_vault.db")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn royal() -> Result<(SystemCharter<hcc_seal::RoyalSealed>, RoyalSeal), String> {
    let key = SigningKey::from_bytes(blake3::hash(b"zerotrace in-test key").as_bytes());
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

/// Vault with a genesis row; returns the operator pubkey.
fn genesis(path: &Path) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let (charter, royal) = royal()?;
    let mut stele = Stele::open(path)?;
    stele.ensure_genesis(&charter, &royal)?;
    Ok(royal.operator_pubkey)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Main file plus any WAL, read with every connection closed.
fn vault_bytes(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = std::fs::read(path)?;
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    if wal.exists() {
        bytes.extend(std::fs::read(wal)?);
    }
    Ok(bytes)
}

#[test]
fn live_key_round_trips_through_the_vault_file() -> TestResult {
    let scratch = Scratch::new("live")?;
    let rmk = RootMasterKey::generate()?;
    let (key_id, blob) = {
        let mut keyring = Keyring::open(&scratch.vault())?;
        let key_id = keyring.create(&rmk, "lessons.v1")?;
        assert_eq!(key_id.len(), 32);
        assert_eq!(keyring.status(&key_id)?, KeyStatus::Live);
        let fek = keyring.open_key(&rmk, &key_id)?;
        (key_id, encrypt_file(&fek, b"file-1", b"sealed lesson")?)
    };
    let keyring = Keyring::open(&scratch.vault())?;
    let fek = keyring.open_key(&rmk, &key_id)?;
    assert_eq!(
        decrypt_file(&fek, b"file-1", &blob)?.as_slice(),
        b"sealed lesson"
    );
    let other = RootMasterKey::generate()?;
    assert!(matches!(
        keyring.open_key(&other, &key_id),
        Err(ZeroTraceError::Krypt(KryptError::Unwrap))
    ));
    drop(keyring);
    Ok(())
}

#[test]
fn destroy_shreds_the_key_and_writes_the_certificate() -> TestResult {
    let scratch = Scratch::new("destroy")?;
    let pubkey = genesis(&scratch.vault())?;
    let rmk = RootMasterKey::generate()?;
    let mut keyring = Keyring::open(&scratch.vault())?;
    let key_id = keyring.create(&rmk, "lessons.v1")?;
    let fingerprint = envelope_fingerprint(&keyring.envelope(&key_id)?);

    let cert = keyring.destroy(&key_id, &pubkey)?;
    assert_eq!(cert.key_id, key_id);
    assert_eq!(cert.envelope_blake3, fingerprint);
    assert_eq!(cert.stele_id, 2);
    assert!(cert.wal_truncated);

    assert!(matches!(
        keyring.open_key(&rmk, &key_id),
        Err(ZeroTraceError::Destroyed {
            certificate_stele_id: 2,
            ..
        })
    ));
    assert!(matches!(
        keyring.status(&key_id)?,
        KeyStatus::Destroyed {
            certificate_stele_id: 2,
            ..
        }
    ));

    let stele = Stele::open(&scratch.vault())?;
    assert_eq!(stele.verify_chain()?, 2);
    let row = stele.row(2)?.ok_or("certificate row missing")?;
    assert_eq!(row.module_namespace, CERTIFICATE_NAMESPACE);
    assert_eq!(
        row.payload,
        certificate_payload(&key_id, "lessons.v1", &fingerprint)
    );
    assert_eq!(row.blake3_digest, cert.stele_blake3);
    assert_eq!(row.ed25519_pubkey, pubkey);
    let text = String::from_utf8(row.payload)?;
    assert!(text.starts_with("HCC-ZEROTRACE-v1\n"), "{text}");
    assert!(text.contains(&format!(
        "envelope_blake3_hex={}\n",
        digest_hex(&fingerprint)
    )));
    drop(stele);
    drop(keyring);
    Ok(())
}

#[test]
fn shredded_key_bytes_are_gone_from_the_file() -> TestResult {
    let scratch = Scratch::new("bytes")?;
    let pubkey = genesis(&scratch.vault())?;
    let rmk = RootMasterKey::generate()?;
    let (key_id, envelope) = {
        let mut keyring = Keyring::open(&scratch.vault())?;
        let key_id = keyring.create(&rmk, "bytes")?;
        (key_id.clone(), keyring.envelope(&key_id)?)
    };
    let secrets = [
        envelope.salt.to_vec(),
        envelope.wrapped_dek.ciphertext.clone(),
        envelope.wrapped_fek.ciphertext.clone(),
    ];
    // Control: the scan can see the envelope while the key is live.
    let before = vault_bytes(&scratch.vault())?;
    for secret in &secrets {
        assert!(
            contains(&before, secret),
            "control scan missed a live envelope field"
        );
    }
    {
        let mut keyring = Keyring::open(&scratch.vault())?;
        keyring.destroy(&key_id, &pubkey)?;
    }
    let after = vault_bytes(&scratch.vault())?;
    for secret in &secrets {
        assert!(
            !contains(&after, secret),
            "shredded envelope bytes still in the vault file"
        );
    }
    Ok(())
}

#[test]
fn second_destroy_is_refused_and_writes_nothing() -> TestResult {
    let scratch = Scratch::new("twice")?;
    let pubkey = genesis(&scratch.vault())?;
    let rmk = RootMasterKey::generate()?;
    let mut keyring = Keyring::open(&scratch.vault())?;
    let key_id = keyring.create(&rmk, "twice")?;
    keyring.destroy(&key_id, &pubkey)?;
    assert!(matches!(
        keyring.destroy(&key_id, &pubkey),
        Err(ZeroTraceError::Destroyed {
            certificate_stele_id: 2,
            ..
        })
    ));
    assert!(matches!(
        keyring.destroy("0123456789abcdef0123456789abcdef", &pubkey),
        Err(ZeroTraceError::NotFound(_))
    ));
    let stele = Stele::open(&scratch.vault())?;
    assert_eq!(stele.verify_chain()?, 2);
    drop(stele);
    drop(keyring);
    Ok(())
}

#[test]
fn no_genesis_no_shred() -> TestResult {
    let scratch = Scratch::new("nogenesis")?;
    let rmk = RootMasterKey::generate()?;
    let mut keyring = Keyring::open(&scratch.vault())?;
    let key_id = keyring.create(&rmk, "orphan")?;
    assert!(matches!(
        keyring.destroy(&key_id, &[7; 32]),
        Err(ZeroTraceError::NoGenesis)
    ));
    assert_eq!(keyring.status(&key_id)?, KeyStatus::Live);
    assert!(keyring.open_key(&rmk, &key_id).is_ok());
    let stele = Stele::open(&scratch.vault())?;
    assert!(stele.is_empty()?);
    drop(stele);
    drop(keyring);
    Ok(())
}

#[test]
fn keyring_rows_change_only_by_shredding() -> TestResult {
    let scratch = Scratch::new("triggers")?;
    let pubkey = genesis(&scratch.vault())?;
    let rmk = RootMasterKey::generate()?;
    let mut keyring = Keyring::open(&scratch.vault())?;
    let live = keyring.create(&rmk, "live")?;
    let dead = keyring.create(&rmk, "dead")?;
    keyring.destroy(&dead, &pubkey)?;
    let conn = keyring.connection();
    let refused = |sql: &str| conn.execute(sql, []).is_err();
    assert!(refused("DELETE FROM hcc_krypt_keyring"));
    assert!(refused(&format!(
        "UPDATE hcc_krypt_keyring SET label = 'x' WHERE key_id = '{live}'"
    )));
    assert!(refused(&format!(
        "UPDATE hcc_krypt_keyring SET salt = zeroblob(32), dek_nonce = zeroblob(12),
           wrapped_dek = zeroblob(48), fek_nonce = zeroblob(12), wrapped_fek = zeroblob(48),
           destroyed_at_utc = NULL, certificate_stele_id = NULL WHERE key_id = '{dead}'"
    )));
    // A shred that skips the certificate violates the row CHECK.
    assert!(refused(&format!(
        "UPDATE hcc_krypt_keyring SET salt = NULL, dek_nonce = NULL, wrapped_dek = NULL,
           fek_nonce = NULL, wrapped_fek = NULL, destroyed_at_utc = 'now' WHERE key_id = '{live}'"
    )));
    assert_eq!(keyring.status(&live)?, KeyStatus::Live);
    drop(keyring);
    Ok(())
}

#[test]
fn labels_come_from_the_allow_list() -> TestResult {
    let scratch = Scratch::new("labels")?;
    let rmk = RootMasterKey::generate()?;
    let mut keyring = Keyring::open(&scratch.vault())?;
    for bad in [
        "",
        "two words",
        "line\nkey_id=forged",
        &"a".repeat(LABEL_MAX + 1),
    ] {
        assert!(matches!(
            keyring.create(&rmk, bad),
            Err(ZeroTraceError::BadLabel)
        ));
    }
    assert!(keyring.create(&rmk, &"a".repeat(LABEL_MAX)).is_ok());
    drop(keyring);
    Ok(())
}
