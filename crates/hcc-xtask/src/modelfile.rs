//! `ziggurat:1.0.0` Modelfile for the local runner (M09 weather).
//!
//! The SYSTEM block is the royal-sealed charter, byte for byte, so `ollama run ziggurat:1.0.0`
//! speaks under the same constitution the daemon injects. The charter file stays the only prompt
//! source: this file is written from it by `cargo run -p hcc-xtask -- modelfile --from <base>`,
//! only after the charter verifies against the baked seal, and a drift test pins the committed
//! copy. No PARAMETER lines: nothing here is tuned that the operator has not chosen.

use hcc_seal::{SealConstants, SystemCharter};

use crate::XtaskError;

/// Output path, relative to the workspace root.
pub const MODELFILE_PATH: &str = "models/ziggurat-1.0.0.Modelfile";
/// The model name the daemon and README use by default.
pub const MODEL_NAME: &str = "ziggurat:1.0.0";
pub const BASE_MAX: usize = 128;

/// Base model names: 1..=128 of `[A-Za-z0-9._:/-]`, so a name cannot add a Modelfile line.
pub fn base_ok(base: &str) -> bool {
    (1..=BASE_MAX).contains(&base.len())
        && base
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
}

/// Render the Modelfile. Refuses unless `charter` verifies against `seal` with a royal seal.
pub fn render(
    charter: &[u8],
    seal: Option<&SealConstants>,
    base: &str,
) -> Result<String, XtaskError> {
    if !base_ok(base) {
        return Err(XtaskError::Modelfile(
            "base model must be 1..=128 of [A-Za-z0-9._:/-]",
        ));
    }
    let sealed = SystemCharter::load(charter.to_vec())
        .verify(seal)
        .map_err(|refusal| XtaskError::Seal(refusal.error))?;
    let prompt = sealed.verified_prompt();
    if prompt.contains("\"\"\"") {
        return Err(XtaskError::Modelfile(
            "charter contains \"\"\", which would end the SYSTEM block",
        ));
    }
    if prompt.contains('\r') {
        return Err(XtaskError::Modelfile(
            "charter contains CR; it must be LF only",
        ));
    }
    Ok(format!(
        "# {MODEL_NAME}: written by `cargo run -p hcc-xtask -- modelfile --from {base}`. Do not edit by hand.\n\
         # SYSTEM is charters/ZIGGURAT_v1.0.0.txt byte for byte: charter_byte_len={} charter_blake3_hex={}\n\
         # The daemon still sends the sealed charter as the system message on every dispatch;\n\
         # this SYSTEM applies when the model is run directly.\n\
         FROM {base}\n\
         SYSTEM \"\"\"{prompt}\"\"\"\n",
        sealed.byte_len(),
        sealed.digest_hex(),
    ))
}

/// The base named on the `FROM` line of a rendered Modelfile.
pub fn from_line(modelfile: &str) -> Option<&str> {
    modelfile.lines().find_map(|l| l.strip_prefix("FROM "))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn root() -> std::path::PathBuf {
        crate::gates::without_dot_dot(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
    }

    #[test]
    fn committed_modelfile_matches_the_sealed_charter() -> TestResult {
        let root = root();
        let committed = std::fs::read_to_string(root.join(MODELFILE_PATH))?;
        let charter = std::fs::read(root.join("charters").join("ZIGGURAT_v1.0.0.txt"))?;
        let base = from_line(&committed).ok_or("no FROM line")?;
        let rendered = render(&charter, hcc_seal::BAKED_SEAL.as_ref(), base)?;
        assert_eq!(committed, rendered, "regenerate with xtask modelfile");
        let system = rendered
            .split_once("SYSTEM \"\"\"")
            .and_then(|(_, rest)| rest.strip_suffix("\"\"\"\n"))
            .ok_or("no SYSTEM block")?;
        assert_eq!(system.as_bytes(), charter.as_slice());
        Ok(())
    }

    #[test]
    fn refuses_a_charter_that_does_not_verify() -> TestResult {
        let root = root();
        let mut charter = std::fs::read(root.join("charters").join("ZIGGURAT_v1.0.0.txt"))?;
        charter.push(b'\n');
        assert!(matches!(
            render(&charter, hcc_seal::BAKED_SEAL.as_ref(), "cogito:latest"),
            Err(XtaskError::Seal(_))
        ));
        assert!(matches!(
            render(b"anything\n", None, "cogito:latest"),
            Err(XtaskError::Seal(_))
        ));
        Ok(())
    }

    #[test]
    fn base_names_cannot_inject_lines() {
        for bad in [
            "",
            "a b",
            "x\nSYSTEM \"\"\"evil\"\"\"",
            "x\"y",
            &"a".repeat(BASE_MAX + 1),
        ] {
            assert!(!base_ok(bad), "{bad:?}");
        }
        for good in ["cogito:latest", "llama3.1:8b", "hf.co/org/model:Q4_K_M"] {
            assert!(base_ok(good), "{good}");
        }
    }
}
