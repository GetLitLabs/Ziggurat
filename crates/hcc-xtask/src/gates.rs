//! M11 nine-gates audit (`INDEX.md` §14: eleven numbered checks).
//!
//! Every gate either runs a real command or exercises the real crate on a scratch database,
//! and reports what it saw. A gate whose subject does not exist yet (Tauri, `origami/dist`)
//! FAILS with that reason. Nothing here reports a pass it did not observe.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Set on every cargo the audit spawns, so a nested audit (a test that runs the audit) cannot
/// recurse into `cargo check` / `clippy` / `test` again.
pub const NESTED_ENV: &str = "HCC_GATES_NESTED";

pub const ALL: [u8; 11] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

/// Crates that must never appear in the dependency tree (zero egress, no TLS stacks).
pub const FORBIDDEN_CRATES: [&str; 6] = [
    "openssl",
    "reqwest",
    "native-tls",
    "trust-dns",
    "hickory",
    "rustls",
];

pub const LOOPBACK_HOSTS: [&str; 2] = ["127.0.0.1:7749", "127.0.0.1:11434"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateResult {
    pub gate: u8,
    pub name: &'static str,
    pub pass: bool,
    pub detail: String,
}

impl fmt::Display for GateResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GATE {:02} {}  {}: {}",
            self.gate,
            if self.pass { "PASS" } else { "FAIL" },
            self.name,
            self.detail
        )
    }
}

pub fn name(gate: u8) -> &'static str {
    match gate {
        1 => "cargo check --workspace --all-targets",
        2 => "#![forbid(unsafe_code)] on every crate",
        3 => "clippy deny warnings, unwrap, expect, panic",
        4 => "cargo test --workspace",
        5 => "no TLS or DNS crates in cargo tree",
        6 => "WAL on stele and BATON databases",
        7 => "seal tests use in-test generated keys",
        8 => "Tauri CSP names both loopback hosts",
        9 => "origami/dist exists with no non-loopback URLs",
        10 => "stele/BATON prev_digest chain + immutable triggers",
        11 => "krypt round-trip + healer empty wasm under fuel",
        _ => "unknown gate",
    }
}

/// Run the selected gates in order against the workspace at `root`.
pub fn run(root: &Path, selection: &[u8]) -> Vec<GateResult> {
    selection
        .iter()
        .map(|&gate| {
            let outcome = match gate {
                1 => cargo_gate(root, &["check", "--workspace", "--all-targets"]),
                2 => gate_forbid_unsafe(root),
                3 => cargo_gate(
                    root,
                    &[
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings",
                        "-D",
                        "clippy::unwrap_used",
                        "-D",
                        "clippy::expect_used",
                        "-D",
                        "clippy::panic",
                    ],
                ),
                4 => cargo_gate(root, &["test", "--workspace"]),
                5 => gate_tree(root),
                6 => gate_wal(),
                7 => gate_seal_keys(root),
                8 => gate_tauri_csp(root),
                9 => gate_origami(root),
                10 => gate_ledgers(),
                11 => gate_krypt_healer(root),
                _ => Err(format!("no gate {gate}; gates are 1..=11")),
            };
            let (pass, detail) = match outcome {
                Ok(detail) => (true, detail),
                Err(detail) => (false, detail),
            };
            GateResult {
                gate,
                name: name(gate),
                pass,
                detail,
            }
        })
        .collect()
}

fn cargo() -> Command {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.env(NESTED_ENV, "1");
    cmd
}

fn last_lines(bytes: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

fn cargo_gate(root: &Path, args: &[&str]) -> Result<String, String> {
    if std::env::var_os(NESTED_ENV).is_some() {
        return Err("refused inside a nested audit (would recurse into cargo)".into());
    }
    let mut full = vec!["--offline"];
    full.extend_from_slice(args);
    let out = cargo()
        .args(&full)
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`cargo {}` exited {}: {}",
            args.join(" "),
            out.status,
            last_lines(&out.stderr, 4)
        ));
    }
    if args.first() == Some(&"test") {
        let (mut passed, mut failed) = (0u64, 0u64);
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(rest) = line.strip_prefix("test result: ") {
                let words: Vec<&str> = rest.split_whitespace().collect();
                for pair in words.windows(2) {
                    let count = pair[0].parse::<u64>().unwrap_or(0);
                    match pair[1].trim_end_matches(';') {
                        "passed" => passed += count,
                        "failed" => failed += count,
                        _ => {}
                    }
                }
            }
        }
        return Ok(format!("{passed} passed, {failed} failed"));
    }
    Ok(format!("`cargo {}` exited 0", args.join(" ")))
}

/// Workspace members, read from the one-entry-per-line `members = [` block.
pub fn members(root: &Path) -> Result<Vec<String>, String> {
    let manifest =
        fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let mut inside = false;
    let mut out = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line == "members = [" {
            inside = true;
        } else if inside && line == "]" {
            break;
        } else if inside {
            out.push(line.trim_end_matches(',').trim_matches('"').to_string());
        }
    }
    if out.is_empty() {
        return Err("no workspace members found".into());
    }
    Ok(out)
}

/// Gate 2. The attribute is compiled by gates 1/3/4; this gate proves every crate carries it
/// (crate root attribute and the workspace lint), rather than grepping for the word `unsafe`.
fn gate_forbid_unsafe(root: &Path) -> Result<String, String> {
    let manifest =
        fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    if !manifest.contains("[workspace.lints.rust]")
        || !manifest.contains("unsafe_code = \"forbid\"")
    {
        return Err("workspace lints do not set unsafe_code = \"forbid\"".into());
    }
    let mut roots = 0;
    for member in members(root)? {
        let dir = root.join(&member);
        let crate_toml = fs::read_to_string(dir.join("Cargo.toml"))
            .map_err(|e| format!("{member}/Cargo.toml: {e}"))?;
        if !crate_toml.contains("[lints]\nworkspace = true") {
            return Err(format!("{member} does not inherit workspace lints"));
        }
        for file in ["src/lib.rs", "src/main.rs"] {
            let path = dir.join(file);
            if path.exists() {
                let text =
                    fs::read_to_string(&path).map_err(|e| format!("{member}/{file}: {e}"))?;
                if !text.contains("#![forbid(unsafe_code)]") {
                    return Err(format!("{member}/{file} lacks #![forbid(unsafe_code)]"));
                }
                roots += 1;
            }
        }
    }
    Ok(format!(
        "{roots} crate roots carry the attribute; every member inherits unsafe_code = forbid"
    ))
}

fn gate_tree(root: &Path) -> Result<String, String> {
    let out = cargo()
        .args([
            "tree",
            "--offline",
            "--workspace",
            "--prefix",
            "none",
            "-e",
            "normal,build",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run cargo tree: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo tree failed: {}", last_lines(&out.stderr, 3)));
    }
    let tree = String::from_utf8_lossy(&out.stdout);
    let mut packages: Vec<&str> = tree
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    packages.sort_unstable();
    packages.dedup();
    let hits: Vec<&str> = packages
        .iter()
        .copied()
        .filter(|p| FORBIDDEN_CRATES.iter().any(|f| p.starts_with(f)))
        .collect();
    if hits.is_empty() {
        Ok(format!(
            "{} packages, none of {}",
            packages.len(),
            FORBIDDEN_CRATES.join("/")
        ))
    } else {
        Err(format!("forbidden crates present: {}", hits.join(", ")))
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Result<Self, String> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("hcc-gates-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|e| format!("scratch dir: {e}"))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn journal_mode(path: &Path) -> Result<String, String> {
    let conn = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
    conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

fn gate_wal() -> Result<String, String> {
    let scratch = Scratch::new("wal")?;
    let stele_db = scratch.0.join("stele.db");
    let baton_db = scratch.0.join("baton.db");
    drop(hcc_iedb_stele::Stele::open(&stele_db).map_err(|e| format!("stele: {e}"))?);
    drop(hcc_baton::BatonLedger::open(&baton_db).map_err(|e| format!("baton: {e}"))?);
    let stele_mode = journal_mode(&stele_db)?;
    let baton_mode = journal_mode(&baton_db)?;
    if stele_mode == "wal" && baton_mode == "wal" {
        Ok("fresh stele and BATON files report journal_mode=wal".into())
    } else {
        Err(format!("stele {stele_mode}, baton {baton_mode}"))
    }
}

/// True if `text` holds a run of at least 64 hex digits (a key or signature literal).
fn has_long_hex(text: &str) -> bool {
    let mut run = 0;
    for c in text.chars() {
        run = if c.is_ascii_hexdigit() { run + 1 } else { 0 };
        if run >= 64 {
            return true;
        }
    }
    false
}

fn gate_seal_keys(root: &Path) -> Result<String, String> {
    let dir = root.join("crates/hcc-seal/tests");
    let entries = fs::read_dir(&dir).map_err(|e| format!("crates/hcc-seal/tests: {e}"))?;
    let mut generated = 0;
    let mut files = 0;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        files += 1;
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let shown = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        if has_long_hex(&text) {
            return Err(format!("{shown} holds a 64+ hex-digit literal"));
        }
        if text.contains("SigningKey::from_bytes") && text.contains("getrandom::getrandom") {
            generated += 1;
        }
    }
    if generated == 0 {
        return Err("no seal test generates its key with getrandom".into());
    }
    Ok(format!(
        "{files} seal test files, no hex key literals; {generated} generate keys with getrandom"
    ))
}

fn gate_tauri_csp(root: &Path) -> Result<String, String> {
    let conf = root.join("src-tauri/tauri.conf.json");
    let text = fs::read_to_string(&conf).map_err(|_| {
        "src-tauri/tauri.conf.json absent: the Tauri shell is not built yet".to_string()
    })?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("tauri.conf.json: {e}"))?;
    let csp = json
        .pointer("/app/security/csp")
        .and_then(serde_json::Value::as_str)
        .ok_or("tauri.conf.json has no app.security.csp string")?;
    let missing: Vec<&str> = LOOPBACK_HOSTS
        .iter()
        .copied()
        .filter(|h| !csp.contains(h))
        .collect();
    if missing.is_empty() {
        Ok("CSP names 127.0.0.1:7749 and 127.0.0.1:11434".into())
    } else {
        Err(format!("CSP is missing {}", missing.join(", ")))
    }
}

/// Every `scheme://host` in `text` whose host is not loopback. XML namespace URIs are not fetched.
pub fn non_loopback_urls(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let after = &rest[i + 3..];
        let end = after
            .find(|c: char| {
                !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
            })
            .unwrap_or(after.len());
        let host_port = &after[..end];
        let host = host_port.split(':').next().unwrap_or("");
        let namespace = after.starts_with("www.w3.org/");
        if !host.is_empty() && host != "127.0.0.1" && !namespace {
            found.push(host_port.to_string());
        }
        rest = after;
    }
    found
}

fn gate_origami(root: &Path) -> Result<String, String> {
    let dist = root.join("origami/dist");
    if !dist.is_dir() {
        return Err("origami/dist absent: the cockpit is not built yet".into());
    }
    let mut stack = vec![dist];
    let mut files = 0;
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            files += 1;
            if let Ok(text) = fs::read_to_string(&path) {
                if let Some(url) = non_loopback_urls(&text).first() {
                    return Err(format!("{} names {url}", path.display()));
                }
            }
        }
    }
    if files == 0 {
        return Err("origami/dist is empty".into());
    }
    Ok(format!("{files} files, no non-loopback URLs"))
}

fn expect_refused(conn: &rusqlite::Connection, sql: &str, needle: &str) -> Result<(), String> {
    match conn.execute(sql, []) {
        Ok(_) => Err(format!("`{sql}` was allowed")),
        Err(e) if e.to_string().contains(needle) => Ok(()),
        Err(e) => Err(format!("`{sql}` failed for the wrong reason: {e}")),
    }
}

fn gate_ledgers() -> Result<String, String> {
    let scratch = Scratch::new("ledgers")?;
    let stele_db = scratch.0.join("stele.db");
    let baton_db = scratch.0.join("baton.db");
    let pubkey = *blake3::hash(b"gate 10 scratch pubkey").as_bytes();
    let charter = *blake3::hash(b"gate 10 scratch charter").as_bytes();
    {
        let mut stele = hcc_iedb_stele::Stele::open(&stele_db).map_err(|e| e.to_string())?;
        stele
            .append("inspect.gate.validate", b"one", &pubkey, None)
            .map_err(|e| e.to_string())?;
        stele
            .append("inspect.gate.validate", b"two", &pubkey, None)
            .map_err(|e| e.to_string())?;
        if stele.verify_chain().map_err(|e| e.to_string())? != 2 {
            return Err("stele chain did not verify 2 rows".into());
        }
        let mut baton = hcc_baton::BatonLedger::open(&baton_db).map_err(|e| e.to_string())?;
        let terrace = hcc_types::TerraceLevel::try_from(4u8).map_err(|e| e.to_string())?;
        for prompt in [b"p1", b"p2"] {
            baton
                .record(&charter, "inspect.gate.validate", terrace, prompt, b"r")
                .map_err(|e| e.to_string())?;
        }
        if baton.replay(&charter).map_err(|e| e.to_string())? != 2 {
            return Err("BATON replay did not verify 2 traces".into());
        }
    }
    let stele = rusqlite::Connection::open(&stele_db).map_err(|e| e.to_string())?;
    expect_refused(
        &stele,
        "UPDATE hcc_precedent_stele SET payload = payload",
        "append-only",
    )?;
    expect_refused(&stele, "DELETE FROM hcc_precedent_stele", "append-only")?;
    expect_refused(
        &stele,
        "INSERT INTO hcc_precedent_stele (prev_digest, module_namespace, version_tag, domain_separator, payload, payload_byte_len, blake3_digest, ed25519_pubkey) VALUES (zeroblob(32), 'a.b', '1.0.0', X'4843432D5354454C4C452D763200', X'00', 1, randomblob(32), zeroblob(32))",
        "does not link",
    )?;
    let baton = rusqlite::Connection::open(&baton_db).map_err(|e| e.to_string())?;
    expect_refused(
        &baton,
        "UPDATE hcc_baton_trace_ledger SET terrace_level = 1",
        "append-only",
    )?;
    expect_refused(&baton, "DELETE FROM hcc_baton_trace_ledger", "append-only")?;
    expect_refused(
        &baton,
        "INSERT INTO hcc_baton_trace_ledger (prev_digest, module_namespace, sequence_number, terrace_level, input_state_hash, output_state_hash, baton_envelope_sig) VALUES (zeroblob(32), 'a.b', 3, 1, zeroblob(32), zeroblob(32), randomblob(32))",
        "does not link",
    )?;
    Ok("2-row stele and BATON chains verify; UPDATE, DELETE and an unlinked INSERT are refused on both".into())
}

fn gate_krypt_healer(root: &Path) -> Result<String, String> {
    use hcc_krypt::{decrypt_file, encrypt_file, init_stratum, open_stratum, RootMasterKey};
    let rmk = RootMasterKey::generate().map_err(|e| e.to_string())?;
    let envelope = init_stratum(&rmk).map_err(|e| e.to_string())?;
    let fek = open_stratum(&rmk, &envelope).map_err(|e| e.to_string())?;
    let blob = encrypt_file(&fek, b"gate11", b"krypt round trip").map_err(|e| e.to_string())?;
    let plain = decrypt_file(&fek, b"gate11", &blob).map_err(|e| e.to_string())?;
    if plain.as_slice() != b"krypt round trip" {
        return Err("krypt round trip returned different bytes".into());
    }
    if decrypt_file(&fek, b"other aad", &blob).is_ok() {
        return Err("krypt decrypted under the wrong AAD".into());
    }
    let wasm = fs::read(root.join(hcc_wasi_healer::EMPTY_FIXTURE_PATH))
        .map_err(|e| format!("{}: {e}", hcc_wasi_healer::EMPTY_FIXTURE_PATH))?;
    let policy = hcc_wasi_healer::SandboxPolicy::LOCKED;
    let report = hcc_wasi_healer::diagnose(&wasm, &policy).map_err(|e| format!("healer: {e}"))?;
    Ok(format!(
        "RMK→KEK→DEK→FEK round trip ok, wrong AAD refused; {}-byte fixture ran under fuel cap {} (used {})",
        report.wasm_byte_len, report.fuel_cap, report.fuel_consumed
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_scan_flags_only_non_loopback() {
        assert!(non_loopback_urls("fetch('http://127.0.0.1:7749/v1/x')").is_empty());
        assert!(non_loopback_urls("xmlns=\"http://www.w3.org/2000/svg\"").is_empty());
        assert_eq!(
            non_loopback_urls("https://cdn.example.com/a.js"),
            ["cdn.example.com"]
        );
        assert_eq!(non_loopback_urls("ws://localhost:1"), ["localhost:1"]);
    }

    #[test]
    fn long_hex_detector() {
        assert!(!has_long_hex("deadbeef"));
        assert!(has_long_hex(&"ab".repeat(32)));
    }
}
