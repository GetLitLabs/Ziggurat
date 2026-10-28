use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A one-crate workspace with no dependencies: builds offline in seconds.
fn tiny_workspace(tag: &str) -> Result<PathBuf, std::io::Error> {
    let root = std::env::temp_dir().join(format!("hcc-polymorph-{tag}-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::create_dir_all(root.join("tiny/src"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"tiny\"]\n",
    )?;
    std::fs::write(
        root.join("tiny/Cargo.toml"),
        "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"rlib\", \"cdylib\"]\n",
    )?;
    std::fs::write(
        root.join("tiny/src/lib.rs"),
        "#![cfg_attr(target_arch = \"wasm32\", no_std)]\n#![forbid(unsafe_code)]\n\npub fn add(a: u32, b: u32) -> u32 {\n    a.wrapping_add(b)\n}\n\n#[cfg(target_arch = \"wasm32\")]\n#[panic_handler]\nfn panic(_: &core::panic::PanicInfo<'_>) -> ! {\n    loop {}\n}\n",
    )?;
    Ok(root)
}

#[test]
fn target_cpus_are_parsed_with_native_resolved() {
    let text = "Available CPUs for this target:\n    native                  - Select the CPU of the current host (currently znver3).\n    alderlake\n    x86-64-v3\n\nAvailable features...\n";
    let (cpus, native) = parse_target_cpus(text);
    assert_eq!(cpus, ["native", "alderlake", "x86-64-v3"]);
    assert_eq!(native.as_deref(), Some("znver3"));
}

#[test]
fn probe_reads_the_local_toolchain() -> TestResult {
    let probe = probe()?;
    assert!(probe.rustc.starts_with("rustc "), "{}", probe.rustc);
    assert!(probe.target_cpus.iter().any(|c| c == "native"));
    assert!(
        probe.installed_targets.contains(&probe.host),
        "the host's own std must be in the sysroot: {:?}",
        probe.installed_targets
    );
    Ok(())
}

#[test]
fn targets_are_checked_against_the_probe_without_fetching() -> TestResult {
    let probe = probe()?;
    let native = Target::Native {
        cpu: "native".into(),
    };
    check_target(&native, &probe)?;
    for cpu in ["no-such-cpu", "../x", ""] {
        assert!(matches!(
            check_target(&Target::Native { cpu: cpu.into() }, &probe),
            Err(PolymorphError::UnknownCpu(_))
        ));
    }
    assert!(matches!(
        check_target(
            &Target::Wasm {
                triple: probe.host.clone()
            },
            &probe
        ),
        Err(PolymorphError::BadTriple(_))
    ));
    assert!(matches!(
        check_target(
            &Target::Wasm {
                triple: "wasm32-not-installed".into()
            },
            &probe
        ),
        Err(PolymorphError::TargetMissing(_))
    ));
    Ok(())
}

#[test]
fn native_emit_builds_hashes_and_separates_cpus() -> TestResult {
    let root = tiny_workspace("native")?;
    let probe = probe()?;
    let generic = "x86-64";
    let cpus: Vec<&str> = if probe.target_cpus.iter().any(|c| c == generic) {
        vec!["native", generic]
    } else {
        vec!["native"]
    };
    for cpu in cpus {
        let report = emit(&root, "tiny", &Target::Native { cpu: cpu.into() }, &probe)?;
        assert_eq!(report.triple, probe.host);
        assert_eq!(report.rustflags, format!("-C target-cpu={cpu}"));
        assert_eq!(report.target_dir, format!("target/polymorph/cpu-{cpu}"));
        let rlib = report
            .artifacts
            .iter()
            .find(|a| a.path.ends_with("libtiny.rlib"))
            .ok_or("no rlib")?;
        let bytes = std::fs::read(root.join(&rlib.path))?;
        assert_eq!(rlib.blake3, *blake3::hash(&bytes).as_bytes());
        assert_eq!(rlib.bytes, bytes.len() as u64);
        if cpu == "native" {
            assert_eq!(report.resolved_cpu, probe.native_cpu);
        }
    }
    assert!(matches!(
        emit(
            &root,
            "not-a-member",
            &Target::Native {
                cpu: "native".into()
            },
            &probe
        ),
        Err(PolymorphError::BadPackage(_))
    ));
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn wasm_emit_builds_offline_when_installed_and_refuses_when_not() -> TestResult {
    let root = tiny_workspace("wasm")?;
    let probe = probe()?;
    let triple = "wasm32-unknown-unknown";
    let wasm = Target::Wasm {
        triple: triple.into(),
    };
    if probe.installed_targets.iter().any(|t| t == triple) {
        let report = emit(&root, "tiny", &wasm, &probe)?;
        assert_eq!(report.triple, triple);
        let module = report
            .artifacts
            .iter()
            .find(|a| a.path.ends_with("tiny.wasm"))
            .ok_or("no .wasm artifact")?;
        let bytes = std::fs::read(root.join(&module.path))?;
        assert_eq!(&bytes[..4], b"\0asm", "a real wasm module");
        assert_eq!(module.blake3, *blake3::hash(&bytes).as_bytes());
    } else {
        assert!(matches!(
            emit(&root, "tiny", &wasm, &probe),
            Err(PolymorphError::TargetMissing(_))
        ));
        assert!(!root.join("target").exists(), "nothing was built");
    }
    std::fs::remove_dir_all(&root)?;
    Ok(())
}
