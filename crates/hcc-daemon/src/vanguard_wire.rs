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
    pub const TRACE_EXISTS: &str = "this exact trace is already on the BATON ledger";
    /// Followed by `: <reason>`.
    pub const REPLAY_FAILED: &str = "BATON replay against the charter lineage failed";

    pub const ALL: [&str; 4] = [BAD_HANDOFF, NO_FACTORY, TRACE_EXISTS, REPLAY_FAILED];
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
    /// BLAKE3 of the exact response bytes the BATON envelope was computed over: the
    /// serialized `result`, except for `baton.trace.record`, whose envelope covers the
    /// operator's `output`.
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
    let (job, exec, terrace) = (checked.job, checked.job.exec, checked.terrace);
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
        Exec::Daemon => {
            let name = job.name;
            if name == "baton.trace.record" {
                return baton_trace_record(state, charter, handoff, terrace).await;
            }
            let mut result = match name {
                "genie.dispatch.local" => genie_dispatch_local(state, &charter, &handoff).await?,
                "lense.pillar.status" => {
                    let module_id = payload_str(&handoff, "module_id", PILLAR_PAYLOAD)?;
                    to_value(&crate::pillar_report(state, module_id.to_owned(), peer).await?)?
                }
                "lense.daemon.health" => to_value(&crate::health_report(state, peer).await)?,
                "baton.state.replay" => {
                    // Replay and its receipt share one hold of the vault lock.
                    return on_ledgers(state, move |_, ledgers| {
                        let mut result = baton_state_replay(ledgers)?;
                        stamp(&mut result, &handoff, terrace, &charter);
                        record(ledgers, &charter, &handoff, terrace, &body, result)
                    })
                    .await;
                }
                job => {
                    return Err(refuse_job(&VanguardError::Unwired {
                        job,
                        needs: "a daemon handler",
                    }))
                }
            };
            stamp(&mut result, &handoff, terrace, &charter);
            on_ledgers(state, move |_, ledgers| {
                record(ledgers, &charter, &handoff, terrace, &body, result)
            })
            .await
        }
    }
}

const DISPATCH_PAYLOAD: &str =
    "genie.dispatch.local needs {\"model\": string, \"operator_prompt\": non-empty string}";
const PILLAR_PAYLOAD: &str =
    "lense.pillar.status needs {\"module_id\": string}, one of M01-NeuralHighway..M12-Origami";
const RECORD_PAYLOAD: &str =
    "baton.trace.record needs {\"input\": non-empty string, \"output\": non-empty string}";

fn payload_str<'h>(
    handoff: &'h Handoff,
    key: &str,
    why: &'static str,
) -> Result<&'h str, (StatusCode, Json<ErrorBody>)> {
    handoff
        .payload
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| refuse_job(&VanguardError::BadPayload(why)))
}

fn to_value<T: Serialize>(report: &T) -> Result<Value, (StatusCode, Json<ErrorBody>)> {
    serde_json::to_value(report).map_err(|_| {
        refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            dispatch_error::LEDGER_WRITE.into(),
        )
    })
}

/// Same stamp `hcc_vanguard` puts on ledger and runner results.
fn stamp(
    result: &mut Value,
    handoff: &Handoff,
    terrace: TerraceLevel,
    charter: &SystemCharter<RoyalSealed>,
) {
    if let Value::Object(map) = result {
        map.insert("job".into(), Value::String(handoff.job.clone()));
        map.insert(
            "bsmc_namespace".into(),
            Value::String(handoff.bsmc_namespace.clone()),
        );
        map.insert("terrace_level".into(), terrace.as_u8().into());
        map.insert(
            "charter_blake3_hex".into(),
            Value::String(charter.digest_hex()),
        );
    }
}

/// `genie.dispatch.local` as a job: payload `{model, operator_prompt}`; namespace and terrace
/// come from the handoff. The sealed charter goes to the local runner as the system message,
/// exactly as `POST /v1/ramgenie/dispatch` does. The job's one BATON trace is its receipt.
async fn genie_dispatch_local(
    state: &AccordionSwitchboardState,
    charter: &SystemCharter<RoyalSealed>,
    handoff: &Handoff,
) -> Result<Value, (StatusCode, Json<ErrorBody>)> {
    let model = payload_str(handoff, "model", DISPATCH_PAYLOAD)?;
    let prompt = payload_str(handoff, "operator_prompt", DISPATCH_PAYLOAD)?;
    if !crate::model_ok(model) || prompt.is_empty() {
        return Err(refuse_job(&VanguardError::BadPayload(DISPATCH_PAYLOAD)));
    }
    let output = crate::ramgenie::generate(
        state.runner_addr,
        model,
        charter.verified_prompt(),
        prompt,
        crate::ramgenie::RUNNER_TIMEOUT,
    )
    .await
    .map_err(|err| refuse(StatusCode::BAD_GATEWAY, err.message()))?;
    Ok(serde_json::json!({
        "model": model,
        "operator_prompt_blake3_hex": digest_hex(blake3::hash(prompt.as_bytes()).as_bytes()),
        "model_output_blake3_hex": digest_hex(blake3::hash(output.as_bytes()).as_bytes()),
        "model_output": output,
    }))
}

/// `baton.state.replay`: recompute every BATON envelope against the vault's charter lineage,
/// the same check the daemon runs at boot.
fn baton_state_replay(ledgers: &mut Ledgers) -> Result<Value, (StatusCode, Json<ErrorBody>)> {
    let lineage = ledgers.stele.lineage().map_err(|err| {
        refuse(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{}: {err}", job_error::REPLAY_FAILED),
        )
    })?;
    let digests: Vec<_> = lineage.iter().map(|epoch| epoch.digest).collect();
    let traces = ledgers.baton.replay_lineage(&digests).map_err(|err| {
        refuse(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{}: {err}", job_error::REPLAY_FAILED),
        )
    })?;
    let epochs: Vec<Value> = lineage
        .iter()
        .map(|epoch| {
            serde_json::json!({
                "version": epoch.version,
                "charter_blake3_hex": digest_hex(&epoch.digest),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "traces_replayed": traces,
        "charter_epochs": epochs,
    }))
}

/// Canonical stele payload for an operator-supplied trace (`baton.trace.record`).
pub fn operator_trace_payload(
    namespace: &str,
    terrace: TerraceLevel,
    trace_id: i64,
    envelope_hex: &str,
) -> Vec<u8> {
    format!(
        "HCC-BATON-RECORD-v1\nsource=operator\nbsmc_namespace={namespace}\nterrace_level={}\nbaton_trace_id={trace_id}\nbaton_envelope_hex={envelope_hex}\n",
        terrace.as_u8()
    )
    .into_bytes()
}

/// `baton.trace.record`: payload `{input, output}`, an exchange the operator ran elsewhere.
/// It becomes one BATON trace over exactly those bytes (prompt = input, response = output,
/// namespace and terrace from the handoff) plus one `HCC-BATON-RECORD-v1 source=operator`
/// stele row, in one transaction. That trace is the job's receipt: no second trace is made,
/// so `result_blake3_hex` here is the BLAKE3 of `output`. The same exchange twice is refused.
async fn baton_trace_record(
    state: &Arc<AccordionSwitchboardState>,
    charter: Arc<SystemCharter<RoyalSealed>>,
    handoff: Handoff,
    terrace: TerraceLevel,
) -> Reply<JobSealed> {
    let input = payload_str(&handoff, "input", RECORD_PAYLOAD)?.to_owned();
    let output = payload_str(&handoff, "output", RECORD_PAYLOAD)?.to_owned();
    if input.is_empty() || output.is_empty() {
        return Err(refuse_job(&VanguardError::BadPayload(RECORD_PAYLOAD)));
    }
    on_ledgers(state, move |_, ledgers| {
        use dispatch_error as e;
        let namespace = handoff.bsmc_namespace.as_str();
        let envelope = hcc_baton::baton_envelope(
            charter.digest(),
            &hcc_types::BsmcNamespace::parse(namespace)
                .map_err(|_| refuse_job(&VanguardError::BadNamespace))?,
            terrace,
            blake3::hash(input.as_bytes()).as_bytes(),
            blake3::hash(output.as_bytes()).as_bytes(),
        );
        let write_failed = |detail: String| {
            eprintln!("hcc-daemon: {}: {detail}", e::LEDGER_WRITE);
            refuse(StatusCode::INTERNAL_SERVER_ERROR, e::LEDGER_WRITE.into())
        };
        let seen = ledgers
            .baton
            .snapshot_then(|tx, _| {
                tx.query_row(
                    "SELECT COUNT(*) FROM hcc_baton_trace_ledger WHERE baton_envelope_sig = ?1",
                    [envelope.as_slice()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|err| LedgerWrite(err.to_string()))
            })
            .map_err(|LedgerWrite(detail)| write_failed(detail))?;
        if seen > 0 {
            return Err(refuse(StatusCode::CONFLICT, job_error::TRACE_EXISTS.into()));
        }
        let pubkey = ledgers.operator_pubkey;
        let (trace, stele_id) = ledgers
            .baton
            .record_then(
                charter.digest(),
                namespace,
                terrace,
                input.as_bytes(),
                output.as_bytes(),
                |tx, trace| {
                    let payload = operator_trace_payload(
                        namespace,
                        terrace,
                        trace.trace_id,
                        &digest_hex(&trace.envelope),
                    );
                    hcc_iedb_stele::append_in(tx, "baton.trace.record", &payload, &pubkey, None)
                        .map_err(|err| LedgerWrite(err.to_string()))
                },
            )
            .map_err(|LedgerWrite(detail)| write_failed(detail))?;
        let output_hex = digest_hex(blake3::hash(output.as_bytes()).as_bytes());
        let mut result = serde_json::json!({
            "source": "operator",
            "input_bytes": input.len(),
            "output_bytes": output.len(),
            "input_blake3_hex": digest_hex(blake3::hash(input.as_bytes()).as_bytes()),
            "output_blake3_hex": output_hex,
        });
        stamp(&mut result, &handoff, terrace, &charter);
        Ok(Json(JobSealed {
            status: "JOB_SEALED",
            job: handoff.job.clone(),
            bsmc_namespace: handoff.bsmc_namespace.clone(),
            terrace_level: terrace.as_u8(),
            charter_blake3_hex: charter.digest_hex(),
            baton_envelope_blake3_hex: digest_hex(&trace.envelope),
            baton_trace_id: trace.trace_id,
            stele_job_row_id: stele_id,
            result_blake3_hex: output_hex,
            result,
        }))
    })
    .await
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
