//! `freeze-vendor`: copy every dependency named in Cargo.lock into `vendor/` and point this
//! checkout's cargo at it, so builds, tests and the gates run with networking off.
//!
//! Both `vendor/` and `.cargo/config.toml` are committed (operator decision, 2026-10-22), so a fresh
//! clone builds with networking off. Cargo.lock's checksums pin every crate's bytes and cargo
//! re-checks them against `vendor/`; `.gitattributes` stops git from converting vendored files.
//! Re-run after any dependency change and commit the result.

use std::fs;
use std::path::Path;
use std::process::Command;

pub const VENDOR_DIR: &str = "vendor";
pub const CONFIG_PATH: &str = ".cargo/config.toml";

/// The source replacement written to `.cargo/config.toml`.
pub const CONFIG: &str = "\
# Written by `cargo run -p hcc-xtask -- freeze-vendor`; committed with vendor/ so clones build offline.
# After changing dependencies, re-run freeze-vendor and commit vendor/ and this file.
[source.crates-io]
replace-with = \"vendored-sources\"

[source.vendored-sources]
directory = \"vendor\"
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorReport {
    pub crates: usize,
    pub lock_packages: usize,
}

/// Run `cargo vendor --locked vendor`, then write the source replacement.
/// The replacement is removed first, so vendoring always reads the real registry/cache.
pub fn freeze(root: &Path) -> Result<VendorReport, String> {
    undo(root)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args(["vendor", "--locked", "--versioned-dirs", VENDOR_DIR])
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run cargo vendor: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo vendor failed (the first run needs the network once): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let crates = fs::read_dir(root.join(VENDOR_DIR))
        .map_err(|e| format!("{VENDOR_DIR}: {e}"))?
        .filter_map(Result::ok)
        .filter(|e| e.path().join(".cargo-checksum.json").is_file())
        .count();
    let lock =
        fs::read_to_string(root.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    let lock_packages = lock
        .split("[[package]]")
        .filter(|block| block.contains("source = \"registry+"))
        .count();
    if crates != lock_packages {
        return Err(format!(
            "vendor/ holds {crates} crates but Cargo.lock names {lock_packages} registry packages"
        ));
    }
    let config = root.join(CONFIG_PATH);
    if let Some(dir) = config.parent() {
        fs::create_dir_all(dir).map_err(|e| format!(".cargo: {e}"))?;
    }
    fs::write(&config, CONFIG).map_err(|e| format!("{CONFIG_PATH}: {e}"))?;
    Ok(VendorReport {
        crates,
        lock_packages,
    })
}

/// The config with comments and blank lines dropped: a config counts as freeze-vendor's when its
/// settings match, whatever its comment header says (older versions wrote other comments).
fn settings(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// Remove the source replacement (leaves `vendor/` on disk). Refuses to touch a config it did not write.
pub fn undo(root: &Path) -> Result<bool, String> {
    let config = root.join(CONFIG_PATH);
    match fs::read_to_string(&config) {
        Ok(text) if settings(&text) == settings(CONFIG) => {
            fs::remove_file(&config).map_err(|e| format!("{CONFIG_PATH}: {e}"))?;
            Ok(true)
        }
        Ok(_) => Err(format!(
            "{CONFIG_PATH} exists and was not written by freeze-vendor; leaving it alone"
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{CONFIG_PATH}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_removes_only_its_own_config() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!("hcc-vendor-undo-{}", std::process::id()));
        let config = root.join(CONFIG_PATH);
        fs::create_dir_all(root.join(".cargo")).map_err(|e| e.to_string())?;
        assert_eq!(undo(&root), Ok(false));
        fs::write(&config, "[build]\njobs = 2\n").map_err(|e| e.to_string())?;
        assert!(undo(&root).is_err(), "a hand-written config must survive");
        assert!(config.exists());
        fs::write(&config, CONFIG).map_err(|e| e.to_string())?;
        assert_eq!(undo(&root), Ok(true));
        let older = CONFIG.replace("committed with vendor/", "local only");
        fs::write(&config, older).map_err(|e| e.to_string())?;
        assert_eq!(undo(&root), Ok(true), "an older header is still ours");
        assert!(!config.exists());
        fs::remove_dir_all(&root).map_err(|e| e.to_string())
    }
}
