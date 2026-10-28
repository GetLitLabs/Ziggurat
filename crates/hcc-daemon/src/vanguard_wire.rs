//! Vanguard through M09 (`POST /v1/ramgenie/vanguard`, `GET /v1/ramgenie/jobs`).
//!
//! The body is the Vanguard JSON handoff `{job, bsmc_namespace, terrace_level, payload}`.
//! Guards match `/v1/ramgenie/dispatch`: loopback peer, royal-sealed charter, open ledgers.
//! Ledger jobs run on a blocking thread and are recorded under the same hold of the vault
//! lock; runner jobs wait on 127.0.0.1:11434 without it, then take it to record. Every job
//! that succeeds is recorded as one BATON trace (prompt = the handoff bytes as received,
//! response = the result bytes returned, which name the prior BATON head) and one
//! `HCC-VANGUARD-v1` stele row, in one transaction. A refused or failed job records nothing.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::Json;
use hcc_seal::{digest_hex, RoyalSealed, SystemCharter};
use hcc_types::TerraceLevel;
use hcc_vanguard::{
    Exec, Handoff, JobStatus, RunnerContext, VanguardContext, VanguardError, VANGUARD_TABLE,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    dispatch_error, refuse, AccordionSwitchboardState, CharterStatus, ErrorBody, LedgerWrite,
    Ledgers, Reply,
};

/// Job refusals that are fixed strings (listed in `SPEC/ERRORS.md`). Everything else a job
/// refuses with is the `VanguardError` text, which names the job and the reason.
pub mod job_error {
    pub const BAD_HANDOFF: &str =
        "job body must be JSON {job, bsmc_namespace, terrace_level, payload}";
    pub const NO_FACTORY: &str = "daemon booted without a vault path; jobs held";

    pub const ALL: [&str; 2] = [BAD_HANDOFF, NO_FACTORY];
}

/// Stele namespace of the row that records each job's BATON trace (M09 dispatch).
pub const JOB_ROW_NAMESPACE: &str = "genie.dispatch.local";

#[derive(Debug, Serialize)]
pub struct JobRow {
    pub name: &'static str,
    pub module: &'static str,
    pub status: &'static str,
    pub exec: &'static str,
}

#[derive(Debug, Serialize)]
pub struct JobTable {
    pub jobs: Vec<JobRow>,
    pub live: usize,
    pub typed: usize,
}

/// `GET /v1/ramgenie/jobs`: the Vanguard table as compiled into this daemon.
pub async fn job_table(ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Reply<JobTable> {
    if !peer.ip().is_loopback() {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            dispatch_error::NON_LOOPBACK.into(),
        ));
    }
    let jobs: Vec<JobRow> = VANGUARD_TABLE
        .iter()
        .map(|job| JobRow {
            name: job.name,
            module: job.module,
            status: match job.status {
                JobStatus::Live => "LIVE",
                JobStatus::Typed => "TYPED",
            },
            exec: job.exec.as_str(),
        })
        .collect();
    let live = jobs.iter().filter(|j| j.status == "LIVE").count();
    Ok(Json(JobTable {
        typed: jobs.len() - live,
        live,
        jobs,
    }))
}

#[derive(Debug, Serialize)]
pub struct JobSealed {
    pub status: &'static str,
    pub job: String,
    pub bsmc_namespace: String,
    pub terrace_level: u8,
    pub charter_blake3_hex: String,
    pub baton_envelope_blake3_hex: String,
    pub baton_trace_id: i64,
    pub stele_job_row_id: i64,
    /// BLAKE3 of the exact result bytes the BATON envelope was computed over.
    pub result_blake3_hex: String,
    pub result: Value,
}

/// HTTP status for a job refusal.
pub fn job_status(err: &VanguardError) -> StatusCode {
    use hcc_neural::NeuralError as N;
    use hcc_zerotrace::ZeroTraceError as Z;
    match err {
        VanguardError::BadHandoff(_)
        | VanguardError::UnknownJob(_)
        | VanguardError::BadNamespace
        | VanguardError::BadTerrace(_)
        | VanguardError::BadPayload(_)
        | VanguardError::Keyring(Z::BadLabel)
        | VanguardError::Neural(
            N::BadNamespace | N::BadTitle | N::BadBody | N::BadModel | N::BadTopK,
        ) => StatusCode::BAD_REQUEST,
        VanguardError::Unimplemented { .. } | VanguardError::Unwired { .. } => {
            StatusCode::NOT_IMPLEMENTED
        }
        VanguardError::NoWorkspace | VanguardError::NoVault | VanguardError::NoRmk => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        VanguardError::Keyring(Z::NotFound(_)) => StatusCode::NOT_FOUND,
        VanguardError::Keyring(Z::Destroyed { .. })
        | VanguardError::Neural(N::Duplicate { .. })
        | VanguardError::Seed(hcc_seed::SeedError::AlreadyExists(_)) => StatusCode::CONFLICT,
        VanguardError::Neural(N::Embed(_) | N::EmbedCount { .. }) => StatusCode::BAD_GATEWAY,
        VanguardError::Healer(_) | VanguardError::Seed(_) => StatusCode::UNPROCESSABLE_ENTITY,
        VanguardError::WrongExec { .. }
        | VanguardError::Stele(_)
        | VanguardError::Keyring(_)
        | VanguardError::Neural(_)
        | VanguardError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn refuse_job(err: &VanguardError) -> (StatusCode, Json<ErrorBody>) {
    refuse(job_status(err), err.to_string())
}

/// `POST /v1/ramgenie/vanguard`. Outcomes are counted in `HealthLog`.
pub async fn vanguard_dispatch(
    State(state): State<Arc<AccordionSwitchboardState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: Result<Bytes, BytesRejection>,
) -> Reply<JobSealed> {
    let outcome = job_sealed(&state, peer, body).await;
    match &outcome {
        Ok(_) => state.health.job_sealed(),
        Err((_, Json(refusal))) => state.health.job_refused(&refusal.error),
    }
    outcome
}

async fn job_sealed(
    state: &Arc<AccordionSwitchboardState>,
    peer: SocketAddr,
    body: Result<Bytes, BytesRejection>,
) -> Reply<JobSealed> {
    use dispatch_error as e;
    if !peer.ip().is_loopback() {
        return Err(refuse(StatusCode::FORBIDDEN, e::NON_LOOPBACK.into()));
    }
    let body = body.map_err(|rejection| match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => refuse(StatusCode::PAYLOAD_TOO_LARGE, e::TOO_LARGE.into()),
        _ => refuse(StatusCode::BAD_REQUEST, job_error::BAD_HANDOFF.into()),
    })?;
    let text = std::str::from_utf8(&body)
        .map_err(|_| refuse(StatusCode::BAD_REQUEST, job_error::BAD_HANDOFF.into()))?;
    let handoff = Handoff::from_json(text)
        .map_err(|_| refuse(StatusCode::BAD_REQUEST, job_error::BAD_HANDOFF.into()))?;
    let checked = hcc_vanguard::check(&handoff).map_err(|err| refuse_job(&err))?;
    let (exec, terrace) = (checked.job.exec, checked.terrace);
    let CharterStatus::Sealed(charter) = &state.charter else {
        return Err(refuse(StatusCode::FORBIDDEN, e::UNSEALED.into()));
    };
    if state.ledgers.is_none() {
        return Err(refuse(StatusCode::SERVICE_UNAVAILABLE, e::NO_LEDGER.into()));
    }
    let Some(vault_path) = state.factory.vault_path.clone() else {
        return Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            job_error::NO_FACTORY.into(),
        ));
    };

    let charter = Arc::clone(charter);
    match exec {
        // The job and its receipt run under one hold of the vault lock, so no other
        // dispatch can land between the job's effects and its BATON trace.
        Exec::Ledger => {
            on_ledgers(state, move |state, ledgers| {
                let result = run_ledger_job(state, ledgers, &charter, &handoff)?;
                record(ledgers, &charter, &handoff, terrace, &body, result)
            })
            .await
        }
        Exec::Runner => {
            let ctx = RunnerContext {
                charter: &charter,
                vault_path: &vault_path,
                runner_addr: state.runner_addr,
                timeout: crate::ramgenie::RUNNER_TIMEOUT,
            };
            let result = hcc_vanguard::dispatch_runner(&handoff, &ctx)
                .await
                .map_err(|err| refuse_job(&err))?;
            on_ledgers(state, move |_, ledgers| {
                record(ledgers, &charter, &handoff, terrace, &body, result)
            })
            .await
        }
        Exec::Daemon => Err(refuse_job(&VanguardError::Unwired {
            job: checked.job.name,
            needs: "a daemon handler",
        })),
    }
}

/// Run `work` on a blocking thread with the vault lock held, so a long job (a cargo
/// build, a gate run) never parks a tokio worker on the mutex.
async fn on_ledgers<T: Send + 'static>(
    state: &Arc<AccordionSwitchboardState>,
    work: impl FnOnce(&AccordionSwitchboardState, &mut Ledgers) -> Result<T, (StatusCode, Json<ErrorBody>)>
        + Send
        + 'static,
) -> Result<T, (StatusCode, Json<ErrorBody>)> {
    use dispatch_error as e;
    let state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        let ledgers = state
            .ledgers
            .as_ref()
            .ok_or_else(|| refuse(StatusCode::SERVICE_UNAVAILABLE, e::NO_LEDGER.into()))?;
        let mut ledgers = ledgers
            .lock()
            .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR, e::LEDGER_LOCK.into()))?;
        work(&state, &mut ledgers)
    })
    .await
    .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR, e::LEDGER_LOCK.into()))?
}

/// Run a ledger job on the locked ledgers.
fn run_ledger_job(
    state: &AccordionSwitchboardState,
    ledgers: &mut Ledgers,
    charter: &SystemCharter<RoyalSealed>,
    handoff: &Handoff,
) -> Result<Value, (StatusCode, Json<ErrorBody>)> {
    let royal = ledgers.royal;
    let factory = &state.factory;
    let mut ctx = VanguardContext {
        charter,
        royal: &royal,
        stele: &mut ledgers.stele,
        workspace_root: factory.workspace_root.as_deref(),
        vault_path: factory.vault_path.as_deref(),
        rmk: factory.rmk.as_ref(),
    };
    hcc_vanguard::dispatch(handoff, &mut ctx).map_err(|err| refuse_job(&err))
}

/// Canonical stele payload for one recorded job.
pub fn job_payload(
    job: &str,
    namespace: &str,
    terrace: TerraceLevel,
    trace_id: i64,
    envelope_hex: &str,
    result_hex: &str,
) -> Vec<u8> {
    format!(
        "HCC-VANGUARD-v1\njob={job}\nbsmc_namespace={namespace}\nterrace_level={}\nbaton_trace_id={trace_id}\nbaton_envelope_hex={envelope_hex}\nresult_blake3_hex={result_hex}\n",
        terrace.as_u8()
    )
    .into_bytes()
}

/// Result key that names the BATON head this job was recorded after (`null` for the
/// first trace). It makes every receipt's response bytes unique, so a deterministic job
/// run twice gets two traces instead of a duplicate-envelope refusal.
pub const PREV_ENVELOPE_KEY: &str = "baton_prev_envelope_hex";

/// BATON trace + `HCC-VANGUARD-v1` stele row for a job that already succeeded. The caller
/// holds the vault lock, so the head read here is still the head when the trace lands.
fn record(
    ledgers: &mut Ledgers,
    charter: &SystemCharter<RoyalSealed>,
    handoff: &Handoff,
    terrace: TerraceLevel,
    handoff_bytes: &[u8],
    mut result: Value,
) -> Reply<JobSealed> {
    use dispatch_error as e;
    let write_failed = |detail: String| {
        // The job's own effects (if any) are already committed; say so in the log.
        eprintln!(
            "hcc-daemon: {} after job {} succeeded: {detail}",
            e::LEDGER_WRITE,
            handoff.job
        );
        refuse(StatusCode::INTERNAL_SERVER_ERROR, e::LEDGER_WRITE.into())
    };
    let prev = ledgers
        .baton
        .snapshot_then(|_, head| Ok::<_, LedgerWrite>(head.map(|(_, env)| digest_hex(&env))))
        .map_err(|LedgerWrite(detail)| write_failed(detail))?;
    let Value::Object(fields) = &mut result else {
        return Err(write_failed("job result is not a JSON object".into()));
    };
    fields.insert(
        PREV_ENVELOPE_KEY.into(),
        prev.map_or(Value::Null, Value::String),
    );
    let result_bytes = serde_json::to_vec(&result)
        .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR, e::LEDGER_WRITE.into()))?;
    let result_hex = digest_hex(blake3::hash(&result_bytes).as_bytes());
    let pubkey = ledgers.operator_pubkey;
    let (trace, stele_id) = ledgers
        .baton
        .record_then(
            charter.digest(),
            &handoff.bsmc_namespace,
            terrace,
            handoff_bytes,
            &result_bytes,
            |tx, trace| {
                let payload = job_payload(
                    &handoff.job,
                    &handoff.bsmc_namespace,
                    terrace,
                    trace.trace_id,
                    &digest_hex(&trace.envelope),
                    &result_hex,
                );
                hcc_iedb_stele::append_in(tx, JOB_ROW_NAMESPACE, &payload, &pubkey, None)
                    .map_err(|err| LedgerWrite(err.to_string()))
            },
        )
        .map_err(|LedgerWrite(detail)| write_failed(detail))?;
    Ok(Json(JobSealed {
        status: "JOB_SEALED",
        job: handoff.job.clone(),
        bsmc_namespace: handoff.bsmc_namespace.clone(),
        terrace_level: terrace.as_u8(),
        charter_blake3_hex: charter.digest_hex(),
        baton_envelope_blake3_hex: digest_hex(&trace.envelope),
        baton_trace_id: trace.trace_id,
        stele_job_row_id: stele_id,
        result_blake3_hex: result_hex,
        result,
    }))
}
