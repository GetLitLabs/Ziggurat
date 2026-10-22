use super::*;

type TestResult = Result<(), KryptError>;

const AAD: &[u8] = b"file:capsnap/schema.sql";
const PLAIN: &[u8] = b"CREATE TABLE capsnap (id INTEGER PRIMARY KEY);\n";

#[test]
fn four_strata_round_trip() -> TestResult {
    let rmk = RootMasterKey::generate()?;
    let envelope = init_stratum(&rmk)?;
    let blob = encrypt_file(&open_stratum(&rmk, &envelope)?, AAD, PLAIN)?;
    assert_ne!(blob.ciphertext.as_slice(), PLAIN);
    let fek = open_stratum(&rmk, &envelope)?;
    assert_eq!(decrypt_file(&fek, AAD, &blob)?.as_slice(), PLAIN);
    Ok(())
}

#[test]
fn kek_is_blake3_derive_key_over_rmk_and_salt() {
    let rmk = RootMasterKey::from_bytes([1u8; KEY_LEN]);
    let salt = [2u8; SALT_LEN];
    let mut material = [1u8; KEY_LEN + SALT_LEN].to_vec();
    material[KEY_LEN..].fill(2);
    assert_eq!(
        derive_kek(&rmk, &salt).expose(),
        &blake3::derive_key("HCC-KRYPT-STRATUM-v2", &material)
    );
    assert_ne!(
        derive_kek(&rmk, &salt).expose(),
        derive_kek(&rmk, &[3u8; SALT_LEN]).expose()
    );
}

#[test]
fn wrong_rmk_cannot_open_the_stratum() -> TestResult {
    let envelope = init_stratum(&RootMasterKey::generate()?)?;
    let other = RootMasterKey::generate()?;
    assert_eq!(
        open_stratum(&other, &envelope).err(),
        Some(KryptError::Unwrap)
    );
    Ok(())
}

#[test]
fn tampered_wrapped_keys_and_blobs_fail() -> TestResult {
    let rmk = RootMasterKey::generate()?;
    let envelope = init_stratum(&rmk)?;
    let mut bad_dek = envelope.clone();
    bad_dek.wrapped_dek.ciphertext[0] ^= 1;
    assert_eq!(open_stratum(&rmk, &bad_dek).err(), Some(KryptError::Unwrap));
    let mut bad_fek = envelope.clone();
    bad_fek.wrapped_fek.nonce[0] ^= 1;
    assert_eq!(open_stratum(&rmk, &bad_fek).err(), Some(KryptError::Unwrap));
    let mut bad_salt = envelope.clone();
    bad_salt.salt[0] ^= 1;
    assert_eq!(
        open_stratum(&rmk, &bad_salt).err(),
        Some(KryptError::Unwrap)
    );

    let fek = open_stratum(&rmk, &envelope)?;
    let mut blob = encrypt_file(&fek, AAD, PLAIN)?;
    assert_eq!(
        decrypt_file(&fek, b"file:other", &blob).err(),
        Some(KryptError::Unwrap)
    );
    blob.ciphertext[0] ^= 1;
    assert_eq!(
        decrypt_file(&fek, AAD, &blob).err(),
        Some(KryptError::Unwrap)
    );
    Ok(())
}

#[test]
fn wrapped_dek_cannot_pose_as_wrapped_fek() -> TestResult {
    let rmk = RootMasterKey::generate()?;
    let envelope = init_stratum(&rmk)?;
    let kek = derive_kek(&rmk, &envelope.salt);
    let dek = unwrap_dek(&kek, &envelope.wrapped_dek)?;
    let kek_as_dek = DataEncryptionKey::from_bytes(*kek.expose());
    assert_eq!(
        unwrap_fek(&kek_as_dek, &envelope.wrapped_dek).err(),
        Some(KryptError::Unwrap)
    );
    assert!(unwrap_fek(&dek, &envelope.wrapped_fek).is_ok());
    Ok(())
}

#[test]
fn nonces_and_salts_are_fresh() -> TestResult {
    let rmk = RootMasterKey::generate()?;
    let a = init_stratum(&rmk)?;
    let b = init_stratum(&rmk)?;
    assert_ne!(a.salt, b.salt);
    assert_ne!(a.wrapped_dek.nonce, b.wrapped_dek.nonce);
    assert_ne!(a.wrapped_fek.nonce, a.wrapped_dek.nonce);
    let fek = open_stratum(&rmk, &a)?;
    let x = encrypt_file(&fek, AAD, PLAIN)?;
    let y = encrypt_file(&fek, AAD, PLAIN)?;
    assert_ne!(x.nonce, y.nonce);
    assert_ne!(x.ciphertext, y.ciphertext);
    Ok(())
}

#[test]
fn malformed_wrapped_key_is_refused() -> TestResult {
    let rmk = RootMasterKey::generate()?;
    let mut envelope = init_stratum(&rmk)?;
    envelope.wrapped_dek.ciphertext.pop();
    assert_eq!(
        open_stratum(&rmk, &envelope).err(),
        Some(KryptError::MalformedWrappedKey)
    );
    Ok(())
}

#[test]
fn keys_zeroize_and_never_print() {
    let mut key = FileEncryptionKey::from_bytes([0xAB; KEY_LEN]);
    assert_eq!(format!("{key:?}"), "FileEncryptionKey(<redacted>)");
    key.zeroize();
    assert_eq!(key.expose(), &[0u8; KEY_LEN]);
}

#[test]
fn shredding_the_wrapped_dek_makes_data_unrecoverable() -> TestResult {
    let rmk = RootMasterKey::generate()?;
    let envelope = init_stratum(&rmk)?;
    let blob = encrypt_file(&open_stratum(&rmk, &envelope)?, AAD, PLAIN)?;
    let shredded = StratumEnvelope {
        wrapped_dek: WrappedKey {
            nonce: [0u8; NONCE_LEN],
            ciphertext: vec![0u8; WRAPPED_KEY_LEN],
        },
        ..envelope
    };
    assert_eq!(
        open_stratum(&rmk, &shredded).err(),
        Some(KryptError::Unwrap)
    );
    assert!(!blob.ciphertext.is_empty());
    Ok(())
}
