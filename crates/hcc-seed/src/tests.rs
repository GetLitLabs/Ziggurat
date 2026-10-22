use super::*;

const CAPSNAP: &str = r#"{"name":"capsnap","summary":"Capture snapshots in a WAL SQLite vault","tables":[{"name":"snap","columns":[{"name":"label","type":"TEXT"},{"name":"captured_at_utc","type":"TEXT"},{"name":"payload","type":"BLOB"}]}]}"#;

fn scratch(tag: &str) -> Result<PathBuf, SeedError> {
    let dir = std::env::temp_dir().join(format!("hcc-seed-{tag}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(dir.join("crates"))?;
    Ok(dir)
}

#[test]
fn valid_blueprint_parses() -> Result<(), SeedError> {
    let bp = Blueprint::from_json(CAPSNAP)?;
    assert_eq!(bp.crate_ident(), "capsnap");
    assert_eq!(bp.member_path(), "crates/capsnap");
    Ok(())
}

#[test]
fn hostile_blueprints_are_refused() {
    let cases = [
        r#"{"name":"Caps","summary":"x"}"#,
        r#"{"name":"hcc-evil","summary":"x"}"#,
        r#"{"name":"a/../b","summary":"x"}"#,
        r#"{"name":"ok","summary":"say \"hi\""}"#,
        r#"{"name":"ok","summary":""}"#,
        r#"{"name":"ok","summary":"x","tables":[{"name":"t; DROP TABLE x","columns":[{"name":"a","type":"TEXT"}]}]}"#,
        r#"{"name":"ok","summary":"x","tables":[{"name":"t","columns":[]}]}"#,
        r#"{"name":"ok","summary":"x","tables":[{"name":"t","columns":[{"name":"id","type":"TEXT"}]}]}"#,
        r#"{"name":"ok","summary":"x","tables":[{"name":"t","columns":[{"name":"a","type":"JSON"}]}]}"#,
        r#"{"name":"ok","summary":"x","tables":[{"name":"t","columns":[{"name":"a","type":"TEXT"},{"name":"a","type":"TEXT"}]}]}"#,
        r#"{"name":"ok","summary":"x","extra":1}"#,
    ];
    for case in cases {
        assert!(Blueprint::from_json(case).is_err(), "{case}");
    }
}

#[test]
fn render_is_deterministic_and_digest_is_stable() -> Result<(), SeedError> {
    let bp = Blueprint::from_json(CAPSNAP)?;
    assert_eq!(render(&bp)?, render(&bp)?);
    let reparsed = Blueprint::from_json(
        &serde_json::to_string(&bp).map_err(|e| SeedError::BadJson(e.to_string()))?,
    )?;
    assert_eq!(bp.digest()?, reparsed.digest()?);
    let files = render(&bp)?;
    let lib = files
        .iter()
        .find(|f| f.path == "src/lib.rs")
        .ok_or(SeedError::MembersBlock)?;
    assert!(lib.contents.contains("#![forbid(unsafe_code)]"));
    assert!(lib.contents.contains("PRAGMA journal_mode = WAL"));
    assert!(lib
        .contents
        .contains(&blake3::Hash::from(bp.digest()?).to_hex().to_string()));
    Ok(())
}

#[test]
fn add_member_is_deterministic_and_idempotent() -> Result<(), SeedError> {
    let toml = "[workspace]\nresolver = \"2\"\nmembers = [\n    \"crates/a\",\n]\n\n[x]\n";
    let once = add_member(toml, "crates/b")?;
    assert_eq!(once, "[workspace]\nresolver = \"2\"\nmembers = [\n    \"crates/a\",\n    \"crates/b\",\n]\n\n[x]\n");
    assert_eq!(add_member(&once, "crates/b")?, once);
    assert_eq!(remove_member(&once, "crates/b"), toml);
    assert!(add_member("members = [\"a\"]\n", "crates/b").is_err());
    Ok(())
}

#[test]
fn emit_writes_files_and_never_overwrites() -> Result<(), SeedError> {
    let root = scratch("emit")?;
    let bp = Blueprint::from_json(CAPSNAP)?;
    let report = emit(&bp, &root)?;
    assert_eq!(report.files.len(), 4);
    for (rel, hash) in &report.files {
        let bytes = fs::read(root.join(rel))?;
        assert_eq!(blake3::hash(&bytes).as_bytes(), hash);
    }
    assert!(matches!(emit(&bp, &root), Err(SeedError::AlreadyExists(_))));
    assert!(!root.join("crates/.capsnap.seed-staging").exists());
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn seed_rolls_back_when_cargo_check_fails() -> Result<(), SeedError> {
    // A workspace whose manifest lacks [workspace.package], so the seeded crate cannot build.
    let root = scratch("rollback")?;
    let manifest = "[workspace]\nresolver = \"2\"\nmembers = [\n]\n";
    fs::write(root.join("Cargo.toml"), manifest)?;
    let bp = Blueprint::from_json(r#"{"name":"broken","summary":"x"}"#)?;
    assert!(matches!(seed(&bp, &root), Err(SeedError::CargoCheck(_))));
    assert_eq!(fs::read_to_string(root.join("Cargo.toml"))?, manifest);
    assert!(!root.join("crates/broken").exists());
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn committed_capsnap_matches_its_blueprint() -> Result<(), SeedError> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bp = Blueprint::from_json(&fs::read_to_string(root.join("blueprints/capsnap.json"))?)?;
    for file in render(&bp)? {
        let on_disk = fs::read_to_string(root.join("crates/capsnap").join(file.path))?;
        assert_eq!(
            on_disk, file.contents,
            "crates/capsnap/{} drifted from the blueprint",
            file.path
        );
    }
    Ok(())
}
