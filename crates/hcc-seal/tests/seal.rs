//! Seal tests. Every key is generated inside the test; no baked fantasy hex.

use ed25519_dalek::{Signer, SigningKey};
use hcc_seal::{
    charter_digest, digest_hex, verify_royal_seal, RoyalSeal, SealConstants, SealError,
    SystemCharter, BATON_DOMAIN, STELE_DOMAIN,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SAMPLE: &[u8] = b"sample charter\n";

fn fresh_key() -> Result<SigningKey, getrandom::Error> {
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret)?;
    Ok(SigningKey::from_bytes(&secret))
}

fn constants_for(bytes: &[u8], key: Option<&SigningKey>) -> SealConstants {
    let digest = charter_digest(bytes);
    SealConstants {
        byte_len: bytes.len(),
        blake3: digest,
        royal: key.map(|k| RoyalSeal {
            operator_pubkey: k.verifying_key().to_bytes(),
            signature: k.sign(&digest).to_bytes(),
        }),
    }
}

#[test]
fn domains_are_frozen() {
    assert_eq!(STELE_DOMAIN.len(), 14);
    assert_eq!(&STELE_DOMAIN[..], b"HCC-STELLE-v2\0");
    assert_eq!(BATON_DOMAIN.len(), 13);
    assert_eq!(&BATON_DOMAIN[..], b"HCC-BATON-v1\0");
}

#[test]
fn digest_is_domain_separated() {
    let mut joined = STELE_DOMAIN.to_vec();
    joined.extend_from_slice(SAMPLE);
    assert_eq!(charter_digest(SAMPLE), *blake3::hash(&joined).as_bytes());
    assert_ne!(charter_digest(SAMPLE), *blake3::hash(SAMPLE).as_bytes());
    assert_eq!(
        digest_hex(&charter_digest(SAMPLE)),
        blake3::hash(&joined).to_hex().as_str()
    );
}

#[test]
fn good_seal_reaches_royal_sealed() -> TestResult {
    let key = fresh_key()?;
    let expected = constants_for(SAMPLE, Some(&key));
    let sealed = SystemCharter::load(SAMPLE.to_vec())
        .verify(Some(&expected))
        .map_err(|r| r.error.to_string())?;
    assert_eq!(sealed.verified_prompt(), "sample charter\n");
    assert_eq!(sealed.byte_len(), SAMPLE.len());
    assert_eq!(sealed.digest(), &charter_digest(SAMPLE));
    Ok(())
}

fn refusal(bytes: &[u8], expected: Option<&SealConstants>) -> Option<SealError> {
    SystemCharter::load(bytes.to_vec())
        .verify(expected)
        .err()
        .map(|r| r.error)
}

#[test]
fn nothing_baked_is_refused() {
    assert_eq!(refusal(SAMPLE, None), Some(SealError::NotBaked));
}

#[test]
fn one_changed_byte_is_refused() -> TestResult {
    let key = fresh_key()?;
    let expected = constants_for(SAMPLE, Some(&key));
    assert_eq!(
        refusal(b"sample charteR\n", Some(&expected)),
        Some(SealError::DigestMismatch)
    );
    Ok(())
}

#[test]
fn crlf_is_refused_by_length() -> TestResult {
    let key = fresh_key()?;
    let expected = constants_for(SAMPLE, Some(&key));
    assert_eq!(
        refusal(b"sample charter\r\n", Some(&expected)),
        Some(SealError::ByteLenMismatch {
            expected: 15,
            actual: 16
        })
    );
    Ok(())
}

#[test]
fn digest_without_royal_seal_is_refused() {
    let expected = constants_for(SAMPLE, None);
    assert_eq!(
        refusal(SAMPLE, Some(&expected)),
        Some(SealError::NoRoyalSeal)
    );
}

#[test]
fn signature_from_another_key_is_refused() -> TestResult {
    let operator = fresh_key()?;
    let impostor = fresh_key()?;
    let mut expected = constants_for(SAMPLE, Some(&operator));
    if let Some(royal) = expected.royal.as_mut() {
        royal.signature = impostor.sign(&charter_digest(SAMPLE)).to_bytes();
    }
    assert_eq!(
        refusal(SAMPLE, Some(&expected)),
        Some(SealError::BadSignature)
    );
    Ok(())
}

#[test]
fn signature_over_raw_bytes_is_refused() -> TestResult {
    let key = fresh_key()?;
    let royal = RoyalSeal {
        operator_pubkey: key.verifying_key().to_bytes(),
        signature: key.sign(SAMPLE).to_bytes(),
    };
    assert_eq!(
        verify_royal_seal(&royal, &charter_digest(SAMPLE)),
        Err(SealError::BadSignature)
    );
    Ok(())
}

#[test]
fn refusal_still_reports_real_length_and_digest() {
    let refused = SystemCharter::load(SAMPLE.to_vec()).verify(None);
    let charter = refused.err().map(|r| r.charter);
    assert_eq!(charter.as_ref().map(|c| c.byte_len()), Some(SAMPLE.len()));
    assert_eq!(
        charter.as_ref().map(|c| *c.digest()),
        Some(charter_digest(SAMPLE))
    );
}

#[test]
fn non_utf8_is_refused_after_digest_matches() -> TestResult {
    let bytes = [0xffu8, 0xfe, b'\n'];
    let key = fresh_key()?;
    let expected = constants_for(&bytes, Some(&key));
    assert_eq!(refusal(&bytes, Some(&expected)), Some(SealError::NotUtf8));
    Ok(())
}
