//! `cargo run -p hcc-xtask -- <command>`
//!
//! keygen        [--out-dir keys]
//! sign-charter  <charter> --key keys/operator.key [--out keys/charter.sig]
//! seal-charter  <charter> [--pubkey keys/operator.pub --sig keys/charter.sig]
//! gates         [--only 2,6,10]   (M11 audit, INDEX §14; exits non-zero unless every selected gate passes)
//! seed          <blueprint.json>   (emit crates/<name>, add workspace member, cargo check; rolls back on failure)
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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
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
        _ => Err(XtaskError::Usage(
            "commands: keygen | sign-charter <charter> --key <file> | seal-charter <charter> [--pubkey <file> --sig <file>] | hygiene <charter> | seed <blueprint.json> | gates [--only N,N]".into(),
        )),
    }
}
