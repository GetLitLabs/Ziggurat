//! Code A Seed (M05 `seed.manifest.init`, `seed.scaffold.emit`).
//!
//! A validated blueprint becomes `crates/<name>/{Cargo.toml, src/lib.rs, tests/schema.rs, rustfmt.toml}`.
//! Rendering is deterministic: the same blueprint always yields the same bytes. Identifiers
//! and SQL types come from allow-lists, so nothing in a blueprint can inject Rust or SQL.
#![forbid(unsafe_code)]

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blueprint {
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub tables: Vec<TableSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableSpec {
    pub name: String,
    pub columns: Vec<ColumnSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub sql_type: SqlType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SqlType {
    #[serde(rename = "INTEGER")]
    Integer,
    #[serde(rename = "TEXT")]
    Text,
    #[serde(rename = "BLOB")]
    Blob,
    #[serde(rename = "REAL")]
    Real,
}

impl SqlType {
    fn sql(self) -> &'static str {
        match self {
            Self::Integer => "INTEGER",
            Self::Text => "TEXT",
            Self::Blob => "BLOB",
            Self::Real => "REAL",
        }
    }

    /// A literal of this type, for the generated round-trip test.
    fn sample(self) -> &'static str {
        match self {
            Self::Integer => "1",
            Self::Text => "'seed'",
            Self::Blob => "X'00'",
            Self::Real => "1.5",
        }
    }
}

#[derive(Debug)]
pub enum SeedError {
    BadJson(String),
    BadName(String),
    BadSummary,
    BadIdentifier(String),
    EmptyTable(String),
    DuplicateIdentifier(String),
    AlreadyExists(PathBuf),
    MembersBlock,
    Io(std::io::Error),
    CargoCheck(String),
}

impl fmt::Display for SeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadJson(err) => write!(f, "blueprint JSON: {err}"),
            Self::BadName(name) => write!(
                f,
                "crate name {name:?} must match [a-z][a-z0-9_-]{{0,39}} and not start with hcc-"
            ),
            Self::BadSummary => f.write_str(
                "summary must be 1-200 printable ASCII chars without quotes or backslashes",
            ),
            Self::BadIdentifier(id) => write!(
                f,
                "identifier {id:?} must match [a-z][a-z0-9_]{{0,39}} and not be reserved"
            ),
            Self::EmptyTable(t) => write!(f, "table {t:?} needs at least one column"),
            Self::DuplicateIdentifier(id) => write!(f, "duplicate identifier {id:?}"),
            Self::AlreadyExists(path) => write!(
                f,
                "{} already exists; seed never overwrites",
                path.display()
            ),
            Self::MembersBlock => {
                f.write_str("workspace Cargo.toml has no one-entry-per-line `members = [` block")
            }
            Self::Io(err) => write!(f, "io: {err}"),
            Self::CargoCheck(out) => write!(f, "cargo check failed:\n{out}"),
        }
    }
}

impl std::error::Error for SeedError {}

impl From<std::io::Error> for SeedError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

const RESERVED: [&str; 24] = [
    "id",
    "select",
    "from",
    "where",
    "table",
    "index",
    "order",
    "group",
    "by",
    "insert",
    "update",
    "delete",
    "create",
    "drop",
    "primary",
    "key",
    "references",
    "default",
    "check",
    "unique",
    "null",
    "not",
    "and",
    "or",
];

fn ident_ok(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some('a'..='z'))
        && id.len() <= 40
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
        && !RESERVED.contains(&id)
}

impl Blueprint {
    /// `seed.manifest.init`: parse and validate.
    pub fn from_json(text: &str) -> Result<Self, SeedError> {
        let blueprint: Blueprint =
            serde_json::from_str(text).map_err(|e| SeedError::BadJson(e.to_string()))?;
        blueprint.validate()?;
        Ok(blueprint)
    }

    pub fn validate(&self) -> Result<(), SeedError> {
        let mut chars = self.name.chars();
        let name_ok = matches!(chars.next(), Some('a'..='z'))
            && self.name.len() <= 40
            && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
            && !self.name.ends_with('-')
            && !self.name.starts_with("hcc-");
        if !name_ok {
            return Err(SeedError::BadName(self.name.clone()));
        }
        let summary_ok = (1..=200).contains(&self.summary.len())
            && self
                .summary
                .chars()
                .all(|c| c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\');
        if !summary_ok {
            return Err(SeedError::BadSummary);
        }
        let mut tables = Vec::new();
        for table in &self.tables {
            if !ident_ok(&table.name) {
                return Err(SeedError::BadIdentifier(table.name.clone()));
            }
            if tables.contains(&table.name.as_str()) {
                return Err(SeedError::DuplicateIdentifier(table.name.clone()));
            }
            tables.push(&table.name);
            if table.columns.is_empty() {
                return Err(SeedError::EmptyTable(table.name.clone()));
            }
            let mut columns = Vec::new();
            for column in &table.columns {
                if !ident_ok(&column.name) {
                    return Err(SeedError::BadIdentifier(column.name.clone()));
                }
                if columns.contains(&column.name.as_str()) {
                    return Err(SeedError::DuplicateIdentifier(column.name.clone()));
                }
                columns.push(&column.name);
            }
        }
        Ok(())
    }

    /// BLAKE3 of the canonical JSON (fixed field order, no whitespace).
    pub fn digest(&self) -> Result<[u8; 32], SeedError> {
        let canonical = serde_json::to_vec(self).map_err(|e| SeedError::BadJson(e.to_string()))?;
        Ok(*blake3::hash(&canonical).as_bytes())
    }

    pub fn crate_ident(&self) -> String {
        self.name.replace('-', "_")
    }

    pub fn member_path(&self) -> String {
        format!("crates/{}", self.name)
    }

    fn schema_sql(&self) -> String {
        let mut sql = String::new();
        for table in &self.tables {
            sql.push_str(&format!(
                "CREATE TABLE IF NOT EXISTS {} (\n  id INTEGER PRIMARY KEY",
                table.name
            ));
            for column in &table.columns {
                sql.push_str(&format!(
                    ",\n  {} {} NOT NULL",
                    column.name,
                    column.sql_type.sql()
                ));
            }
            sql.push_str("\n);\n");
        }
        sql
    }
}

/// One rendered file: path relative to the crate directory, and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedFile {
    pub path: &'static str,
    pub contents: String,
}

/// Deterministic render of every file for `blueprint`.
pub fn render(blueprint: &Blueprint) -> Result<Vec<SeedFile>, SeedError> {
    blueprint.validate()?;
    let digest_hex = blake3::Hash::from(blueprint.digest()?).to_hex();
    let ident = blueprint.crate_ident();
    let has_db = !blueprint.tables.is_empty();
    let deps = if has_db {
        "\n[dependencies]\nrusqlite.workspace = true\n"
    } else {
        ""
    };
    let cargo_toml = format!(
        "[package]\nname = \"{name}\"\ndescription = \"{summary}\"\nversion.workspace = true\nedition.workspace = true\nlicense.workspace = true\npublish.workspace = true\n{deps}\n[lints]\nworkspace = true\n",
        name = blueprint.name,
        summary = blueprint.summary,
    );
    let header = format!(
        "//! {name}: {summary}\n//!\n//! Generated by hcc-seed (`seed.scaffold.emit`) from blueprint blake3 {digest_hex}.\n#![forbid(unsafe_code)]\n",
        name = blueprint.name,
        summary = blueprint.summary,
    );
    let (lib_rs, test_rs) = if has_db {
        let tables: Vec<String> = blueprint
            .tables
            .iter()
            .map(|t| format!("{:?}", t.name))
            .collect();
        let inserts: Vec<String> = blueprint
            .tables
            .iter()
            .map(|t| {
                let cols: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
                let vals: Vec<&str> = t.columns.iter().map(|c| c.sql_type.sample()).collect();
                format!(
                    "{:?}",
                    format!(
                        "INSERT INTO {} ({}) VALUES ({})",
                        t.name,
                        cols.join(", "),
                        vals.join(", ")
                    )
                )
            })
            .collect();
        let lib = format!(
            "{header}\nuse std::path::Path;\n\nuse rusqlite::Connection;\n\n/// Every table this crate owns.\npub const TABLES: [&str; {n}] = [{tables}];\n\npub const SCHEMA: &str = {schema:?};\n\n/// Open (or create) `path` in WAL mode and apply [`SCHEMA`].\npub fn open(path: &Path) -> rusqlite::Result<Connection> {{\n    init(Connection::open(path)?)\n}}\n\npub fn open_in_memory() -> rusqlite::Result<Connection> {{\n    init(Connection::open_in_memory()?)\n}}\n\nfn init(conn: Connection) -> rusqlite::Result<Connection> {{\n    let _mode: String = conn.query_row(\"PRAGMA journal_mode = WAL\", [], |r| r.get(0))?;\n    conn.execute_batch(\"PRAGMA synchronous = NORMAL;\")?;\n    conn.execute_batch(SCHEMA)?;\n    Ok(conn)\n}}\n",
            n = tables.len(),
            tables = tables.join(", "),
            schema = blueprint.schema_sql(),
        );
        let test = format!(
            "//! Generated by hcc-seed. Proves the schema applies, round-trips a row per table, and uses WAL.\n\nuse {ident}::{{open, open_in_memory, TABLES}};\n\n#[test]\nfn schema_creates_every_table() -> rusqlite::Result<()> {{\n    let conn = open_in_memory()?;\n    for table in TABLES {{\n        let n: i64 = conn.query_row(\n            \"SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1\",\n            [table],\n            |r| r.get(0),\n        )?;\n        assert_eq!(n, 1, \"{{table}}\");\n    }}\n    Ok(())\n}}\n\n#[test]\nfn every_table_round_trips_a_row() -> rusqlite::Result<()> {{\n    let conn = open_in_memory()?;\n    for (table, insert) in TABLES.iter().zip([{inserts}]) {{\n        conn.execute(insert, [])?;\n        let n: i64 = conn.query_row(&format!(\"SELECT count(*) FROM {{table}}\"), [], |r| r.get(0))?;\n        assert_eq!(n, 1, \"{{table}}\");\n    }}\n    Ok(())\n}}\n\n#[test]\nfn file_database_uses_wal() -> Result<(), Box<dyn std::error::Error>> {{\n    let dir = std::env::temp_dir().join(format!(\"{ident}-wal-{{}}\", std::process::id()));\n    std::fs::create_dir_all(&dir)?;\n    let conn = open(&dir.join(\"{ident}.db\"))?;\n    let mode: String = conn.query_row(\"PRAGMA journal_mode\", [], |r| r.get(0))?;\n    assert_eq!(mode, \"wal\");\n    drop(conn);\n    std::fs::remove_dir_all(&dir)?;\n    Ok(())\n}}\n",
            inserts = inserts.join(", "),
        );
        (lib, test)
    } else {
        let lib = format!(
            "{header}\npub const NAME: &str = {name:?};\npub const SUMMARY: &str = {summary:?};\n",
            name = blueprint.name,
            summary = blueprint.summary,
        );
        let test = format!(
            "//! Generated by hcc-seed.\n\n#[test]\nfn identity_matches_blueprint() {{\n    assert_eq!({ident}::NAME, {name:?});\n    assert_eq!({ident}::SUMMARY, {summary:?});\n}}\n",
            name = blueprint.name,
            summary = blueprint.summary,
        );
        (lib, test)
    };
    Ok(vec![
        SeedFile {
            path: "Cargo.toml",
            contents: cargo_toml,
        },
        SeedFile {
            path: "src/lib.rs",
            contents: lib_rs,
        },
        SeedFile {
            path: "tests/schema.rs",
            contents: test_rs,
        },
        // Generated bytes are pinned to the blueprint digest; reformatting would break that.
        SeedFile {
            path: "rustfmt.toml",
            contents: "# Generated by hcc-seed: these files are pinned to their blueprint. Re-seed, don't edit.\ndisable_all_formatting = true\n".to_string(),
        },
    ])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitReport {
    pub crate_dir: PathBuf,
    pub blueprint_blake3: [u8; 32],
    /// (path relative to the workspace root, BLAKE3 of the file bytes)
    pub files: Vec<(String, [u8; 32])>,
}

/// `seed.scaffold.emit`: write the crate under `<workspace_root>/crates/<name>`. Never overwrites.
/// Files are written to a sibling temp directory and renamed into place.
pub fn emit(blueprint: &Blueprint, workspace_root: &Path) -> Result<EmitReport, SeedError> {
    let files = render(blueprint)?;
    let crate_dir = workspace_root.join("crates").join(&blueprint.name);
    if crate_dir.exists() {
        return Err(SeedError::AlreadyExists(crate_dir));
    }
    let staging = workspace_root
        .join("crates")
        .join(format!(".{}.seed-staging", blueprint.name));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    let written = (|| -> Result<Vec<(String, [u8; 32])>, SeedError> {
        let mut hashes = Vec::new();
        for file in &files {
            let path = staging.join(file.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, file.contents.as_bytes())?;
            hashes.push((
                format!("{}/{}", blueprint.member_path(), file.path),
                *blake3::hash(file.contents.as_bytes()).as_bytes(),
            ));
        }
        fs::rename(&staging, &crate_dir)?;
        Ok(hashes)
    })();
    match written {
        Ok(files) => Ok(EmitReport {
            crate_dir,
            blueprint_blake3: blueprint.digest()?,
            files,
        }),
        Err(err) => {
            if staging.exists() {
                fs::remove_dir_all(&staging)?;
            }
            Err(err)
        }
    }
}

/// Deterministically add `member` to a `members = [ ... ]` block written one entry per line.
/// Returns the text unchanged if the member is already present.
pub fn add_member(cargo_toml: &str, member: &str) -> Result<String, SeedError> {
    let lines: Vec<&str> = cargo_toml.split_inclusive('\n').collect();
    let start = lines
        .iter()
        .position(|l| l.trim_end() == "members = [")
        .ok_or(SeedError::MembersBlock)?;
    let end = lines
        .iter()
        .skip(start + 1)
        .position(|l| l.trim_end() == "]")
        .map(|i| i + start + 1)
        .ok_or(SeedError::MembersBlock)?;
    let entry = format!("\"{member}\",");
    let entries = lines.get(start + 1..end).ok_or(SeedError::MembersBlock)?;
    if entries.iter().any(|l| l.trim() == entry) {
        return Ok(cargo_toml.to_string());
    }
    if !entries
        .iter()
        .all(|l| l.trim().starts_with('"') && l.trim().ends_with("\","))
    {
        return Err(SeedError::MembersBlock);
    }
    let mut out = String::with_capacity(cargo_toml.len() + entry.len() + 5);
    for line in lines.get(..end).ok_or(SeedError::MembersBlock)? {
        out.push_str(line);
    }
    out.push_str(&format!("    {entry}\n"));
    for line in lines.get(end..).ok_or(SeedError::MembersBlock)? {
        out.push_str(line);
    }
    Ok(out)
}

/// Inverse of [`add_member`], for rollback.
pub fn remove_member(cargo_toml: &str, member: &str) -> String {
    let entry = format!("\"{member}\",");
    cargo_toml
        .split_inclusive('\n')
        .filter(|l| l.trim() != entry)
        .collect()
}

/// `cargo check --offline -p <name>` in `workspace_root`.
pub fn cargo_check(workspace_root: &Path, package: &str) -> Result<(), SeedError> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["check", "--offline", "--quiet", "-p", package])
        .current_dir(workspace_root)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(SeedError::CargoCheck(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedReport {
    pub emit: EmitReport,
    pub member: String,
}

/// Full pipeline: emit files, add the workspace member, `cargo check`. On any failure after
/// emit, the crate directory is removed and `Cargo.toml` restored: the smallest reversible step.
pub fn seed(blueprint: &Blueprint, workspace_root: &Path) -> Result<SeedReport, SeedError> {
    let manifest = workspace_root.join("Cargo.toml");
    let original = fs::read_to_string(&manifest)?;
    let patched = add_member(&original, &blueprint.member_path())?;
    let emitted = emit(blueprint, workspace_root)?;
    let checked = fs::write(&manifest, &patched)
        .map_err(SeedError::Io)
        .and_then(|()| cargo_check(workspace_root, &blueprint.name));
    match checked {
        Ok(()) => Ok(SeedReport {
            emit: emitted,
            member: blueprint.member_path(),
        }),
        Err(err) => {
            fs::write(&manifest, &original)?;
            fs::remove_dir_all(&emitted.crate_dir)?;
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests;
