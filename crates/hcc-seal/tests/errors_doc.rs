//! SPEC/ERRORS.md must list every `SealError` string verbatim.

use std::path::Path;

use hcc_seal::SealError;

#[test]
fn every_seal_error_is_documented() -> Result<(), std::io::Error> {
    let doc = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../SPEC/ERRORS.md"),
    )?;
    for error in SealError::ALL {
        let row = format!("| `{error}` |");
        assert!(doc.contains(&row), "SPEC/ERRORS.md is missing {row}");
    }
    Ok(())
}

#[test]
fn seal_errors_carry_no_lengths_or_paths() {
    let mismatch = SealError::ByteLenMismatch {
        expected: 4660,
        actual: 4661,
    };
    assert!(!mismatch.to_string().contains("466"));
    for error in SealError::ALL {
        let text = error.to_string();
        assert!(!text.contains('/') && !text.contains('\\'), "{text}");
    }
}
