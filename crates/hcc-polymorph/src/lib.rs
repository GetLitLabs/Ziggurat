//! Polymorphic Engine (M06): compile-time `target-cpu` and offline wasm.
//!
//! Nothing here downloads anything. [`probe`] asks the local `rustc` what it is, which CPUs
//! it can tune for and which targets have a standard library installed in its sysroot.
//! [`emit`] runs `cargo build --release --offline` for one workspace package, either for the
//! host tuned to one CPU (`RUSTFLAGS=-C target-cpu=<cpu>`) or for an installed `wasm32-*`
//! target, in its own target directory under `target/polymorph/`, and returns the BLAKE3 of
//! every artifact cargo reports. A target that is not installed is refused, never fetched.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Where emitted builds go, relative to the workspace root.
pub const POLYMORPH_DIR: &str = "target/polymorph";
/// The longest CPU or triple name accepted (they become directory names).
pub const NAME_MAX: usize = 64;

#[derive(Debug)]
pub enum PolymorphError {
    /// `rustc` or `cargo` could not be started.
    Spawn(String),
    /// A toolchain query exited non-zero or printed something unparseable.
    Probe(String),
    BadPackage(String),
    UnknownCpu(String),
    BadTriple(String),
    /// The target has no standard library in the sysroot; installing it needs the network.
    TargetMissing(String),
    /// `cargo build` exited non-zero; carries the last lines of its diagnostics.
    Build(String),
    Io(String),
}

impl fmt::Display for PolymorphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(err) => write!(f, "could not start the toolchain: {err}"),
            Self::Probe(err) => write!(f, "toolchain probe failed: {err}"),
            Self::BadPackage(name) => write!(f, "{name} is not a package of this workspace"),
            Self::UnknownCpu(cpu) => write!(
                f,
                "{cpu} is not in `rustc --print target-cpus` for this host"
            ),
            Self::BadTriple(triple) => write!(f, "{triple} is not a wasm32-* target triple"),
            Self::TargetMissing(triple) => write!(
                f,
                "{triple} has no standard library in this toolchain's sysroot; install it with `rustup target add {triple}` (needs the network), then retry"
            ),
            Self::Build(tail) => write!(f, "cargo build failed: {tail}"),
            Self::Io(err) => write!(f, "io: {err}"),
        }
    }
}

impl std::error::Error for PolymorphError {}

impl From<std::io::Error> for PolymorphError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// What the local toolchain reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// First line of `rustc -vV`.
    pub rustc: String,
    pub host: String,
    pub llvm: Option<String>,
    /// What `native` resolves to on this machine, when rustc says.
    pub native_cpu: Option<String>,
    /// Every CPU name `rustc --print target-cpus` lists for the host (including `native`).
    pub target_cpus: Vec<String>,
    /// Targets with a standard library in the sysroot (`<sysroot>/lib/rustlib/<triple>/lib`).
    pub installed_targets: Vec<String>,
}

impl Probe {
    pub fn wasm_targets(&self) -> Vec<&str> {
        self.installed_targets
            .iter()
            .map(String::as_str)
            .filter(|t| t.starts_with("wasm32-"))
            .collect()
    }
}

fn run_text(mut command: Command) -> Result<String, PolymorphError> {
    let program = format!("{:?}", command.get_program());
    let out = command
        .output()
        .map_err(|e| PolymorphError::Spawn(format!("{program}: {e}")))?;
    if !out.status.success() {
        return Err(PolymorphError::Probe(format!(
            "{program} exited with {}",
            out.status
        )));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| PolymorphError::Probe(format!("{program}: not UTF-8")))
}

fn rustc() -> Command {
    Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Parse `rustc --print target-cpus`: the CPU names, and what `native` currently is.
pub fn parse_target_cpus(text: &str) -> (Vec<String>, Option<String>) {
    let mut cpus = Vec::new();
    let mut native = None;
    for line in text
        .lines()
        .skip_while(|l| !l.starts_with("Available CPUs"))
        .skip(1)
    {
        // The list ends at the first blank line; feature lists may follow.
        let Some(name) = line.split_whitespace().next() else {
            break;
        };
        if name == "native" {
            native = line
                .split("(currently ")
                .nth(1)
                .and_then(|rest| rest.split(')').next())
                .map(str::to_owned);
        }
        cpus.push(name.to_owned());
    }
    (cpus, native)
}

/// Ask the local `rustc` (or `$RUSTC`) what it can build.
pub fn probe() -> Result<Probe, PolymorphError> {
    let mut version = rustc();
    version.arg("-vV");
    let version = run_text(version)?;
    let rustc_line = version
        .lines()
        .next()
        .ok_or_else(|| PolymorphError::Probe("rustc -vV printed nothing".into()))?
        .to_owned();
    let field = |key: &str| {
        version
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .map(|v| v.trim().to_owned())
    };
    let host =
        field("host:").ok_or_else(|| PolymorphError::Probe("rustc -vV has no host".into()))?;

    let mut cpus = rustc();
    cpus.args(["--print", "target-cpus"]);
    let (target_cpus, native_cpu) = parse_target_cpus(&run_text(cpus)?);
    if target_cpus.is_empty() {
        return Err(PolymorphError::Probe(
            "rustc --print target-cpus listed no CPUs".into(),
        ));
    }

    let mut sysroot = rustc();
    sysroot.args(["--print", "sysroot"]);
    let rustlib = PathBuf::from(run_text(sysroot)?.trim())
        .join("lib")
        .join("rustlib");
    let mut installed_targets = Vec::new();
    for entry in std::fs::read_dir(&rustlib)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // A target is installed when its directory holds a compiled standard library.
        let lib = entry.path().join("lib");
        let has_std = lib.is_dir()
            && std::fs::read_dir(&lib)?
                .filter_map(Result::ok)
                .any(|f| f.file_name().to_string_lossy().starts_with("libstd-"));
        if has_std {
            installed_targets.push(name);
        }
    }
    installed_targets.sort();
    Ok(Probe {
        rustc: rustc_line,
        host,
        llvm: field("LLVM version:"),
        native_cpu,
        target_cpus,
        installed_targets,
    })
}

/// What to build for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The host triple, tuned for one CPU from `rustc --print target-cpus`.
    Native { cpu: String },
    /// An installed `wasm32-*` target.
    Wasm { triple: String },
}

impl Target {
    /// Directory name under [`POLYMORPH_DIR`].
    pub fn label(&self) -> String {
        match self {
            Self::Native { cpu } => format!("cpu-{cpu}"),
            Self::Wasm { triple } => triple.clone(),
        }
    }
}

fn name_ok(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Check `target` against what the toolchain reported. Nothing is fetched.
pub fn check_target(target: &Target, probe: &Probe) -> Result<(), PolymorphError> {
    match target {
        Target::Native { cpu } => {
            if name_ok(cpu) && probe.target_cpus.iter().any(|c| c == cpu) {
                Ok(())
            } else {
                Err(PolymorphError::UnknownCpu(cpu.clone()))
            }
        }
        Target::Wasm { triple } => {
            if !name_ok(triple) || !triple.starts_with("wasm32-") {
                return Err(PolymorphError::BadTriple(triple.clone()));
            }
            if probe.installed_targets.iter().any(|t| t == triple) {
                Ok(())
            } else {
                Err(PolymorphError::TargetMissing(triple.clone()))
            }
        }
    }
}

/// Workspace members by name, with their manifest paths (`cargo metadata --offline --no-deps`).
pub fn workspace_packages(workspace_root: &Path) -> Result<Vec<(String, PathBuf)>, PolymorphError> {
    let mut command = cargo();
    command.current_dir(workspace_root).args([
        "metadata",
        "--offline",
        "--no-deps",
        "--format-version",
        "1",
    ]);
    let text = run_text(command)?;
    let meta: Value =
        serde_json::from_str(&text).map_err(|e| PolymorphError::Probe(e.to_string()))?;
    let packages = meta["packages"]
        .as_array()
        .ok_or_else(|| PolymorphError::Probe("cargo metadata has no packages".into()))?;
    let mut out = Vec::new();
    for package in packages {
        if let (Some(name), Some(manifest)) =
            (package["name"].as_str(), package["manifest_path"].as_str())
        {
            out.push((name.to_owned(), PathBuf::from(manifest)));
        }
    }
    out.sort();
    Ok(out)
}

/// One file cargo reported for the package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// Path relative to the workspace root, `/`-separated.
    pub path: String,
    pub bytes: u64,
    pub blake3: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitReport {
    pub package: String,
    pub target: Target,
    /// The triple the build ran for (the host for `Native`).
    pub triple: String,
    /// The CPU the build was tuned for, with `native` resolved when rustc says what it is.
    pub resolved_cpu: Option<String>,
    /// The exact `RUSTFLAGS` the build ran with (empty for wasm).
    pub rustflags: String,
    /// Relative to the workspace root.
    pub target_dir: String,
    pub artifacts: Vec<Artifact>,
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Build `package` for `target` with `cargo build --release --offline`, in
/// `<workspace_root>/target/polymorph/<label>`, and hash every artifact cargo reports for it.
pub fn emit(
    workspace_root: &Path,
    package: &str,
    target: &Target,
    probe: &Probe,
) -> Result<EmitReport, PolymorphError> {
    check_target(target, probe)?;
    let root = workspace_root.canonicalize()?;
    let manifest = workspace_packages(&root)?
        .into_iter()
        .find(|(name, _)| name == package)
        .map(|(_, manifest)| manifest)
        .ok_or_else(|| PolymorphError::BadPackage(package.to_owned()))?;
    let manifest = manifest.canonicalize()?;
    let target_dir = root.join(POLYMORPH_DIR).join(target.label());

    let mut command = cargo();
    command.current_dir(&root).args([
        "build",
        "--release",
        "--offline",
        "--message-format=json-render-diagnostics",
        "-p",
        package,
    ]);
    command.arg("--target-dir").arg(&target_dir);
    // Only the flags below apply: inherited ones would make the build depend on the shell.
    command.env_remove("CARGO_ENCODED_RUSTFLAGS");
    command.env_remove("CARGO_BUILD_TARGET");
    let (triple, resolved_cpu, rustflags) = match target {
        Target::Native { cpu } => {
            let flags = format!("-C target-cpu={cpu}");
            let resolved = if cpu == "native" {
                probe.native_cpu.clone()
            } else {
                Some(cpu.clone())
            };
            (probe.host.clone(), resolved, flags)
        }
        Target::Wasm { triple } => {
            command.args(["--target", triple]);
            (triple.clone(), None, String::new())
        }
    };
    command.env("RUSTFLAGS", &rustflags);
    command.env("CARGO_TERM_COLOR", "never");
    let out = command
        .output()
        .map_err(|e| PolymorphError::Spawn(format!("cargo: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(PolymorphError::Build(tail.join(" | ")));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut artifacts = Vec::new();
    for line in stdout.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let ours = message["manifest_path"]
            .as_str()
            .and_then(|p| Path::new(p).canonicalize().ok())
            .is_some_and(|p| p == manifest);
        if !ours {
            continue;
        }
        for file in message["filenames"].as_array().into_iter().flatten() {
            let Some(file) = file.as_str() else { continue };
            let path = Path::new(file);
            let bytes = std::fs::read(path)?;
            let rel = relative(&root, path);
            if artifacts.iter().any(|a: &Artifact| a.path == rel) {
                continue;
            }
            artifacts.push(Artifact {
                path: rel,
                bytes: bytes.len() as u64,
                blake3: *blake3::hash(&bytes).as_bytes(),
            });
        }
    }
    if artifacts.is_empty() {
        return Err(PolymorphError::Build(format!(
            "cargo reported no artifacts for {package}"
        )));
    }
    Ok(EmitReport {
        package: package.to_owned(),
        target: target.clone(),
        triple,
        resolved_cpu,
        rustflags,
        target_dir: relative(&root, &target_dir),
        artifacts,
    })
}

#[cfg(test)]
mod tests;
