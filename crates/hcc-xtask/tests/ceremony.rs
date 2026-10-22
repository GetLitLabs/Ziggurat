//! Seal ceremony tests. Keys are generated per test in a scratch dir.

use std::path::PathBuf;

use hcc_seal::{charter_digest, digest_hex, STELE_DOMAIN};
use hcc_xtask::{hygiene, keygen, read_hex_file, seal, sign_charter, XtaskError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CLEAN: &[u8] = b"line one\nline two\n";

fn scratch(name: &str) -> Result<PathBuf, std::io::Error> {
    let dir = std::env::temp_dir().join(format!("hcc-xtask-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn is_hygiene(result: Result<(), XtaskError>) -> bool {
    matches!(result, Err(XtaskError::Hygiene(_)))
}

#[test]
fn hygiene_rejects_crlf_bom_and_bad_endings() {
    assert!(hygiene(CLEAN).is_ok());
    assert!(is_hygiene(hygiene(b"line one\r\nline two\r\n")));
    assert!(is_hygiene(hygiene(b"\xef\xbb\xbfline one\n")));
    assert!(is_hygiene(hygiene(b"line one")));
    assert!(is_hygiene(hygiene(b"line one\n\n")));
    assert!(is_hygiene(hygiene(b"\xff\n")));
}

#[test]
fn seal_is_deterministic_and_domain_separated() -> TestResult {
    let first = seal(CLEAN, None)?;
    let second = seal(CLEAN, None)?;
    assert_eq!(first.baked_rs, second.baked_rs);
    assert_eq!(first.byte_len, CLEAN.len());
    assert_eq!(first.digest_hex, digest_hex(&charter_digest(CLEAN)));
    let mut joined = STELE_DOMAIN.to_vec();
    joined.extend_from_slice(CLEAN);
    assert_eq!(first.digest_hex, blake3::hash(&joined).to_hex().as_str());
    assert_ne!(first.digest_hex, first.raw_b3sum_hex);
    assert!(!first.royal);
    assert!(first.baked_rs.contains("royal: None"));
    Ok(())
}

#[test]
fn keygen_sign_seal_round_trip() -> TestResult {
    let dir = scratch("roundtrip")?;
    let pubkey = keygen(&dir)?;
    assert_eq!(read_hex_file::<32>(&dir.join("operator.pub"))?, pubkey);
    let secret = read_hex_file::<32>(&dir.join("operator.key"))?;
    let (signer_pub, signature) = sign_charter(CLEAN, &secret)?;
    assert_eq!(signer_pub, pubkey);
    let report = seal(CLEAN, Some((pubkey, signature)))?;
    assert!(report.royal);
    assert!(report.baked_rs.contains("Some(RoyalSeal {"));
    assert_eq!(
        report.baked_rs,
        seal(CLEAN, Some((pubkey, signature)))?.baked_rs
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("operator.key"))?
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(
        matches!(keygen(&dir), Err(XtaskError::Io(_))),
        "keygen must not overwrite"
    );
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn seal_refuses_signature_over_other_bytes() -> TestResult {
    let dir = scratch("mismatch")?;
    let pubkey = keygen(&dir)?;
    let secret = read_hex_file::<32>(&dir.join("operator.key"))?;
    let (_, signature) = sign_charter(b"a different charter\n", &secret)?;
    assert!(matches!(
        seal(CLEAN, Some((pubkey, signature))),
        Err(XtaskError::Seal(_))
    ));
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
