//! WASI Healer (M04): `wasi.sandbox.lock` and `healer.diagnose.heap`.
//!
//! A module runs inside a locked sandbox: a fuel cap, a linear-memory cap, one instance,
//! and **no host imports at all**. v1.0.0 declares no host paths, so a module that imports
//! anything (WASI filesystem included) is refused before it is instantiated. Diagnosis is
//! read-only; `healer.patch.apply` stays TYPED.
#![forbid(unsafe_code)]

use std::fmt;

use wasmi::errors::{ErrorKind, InstantiationError, MemoryError};
use wasmi::{
    Config, Engine, ExternType, Linker, Module, Store, StoreLimits, StoreLimitsBuilder, TrapCode,
};

/// The committed fixture, an 8-byte empty module.
pub const EMPTY_FIXTURE_PATH: &str = "fixtures/healer_empty.wasm";

const WASM_MAGIC: [u8; 4] = *b"\0asm";

/// The sandbox every module runs in. `LOCKED` is the only policy v1.0.0 uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub fuel: u64,
    pub memory_bytes: usize,
    pub max_module_bytes: usize,
    /// Host paths a module may reach. Empty in v1.0.0: no imports are linked.
    pub declared_paths: &'static [&'static str],
}

impl SandboxPolicy {
    /// `wasi.sandbox.lock`.
    pub const LOCKED: Self = Self {
        fuel: 10_000_000,
        memory_bytes: 16 * 1024 * 1024,
        max_module_bytes: 4 * 1024 * 1024,
        declared_paths: &[],
    };
}

/// One exported linear memory after the start section and `_start` have run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryReport {
    pub export: String,
    pub pages: u64,
    pub bytes: usize,
    pub max_pages: Option<u64>,
}

/// `healer.diagnose.heap` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeapReport {
    pub wasm_byte_len: usize,
    pub wasm_blake3: [u8; 32],
    pub exports: Vec<String>,
    pub memories: Vec<MemoryReport>,
    pub ran_start_export: bool,
    pub fuel_cap: u64,
    pub fuel_consumed: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HealerError {
    NotBinaryWasm,
    TooLarge { byte_len: usize, cap: usize },
    Invalid(String),
    ImportDenied { module: String, name: String },
    OutOfFuel { cap: u64 },
    MemoryCap { cap: usize },
    Trap(String),
}

impl fmt::Display for HealerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBinaryWasm => f.write_str("not a binary wasm module (missing \\0asm magic)"),
            Self::TooLarge { byte_len, cap } => {
                write!(f, "module is {byte_len} bytes; cap is {cap}")
            }
            Self::Invalid(err) => write!(f, "invalid wasm: {err}"),
            Self::ImportDenied { module, name } => write!(
                f,
                "import {module}::{name} denied: the sandbox links no host functions or paths"
            ),
            Self::OutOfFuel { cap } => write!(f, "out of fuel (cap {cap})"),
            Self::MemoryCap { cap } => write!(f, "linear memory cap of {cap} bytes exceeded"),
            Self::Trap(err) => write!(f, "trap: {err}"),
        }
    }
}

impl std::error::Error for HealerError {}

fn classify(err: &wasmi::Error, policy: &SandboxPolicy) -> HealerError {
    let memory = match err.kind() {
        ErrorKind::Memory(e)
        | ErrorKind::Instantiation(InstantiationError::FailedToInstantiateMemory(e)) => Some(e),
        _ => None,
    };
    if err.as_trap_code() == Some(TrapCode::OutOfFuel)
        || matches!(err.kind(), ErrorKind::Fuel(_))
        || matches!(memory, Some(MemoryError::OutOfFuel { .. }))
    {
        return HealerError::OutOfFuel { cap: policy.fuel };
    }
    if err.as_trap_code() == Some(TrapCode::GrowthOperationLimited)
        || matches!(
            memory,
            Some(
                MemoryError::ResourceLimiterDeniedAllocation
                    | MemoryError::OutOfBoundsGrowth
                    | MemoryError::OutOfSystemMemory
            )
        )
    {
        return HealerError::MemoryCap {
            cap: policy.memory_bytes,
        };
    }
    HealerError::Trap(err.to_string())
}

/// `healer.diagnose.heap`: instantiate `wasm` under `policy`, run its start section and an
/// exported `_start: [] -> []` if present, and report every exported memory.
pub fn diagnose(wasm: &[u8], policy: &SandboxPolicy) -> Result<HeapReport, HealerError> {
    if !wasm.starts_with(&WASM_MAGIC) {
        return Err(HealerError::NotBinaryWasm);
    }
    if wasm.len() > policy.max_module_bytes {
        return Err(HealerError::TooLarge {
            byte_len: wasm.len(),
            cap: policy.max_module_bytes,
        });
    }
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, wasm).map_err(|e| HealerError::Invalid(e.to_string()))?;
    if let Some(import) = module.imports().next() {
        return Err(HealerError::ImportDenied {
            module: import.module().to_string(),
            name: import.name().to_string(),
        });
    }
    let limits: StoreLimits = StoreLimitsBuilder::new()
        .memory_size(policy.memory_bytes)
        .instances(1)
        .memories(1)
        .tables(1)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(&engine, limits);
    store.limiter(|limits| limits);
    store
        .set_fuel(policy.fuel)
        .map_err(|e| classify(&e, policy))?;
    let linker: Linker<StoreLimits> = Linker::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| classify(&e, policy))?;

    let ran_start_export = match instance.get_typed_func::<(), ()>(&store, "_start") {
        Ok(start) => {
            start
                .call(&mut store, ())
                .map_err(|e| classify(&e, policy))?;
            true
        }
        Err(_) => false,
    };

    let mut exports = Vec::new();
    let mut memories = Vec::new();
    for export in module.exports() {
        exports.push(export.name().to_string());
        if let ExternType::Memory(_) = export.ty() {
            if let Some(memory) = instance.get_memory(&store, export.name()) {
                memories.push(MemoryReport {
                    export: export.name().to_string(),
                    pages: memory.size(&store),
                    bytes: memory.data_size(&store),
                    max_pages: memory.ty(&store).maximum(),
                });
            }
        }
    }
    let left = store.get_fuel().map_err(|e| classify(&e, policy))?;
    Ok(HeapReport {
        wasm_byte_len: wasm.len(),
        wasm_blake3: *blake3::hash(wasm).as_bytes(),
        exports,
        memories,
        ran_start_export,
        fuel_cap: policy.fuel,
        fuel_consumed: policy.fuel.saturating_sub(left),
    })
}

#[cfg(test)]
mod tests;
