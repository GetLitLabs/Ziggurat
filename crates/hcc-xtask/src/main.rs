//! `cargo run -p hcc-xtask -- <command>`
//!
//! keygen        [--out-dir keys]
//! sign-charter  <charter> --key keys/operator.key [--out keys/charter.sig]
//! seal-charter  <charter> [--pubkey keys/operator.pub --sig keys/charter.sig]
//! freeze-vendor [--undo]          (vendor/ + local .cargo/config.toml, so cargo runs offline)
//! gates         [--only 2,6,10]   (M11 audit, INDEX §14; exits non-zero unless every selected gate passes)
//! seed          <blueprint.json>   (emit crates/<name>, add workspace member, cargo check; rolls back on failure)
//! charter-version <charter> --to-version X.Y.Z --vault <eduba_vault.db>  (M07: move a vault to a newly sealed charter)
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hcc_xtask::{
    hygiene, keygen, read_hex_file, seal, sign_charter, write_new_file, XtaskError, BAKED_RS,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("hcc-xtask: {err}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    hcc_xtask::gates::without_dot_dot(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

fn flag<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>, XtaskError> {
    match args.iter().position(|a| a == name) {
        None => Ok(None),
        Some(i) => args
            .get(i + 1)
            .map(|v| Some(v.as_str()))
            .ok_or_else(|| XtaskError::Usage(format!("{name} needs a value"))),
    }
}

fn positional(args: &[String]) -> Result<&str, XtaskError> {
    args.get(1)
        .filter(|a| !a.starts_with("--"))
        .map(String::as_str)
        .ok_or_else(|| XtaskError::Usage("missing charter path".into()))
}

fn run(args: &[String]) -> Result<(), XtaskError> {
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let out_dir = PathBuf::from(flag(args, "--out-dir")?.unwrap_or("keys"));
            let pubkey = keygen(&out_dir)?;
            println!("secret key   {}  (mode 0600, never commit)", out_dir.join("operator.key").display());
            println!("public key   {}", out_dir.join("operator.pub").display());
            println!("pubkey_hex   {}", hex::encode(pubkey));
            Ok(())
        }
        Some("sign-charter") => {
            let charter = std::fs::read(positional(args)?)?;
            let key_path = flag(args, "--key")?
                .ok_or_else(|| XtaskError::Usage("--key is required".into()))?;
            let out = PathBuf::from(flag(args, "--out")?.unwrap_or("keys/charter.sig"));
            let secret = zeroize::Zeroizing::new(read_hex_file::<32>(Path::new(key_path))?);
            let (pubkey, signature) = sign_charter(&charter, &secret)?;
            write_new_file(&out, format!("{}\n", hex::encode(signature)).as_bytes())?;
            println!("signed       charter_blake3 with operator key {}", hex::encode(pubkey));
            println!("signature    {}", out.display());
            Ok(())
        }
        Some("seal-charter") => {
            let path = positional(args)?;
            let charter = std::fs::read(path)?;
            let royal = match (flag(args, "--pubkey")?, flag(args, "--sig")?) {
                (Some(p), Some(s)) => Some((
                    read_hex_file::<32>(Path::new(p))?,
                    read_hex_file::<64>(Path::new(s))?,
                )),
                (None, None) => None,
                _ => return Err(XtaskError::Usage("--pubkey and --sig go together".into())),
            };
            let report = seal(&charter, royal)?;
            let target = workspace_root().join(BAKED_RS);
            std::fs::write(&target, report.baked_rs.as_bytes())?;
            println!("charter              {path}");
            println!("charter_byte_len     {}", report.byte_len);
            println!("charter_blake3_hex   {}  (BLAKE3(b\"HCC-STELLE-v2\\x00\" || bytes))", report.digest_hex);
            println!("raw_b3sum_hex        {}  (plain b3sum; NOT the seal)", report.raw_b3sum_hex);
            println!(
                "royal_seal           {}",
                if report.royal { "Ed25519 signature verified and baked" } else { "absent (charter stays Unsealed)" }
            );
            println!("wrote                {BAKED_RS}");
            Ok(())
        }
        Some("hygiene") => {
            let path = positional(args)?;
            hygiene(&std::fs::read(path)?)?;
            println!("{path}: UTF-8, LF only, no BOM, single final newline");
            Ok(())
        }
        Some("freeze-vendor") => {
            let root = workspace_root();
            if args.iter().any(|a| a == "--undo") {
                let removed = hcc_xtask::vendor::undo(&root).map_err(XtaskError::Vendor)?;
                println!(
                    "{}",
                    if removed {
                        "removed .cargo/config.toml; cargo uses the registry again (vendor/ left on disk)"
                    } else {
                        "no freeze-vendor config to remove"
                    }
                );
                return Ok(());
            }
            let report = hcc_xtask::vendor::freeze(&root).map_err(XtaskError::Vendor)?;
            println!("vendored             {} crates into vendor/ (Cargo.lock names {})", report.crates, report.lock_packages);
            println!("wrote                .cargo/config.toml (crates-io -> vendor/)");
            println!("next                 cargo test --workspace --offline");
            Ok(())
        }
        Some("gates") => {
            let selection: Vec<u8> = match flag(args, "--only")? {
                None => hcc_xtask::gates::ALL.to_vec(),
                Some(list) => list
                    .split(',')
                    .map(|g| g.trim().parse::<u8>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| XtaskError::Usage("--only takes gate numbers like 2,6,10".into()))?,
            };
            let root = workspace_root();
            let results = hcc_xtask::gates::run(&root, &selection);
            for result in &results {
                println!("{result}");
            }
            let passed = results.iter().filter(|r| r.pass).count();
            println!("GATES {passed}/{} PASS", results.len());
            if passed == results.len() {
                Ok(())
            } else {
                Err(XtaskError::GatesFailed(results.len() - passed))
            }
        }
        Some("seed") => {
            let path = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .ok_or_else(|| XtaskError::Usage("missing blueprint path".into()))?;
            let blueprint = hcc_seed::Blueprint::from_json(&std::fs::read_to_string(path)?)?;
            let report = hcc_seed::seed(&blueprint, &workspace_root())?;
            println!("blueprint            {path}");
            println!("blueprint_blake3     {}", hex::encode(report.emit.blueprint_blake3));
            for (file, hash) in &report.emit.files {
                println!("wrote                {file}  blake3 {}", hex::encode(hash));
            }
            println!("member               {}  (cargo check --offline passed)", report.member);
            Ok(())
        }
        Some("charter-version") => {
            let charter_path = positional(args)?;
            let to_version = flag(args, "--to-version")?
                .ok_or_else(|| XtaskError::Usage("charter-version needs --to-version X.Y.Z".into()))?;
            let vault = flag(args, "--vault")?
                .ok_or_else(|| XtaskError::Usage("charter-version needs --vault <eduba_vault.db>".into()))?;
            let bytes = std::fs::read(charter_path)?;
            let baked = hcc_seal::BAKED_SEAL;
            let royal = baked
                .and_then(|b| b.royal)
                .ok_or(XtaskError::Seal(hcc_seal::SealError::NoRoyalSeal))?;
            let charter = hcc_seal::SystemCharter::load(bytes)
                .verify(baked.as_ref())
                .map_err(|refusal| XtaskError::Seal(refusal.error))?;
            let mut stele = hcc_iedb_stele::Stele::open(Path::new(vault))
                .map_err(|e| XtaskError::Usage(format!("vault: {e}")))?;
            let row = stele
                .record_charter_version(&charter, &royal, to_version)
                .map_err(|e| XtaskError::Usage(format!("charter-version: {e}")))?;
            let rows = stele
                .verify_chain()
                .map_err(|e| XtaskError::Usage(format!("vault: {e}")))?;
            let lineage = stele
                .lineage()
                .map_err(|e| XtaskError::Usage(format!("vault: {e}")))?;
            println!("stele_id             {}", row.stele_id);
            println!("blake3               {}", hex::encode(row.blake3_digest));
            for epoch in &lineage {
                println!(
                    "charter              v{}  {} bytes  {}  (row {})",
                    epoch.version,
                    epoch.byte_len,
                    hex::encode(epoch.digest),
                    epoch.stele_id
                );
            }
            println!("chain                verified, {rows} row(s)");
            Ok(())
        }
        _ => Err(XtaskError::Usage(
            "commands: keygen | sign-charter <charter> --key <file> | seal-charter <charter> [--pubkey <file> --sig <file>] | hygiene <charter> | seed <blueprint.json> | gates [--only N,N] | freeze-vendor [--undo] | charter-version <charter> --to-version X.Y.Z --vault <db>".into(),
        )),
    }
}
