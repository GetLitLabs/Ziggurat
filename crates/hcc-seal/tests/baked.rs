//! Ties the baked constants to the frozen charter file on disk.

use std::path::Path;

use hcc_seal::{SealError, SystemCharter, BAKED_SEAL};

const CHARTER: &str = "../../charters/ZIGGURAT_v1.0.0.txt";

#[test]
fn baked_seal_matches_charter_file() -> Result<(), std::io::Error> {
    let bytes = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(CHARTER))?;
    let charter = SystemCharter::load(bytes);
    match BAKED_SEAL {
        None => {
            assert_eq!(
                charter.verify(None).err().map(|r| r.error),
                Some(SealError::NotBaked)
            );
        }
        Some(baked) => {
            assert_eq!(
                charter.byte_len(),
                baked.byte_len,
                "charter length drifted from seal"
            );
            assert_eq!(
                *charter.digest(),
                baked.blake3,
                "charter bytes drifted from seal"
            );
            match (baked.royal, charter.verify(Some(&baked))) {
                (None, outcome) => {
                    assert_eq!(outcome.err().map(|r| r.error), Some(SealError::NoRoyalSeal));
                }
                (Some(_), outcome) => {
                    assert_eq!(
                        outcome.err().map(|r| r.error),
                        None,
                        "royal seal must verify"
                    );
                }
            }
        }
    }
    Ok(())
}
