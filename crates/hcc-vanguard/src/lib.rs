//! Vanguard job table (M05/M09). A capability table, not processes.
//!
//! Every job name comes from `INDEX.md` §4.1 or §11. Three are live in v1.0.0:
//! `scribe.stele.append`, `seed.scaffold.emit`, `inspect.gate.validate`.
//! In this build only `scribe.stele.append` executes; the other two return
//! `Err(Unwired)` until `hcc-seed` and `scripts/nine_gates_audit.sh` exist.
//! Every TYPED job returns `Err(Unimplemented)`. Nothing reports success it did not earn.
#![forbid(unsafe_code)]

use std::fmt;

use hcc_iedb_stele::{Stele, SteleError};
use hcc_seal::{digest_hex, RoyalSeal, RoyalSealed, SystemCharter};
use hcc_types::{BsmcNamespace, TerraceLevel};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// v1.0.0 live job.
    Live,
    /// Named in the constitution; returns `Err(Unimplemented)`.
    Typed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VanguardJob {
    pub name: &'static str,
    pub module: &'static str,
    pub status: JobStatus,
}

const fn live(name: &'static str, module: &'static str) -> VanguardJob {
    VanguardJob {
        name,
        module,
        status: JobStatus::Live,
    }
}

const fn typed(name: &'static str, module: &'static str) -> VanguardJob {
    VanguardJob {
        name,
        module,
        status: JobStatus::Typed,
    }
}

/// The full table. Add a row here and in `INDEX.md` in the same commit.
pub const VANGUARD_TABLE: [VanguardJob; 35] = [
    live("scribe.stele.append", "M03"),
    live("seed.scaffold.emit", "M05"),
    live("inspect.gate.validate", "M11"),
    typed("accordion.fold.expand", "M02"),
    typed("accordion.persist.sqlite", "M02"),
    typed("krypt.stratum.init", "M03"),
    typed("krypt.ke_key.derive", "M03"),
    typed("krypt.de_key.wrap", "M03"),
    typed("krypt.fe_key.encrypt", "M03"),
    typed("krypt.stele.verify", "M03"),
    typed("vault.ast.parse", "M03"),
    typed("vault.store.lesson", "M03"),
    typed("healer.diagnose.heap", "M04"),
    typed("healer.patch.apply", "M04"),
    typed("wasi.sandbox.lock", "M04"),
    typed("seed.manifest.init", "M05"),
    typed("ziggurat.daemon.boot", "M05"),
    typed("polymorph.workspace.init", "M06"),
    typed("polymorph.emit.native", "M06"),
    typed("genesis.charter.version", "M07"),
    typed("chare.provenance.tag", "M07"),
    typed("zerotrace.key.destroy", "M08"),
    typed("krypt.death.keyDestroy", "M08"),
    typed("genie.prompt.seal", "M09"),
    typed("genie.dispatch.local", "M09"),
    typed("baton.trace.record", "M10"),
    typed("baton.state.replay", "M10"),
    typed("konjure.pipeline.init", "M10"),
    typed("curie_monet.verify", "M10"),
    typed("lense.pillar.status", "M11"),
    typed("origami.fold.assemble", "M12"),
    typed("sue.cockpit.mount", "M12"),
    typed("tailor.burst.balance", "M12"),
    typed("neural.retrieve.lesson", "M01"),
    typed("neural.index.upsert", "M01"),
];

pub fn lookup(name: &str) -> Option<&'static VanguardJob> {
    VANGUARD_TABLE.iter().find(|job| job.name == name)
}

/// JSON handoff: `{ "job", "bsmc_namespace", "terrace_level", "payload" }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub job: String,
    pub bsmc_namespace: String,
    pub terrace_level: u8,
    #[serde(default)]
    pub payload: Value,
}

impl Handoff {
    pub fn from_json(text: &str) -> Result<Self, VanguardError> {
        serde_json::from_str(text).map_err(|e| VanguardError::BadHandoff(e.to_string()))
    }
}

/// What a live job may touch. Requires a royal-sealed charter by type.
pub struct VanguardContext<'a> {
    pub charter: &'a SystemCharter<RoyalSealed>,
    pub royal: &'a RoyalSeal,
    pub stele: &'a mut Stele,
}

#[derive(Debug)]
pub enum VanguardError {
    BadHandoff(String),
    UnknownJob(String),
    BadNamespace,
    BadTerrace(u8),
    BadPayload(&'static str),
    Unimplemented {
        job: &'static str,
    },
    Unwired {
        job: &'static str,
        needs: &'static str,
    },
    Stele(SteleError),
}

impl fmt::Display for VanguardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadHandoff(err) => write!(f, "bad handoff JSON: {err}"),
            Self::UnknownJob(job) => write!(f, "unknown job {job}"),
            Self::BadNamespace => f.write_str("bsmc_namespace must be dot-separated"),
            Self::BadTerrace(level) => write!(f, "terrace_level {level} is outside 1..=5"),
            Self::BadPayload(why) => write!(f, "bad payload: {why}"),
            Self::Unimplemented { job } => write!(f, "{job} is TYPED: unimplemented in v1.0.0"),
            Self::Unwired { job, needs } => write!(
                f,
                "{job} is live in the spec but not wired yet: needs {needs}"
            ),
            Self::Stele(err) => write!(f, "stele: {err}"),
        }
    }
}

impl std::error::Error for VanguardError {}

/// Validate a handoff and run its job.
pub fn dispatch(handoff: &Handoff, ctx: &mut VanguardContext<'_>) -> Result<Value, VanguardError> {
    let job = lookup(&handoff.job).ok_or_else(|| VanguardError::UnknownJob(handoff.job.clone()))?;
    let namespace =
        BsmcNamespace::parse(&handoff.bsmc_namespace).map_err(|_| VanguardError::BadNamespace)?;
    let terrace = TerraceLevel::try_from(handoff.terrace_level)
        .map_err(|_| VanguardError::BadTerrace(handoff.terrace_level))?;
    match (job.status, job.name) {
        (JobStatus::Typed, name) => Err(VanguardError::Unimplemented { job: name }),
        (JobStatus::Live, "scribe.stele.append") => {
            scribe_stele_append(&namespace, terrace, &handoff.payload, ctx)
        }
        (JobStatus::Live, "seed.scaffold.emit") => Err(VanguardError::Unwired {
            job: job.name,
            needs: "hcc-seed",
        }),
        (JobStatus::Live, name) => Err(VanguardError::Unwired {
            job: name,
            needs: "scripts/nine_gates_audit.sh",
        }),
    }
}

/// `scribe.stele.append`: payload `{ "text": "<utf-8>" }` becomes one stele row.
fn scribe_stele_append(
    namespace: &BsmcNamespace<'_>,
    terrace: TerraceLevel,
    payload: &Value,
    ctx: &mut VanguardContext<'_>,
) -> Result<Value, VanguardError> {
    let text = payload
        .get("text")
        .and_then(Value::as_str)
        .ok_or(VanguardError::BadPayload(
            "scribe.stele.append needs {\"text\": string}",
        ))?;
    let row = ctx
        .stele
        .append(
            namespace.as_str(),
            text.as_bytes(),
            &ctx.royal.operator_pubkey,
            None,
        )
        .map_err(VanguardError::Stele)?;
    Ok(json!({
        "job": "scribe.stele.append",
        "bsmc_namespace": namespace.as_str(),
        "terrace_level": terrace.as_u8(),
        "charter_blake3_hex": ctx.charter.digest_hex(),
        "stele_id": row.stele_id,
        "prev_digest_hex": digest_hex(&row.prev_digest),
        "blake3_hex": digest_hex(&row.blake3_digest),
    }))
}

#[cfg(test)]
mod tests;
