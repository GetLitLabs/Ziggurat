use std::path::Path;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn locked_with_fuel(fuel: u64) -> SandboxPolicy {
    SandboxPolicy {
        fuel,
        ..SandboxPolicy::LOCKED
    }
}

#[test]
fn empty_fixture_runs_under_fuel() -> TestResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(EMPTY_FIXTURE_PATH);
    let bytes = std::fs::read(path)?;
    assert_eq!(
        bytes, b"\0asm\x01\0\0\0",
        "fixture must stay the 8-byte empty module"
    );
    let report = diagnose(&bytes, &SandboxPolicy::LOCKED)?;
    assert_eq!(report.wasm_byte_len, 8);
    assert_eq!(report.wasm_blake3, *blake3::hash(&bytes).as_bytes());
    assert!(report.exports.is_empty() && report.memories.is_empty());
    assert!(!report.ran_start_export);
    assert_eq!(report.fuel_cap, SandboxPolicy::LOCKED.fuel);
    assert_eq!(report.fuel_consumed, 0);
    Ok(())
}

#[test]
fn locked_policy_declares_no_host_paths() {
    assert!(SandboxPolicy::LOCKED.declared_paths.is_empty());
    const { assert!(SandboxPolicy::LOCKED.fuel > 0) };
}

#[test]
fn reports_exported_memory_and_charges_fuel() -> TestResult {
    let bytes = wat::parse_str(
        r#"(module
            (memory (export "heap") 2 8)
            (func (export "_start") (drop (memory.grow (i32.const 1)))))"#,
    )?;
    let report = diagnose(&bytes, &SandboxPolicy::LOCKED)?;
    assert!(report.ran_start_export);
    assert_eq!(
        report.memories,
        vec![MemoryReport {
            export: "heap".into(),
            pages: 3,
            bytes: 3 * 65536,
            max_pages: Some(8),
        }]
    );
    assert!(report.fuel_consumed > 0);
    Ok(())
}

#[test]
fn infinite_loop_runs_out_of_fuel() -> TestResult {
    let bytes = wat::parse_str(r#"(module (func (export "_start") (loop (br 0))))"#)?;
    assert_eq!(
        diagnose(&bytes, &locked_with_fuel(10_000)),
        Err(HealerError::OutOfFuel { cap: 10_000 })
    );
    Ok(())
}

#[test]
fn start_section_is_fuel_capped_too() -> TestResult {
    let bytes = wat::parse_str(r#"(module (func $spin (loop (br 0))) (start $spin))"#)?;
    assert_eq!(
        diagnose(&bytes, &locked_with_fuel(10_000)),
        Err(HealerError::OutOfFuel { cap: 10_000 })
    );
    Ok(())
}

#[test]
fn memory_beyond_cap_is_refused() -> TestResult {
    // The 16 MiB cap is 256 pages.
    let grow = wat::parse_str(
        r#"(module (memory 1) (func (export "_start") (drop (memory.grow (i32.const 300)))))"#,
    )?;
    assert!(matches!(
        diagnose(&grow, &SandboxPolicy::LOCKED),
        Err(HealerError::MemoryCap { .. })
    ));
    let initial = wat::parse_str(r#"(module (memory 300))"#)?;
    assert!(matches!(
        diagnose(&initial, &SandboxPolicy::LOCKED),
        Err(HealerError::MemoryCap { .. })
    ));
    Ok(())
}

#[test]
fn any_import_is_denied_including_wasi_fs() -> TestResult {
    let bytes = wat::parse_str(
        r#"(module (import "wasi_snapshot_preview1" "path_open"
            (func (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32))))"#,
    )?;
    assert_eq!(
        diagnose(&bytes, &SandboxPolicy::LOCKED),
        Err(HealerError::ImportDenied {
            module: "wasi_snapshot_preview1".into(),
            name: "path_open".into(),
        })
    );
    Ok(())
}

#[test]
fn text_oversize_and_garbage_are_refused() {
    assert_eq!(
        diagnose(b"(module)", &SandboxPolicy::LOCKED),
        Err(HealerError::NotBinaryWasm)
    );
    let small = SandboxPolicy {
        max_module_bytes: 4,
        ..SandboxPolicy::LOCKED
    };
    assert!(matches!(
        diagnose(b"\0asm\x01\0\0\0", &small),
        Err(HealerError::TooLarge { .. })
    ));
    assert!(matches!(
        diagnose(b"\0asm\x09\0\0\0", &SandboxPolicy::LOCKED),
        Err(HealerError::Invalid(_))
    ));
}

#[test]
fn unreachable_is_a_trap_not_success() -> TestResult {
    let bytes = wat::parse_str(r#"(module (func (export "_start") unreachable))"#)?;
    assert!(matches!(
        diagnose(&bytes, &SandboxPolicy::LOCKED),
        Err(HealerError::Trap(_))
    ));
    Ok(())
}

#[test]
fn unbounded_recursion_traps_instead_of_aborting() -> TestResult {
    let bytes = wat::parse_str(r#"(module (func $f (export "_start") (call $f)))"#)?;
    assert!(matches!(
        diagnose(&bytes, &SandboxPolicy::LOCKED),
        Err(HealerError::Trap(_))
    ));
    Ok(())
}

#[test]
fn zero_fuel_refuses_any_work() -> TestResult {
    let bytes = wat::parse_str(r#"(module (func (export "_start") (nop)))"#)?;
    assert_eq!(
        diagnose(&bytes, &locked_with_fuel(0)),
        Err(HealerError::OutOfFuel { cap: 0 })
    );
    Ok(())
}

#[test]
fn fuel_accounting_is_deterministic_and_per_call() -> TestResult {
    // A bounded loop: same module, same policy, same fuel bill every time. An exhausted
    // run in between must not leak into the next call's store.
    let bounded = wat::parse_str(
        r#"(module (func (export "_start") (local $i i32)
            (loop (local.set $i (i32.add (local.get $i) (i32.const 1)))
                  (br_if 0 (i32.lt_u (local.get $i) (i32.const 1000))))))"#,
    )?;
    let spin = wat::parse_str(r#"(module (func (export "_start") (loop (br 0))))"#)?;
    let first = diagnose(&bounded, &SandboxPolicy::LOCKED)?.fuel_consumed;
    assert!(diagnose(&spin, &locked_with_fuel(5_000)).is_err());
    let second = diagnose(&bounded, &SandboxPolicy::LOCKED)?.fuel_consumed;
    assert!(first > 1000, "loop of 1000 iterations was charged {first}");
    assert_eq!(first, second);
    // Exactly the bill succeeds; one less runs out.
    assert!(diagnose(&bounded, &locked_with_fuel(first)).is_ok());
    assert_eq!(
        diagnose(&bounded, &locked_with_fuel(first - 1)),
        Err(HealerError::OutOfFuel { cap: first - 1 })
    );
    Ok(())
}

#[test]
fn growing_memory_in_a_loop_hits_a_cap_not_the_host() -> TestResult {
    // Grows one page per iteration forever: either the memory cap or fuel stops it.
    let bytes = wat::parse_str(
        r#"(module (memory 1)
            (func (export "_start")
              (loop (drop (memory.grow (i32.const 1))) (br 0))))"#,
    )?;
    match diagnose(&bytes, &SandboxPolicy::LOCKED) {
        Err(HealerError::MemoryCap { .. } | HealerError::OutOfFuel { .. }) => Ok(()),
        other => Err(format!("expected a cap, got {other:?}").into()),
    }
}
