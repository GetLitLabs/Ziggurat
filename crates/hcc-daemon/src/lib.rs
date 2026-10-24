//! Accordion.be (M02) and Lense pillar status (M11 `lense.pillar.status`).
//!
//! Boot reads the charter and checks it against the xtask-baked seal. Only a
//! royal-sealed charter opens the Eduba stele (WAL) and appends or confirms
//! genesis. The daemon then binds 127.0.0.1:7749 whether or not the seal holds:
//! a charter that fails is served honestly as unsealed, and nothing here can
//! forge a sealed state or a genesis row.
#![forbid(unsafe_code)]

pub mod ramgenie;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path as UrlPath, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hcc_baton::BatonLedger;
use hcc_iedb_stele::{Genesis, Stele};
use hcc_seal::{digest_hex, RoyalSeal, RoyalSealed, SealConstants, SealError, SystemCharter};
use hcc_types::{BsmcNamespace, TerraceLevel};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};

pub const ZIGGURAT_VERSION: &str = "1.0.0";
/// Accordion.be bind address. Loopback literal: no DNS.
pub const ACCORDION_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7749));
/// Local model runner (weather). Loopback literal: no DNS.
pub const RUNNER_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 11434));
pub const DEFAULT_CHARTER_PATH: &str = "charters/ZIGGURAT_v1.0.0.txt";
pub const DEFAULT_DB_PATH: &str = "eduba_vault.db";
/// `seal_error` when the charter file cannot be read. The path and io error go to the boot log only.
pub const CHARTER_UNREADABLE: &str = "charter file unreadable";
/// Largest dispatch body accepted. Larger bodies get `dispatch_error::TOO_LARGE` as JSON.
pub const DISPATCH_MAX_BODY_BYTES: usize = 1024 * 1024;
/// Vault lock wait shared by the stele and the BATON ledger.
pub const VAULT_BUSY_TIMEOUT: Duration = hcc_iedb_stele::BUSY_TIMEOUT;

/// The twelve pillars. Status is served only for these ids.
pub const MODULE_IDS: [&str; 12] = [
    "M01-NeuralHighway",
    "M02-Accordion",
    "M03-Krypt",
    "M04-WasiHealer",
    "M05-CodeASeed",
    "M06-Polymorph",
    "M07-Genesis",
    "M08-ZeroTrace",
    "M09-RamGenie",
    "M10-Baton",
    "M11-Lense",
    "M12-Origami",
];

/// What boot learned about the charter. Only `Sealed` can ever hand out a prompt.
pub enum CharterStatus {
    Sealed(Arc<SystemCharter<RoyalSealed>>),
    /// Bytes were read and measured but the seal refused them.
    Unsealed {
        byte_len: usize,
        blake3_hex: String,
        reason: SealError,
    },
    /// The charter file could not be read at all.
    Unreadable {
        reason: String,
    },
}

impl CharterStatus {
    pub fn from_bytes(bytes: Vec<u8>, expected: Option<&SealConstants>) -> Self {
        match SystemCharter::load(bytes).verify(expected) {
            Ok(sealed) => Self::Sealed(Arc::new(sealed)),
            Err(refusal) => Self::Unsealed {
                byte_len: refusal.charter.byte_len(),
                blake3_hex: digest_hex(refusal.charter.digest()),
                reason: refusal.error,
            },
        }
    }

    /// `ziggurat.daemon.boot` step: read the charter and check the baked seal.
    pub fn boot(path: &Path, expected: Option<&SealConstants>) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => Self::from_bytes(bytes, expected),
            Err(err) => Self::Unreadable {
                reason: format!("cannot read {}: {err}", path.display()),
            },
        }
    }

    pub fn is_sealed(&self) -> bool {
        matches!(self, Self::Sealed(_))
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Sealed(c) => format!("SEALED {} bytes blake3 {}", c.byte_len(), c.digest_hex()),
            Self::Unsealed {
                byte_len,
                blake3_hex,
                reason,
            } => {
                format!("UNSEALED {byte_len} bytes blake3 {blake3_hex}: {reason}")
            }
            Self::Unreadable { reason } => format!("UNSEALED: {reason}"),
        }
    }
}

/// The open Eduba vault: stele and BATON ledger in one WAL file.
pub struct Ledgers {
    pub stele: Stele,
    pub baton: BatonLedger,
    /// Operator pubkey recorded on every stele row this daemon writes.
    pub operator_pubkey: [u8; 32],
}

/// Outcome of opening the vault at boot.
pub struct SteleBoot {
    /// Row 1 exists, starts from 32 zero bytes, embeds this charter, and the chain verifies.
    pub genesis: bool,
    /// Present only when genesis holds and both chains verify.
    pub ledgers: Option<Ledgers>,
    /// One line for the boot log.
    pub note: String,
}

impl SteleBoot {
    /// Boot step after the seal: open WAL, append genesis if missing, verify the stele
    /// chain, open the BATON ledger and replay it. An unsealed charter never opens the vault.
    pub fn boot(db_path: &Path, charter: &CharterStatus, royal: Option<&RoyalSeal>) -> Self {
        let (CharterStatus::Sealed(sealed), Some(royal)) = (charter, royal) else {
            return Self {
                genesis: false,
                ledgers: None,
                note: "charter not royal-sealed; stele not opened".to_string(),
            };
        };
        let opened = Stele::open(db_path)
            .map_err(|e| e.to_string())
            .and_then(|mut stele| {
                let genesis = stele
                    .ensure_genesis(sealed, royal)
                    .map_err(|e| e.to_string())?;
                let rows = stele.verify_chain().map_err(|e| e.to_string())?;
                Ok((stele, genesis, rows))
            });
        let (stele, genesis, rows) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                return Self {
                    genesis: false,
                    ledgers: None,
                    note: format!("{}: {err}", db_path.display()),
                }
            }
        };
        let verb = match genesis {
            Genesis::Created(_) => "appended",
            Genesis::Present(_) => "found",
        };
        let stele_note = format!(
            "genesis {verb} in {}; chain verified, {rows} row(s); genesis blake3 {}",
            db_path.display(),
            digest_hex(&genesis.row().blake3_digest)
        );
        let baton = BatonLedger::open(db_path).and_then(|baton| {
            let traces = baton.replay(sealed.digest())?;
            Ok((baton, traces))
        });
        match baton {
            Ok((baton, traces)) => Self {
                genesis: true,
                ledgers: Some(Ledgers {
                    stele,
                    baton,
                    operator_pubkey: royal.operator_pubkey,
                }),
                note: format!("{stele_note}; BATON replayed {traces} trace(s)"),
            },
            Err(err) => Self {
                genesis: true,
                ledgers: None,
                note: format!("{stele_note}; BATON ledger refused: {err}; dispatch held"),
            },
        }
    }
}

/// Shared daemon state (`AccordionSwitchboardState`).
pub struct AccordionSwitchboardState {
    pub charter: CharterStatus,
    pub stele_genesis: bool,
    /// `None` unless the charter is royal-sealed and both chains verified at boot.
    pub ledgers: Option<Mutex<Ledgers>>,
    pub runner_addr: SocketAddr,
    /// In-process counters for `GET /v1/ziggurat/health`. Never leave the machine.
    pub health: HealthLog,
}

impl AccordionSwitchboardState {
    pub fn new(
        charter: CharterStatus,
        stele_genesis: bool,
        ledgers: Option<Ledgers>,
        runner_addr: SocketAddr,
    ) -> Self {
        Self {
            charter,
            stele_genesis,
            ledgers: ledgers.map(Mutex::new),
            runner_addr,
            health: HealthLog::new(),
        }
    }
}

/// `lense.daemon.health` counters: boot instant, dispatch outcomes, the last refusal.
pub struct HealthLog {
    started: Instant,
    dispatches_sealed: AtomicU64,
    dispatches_refused: AtomicU64,
    last_dispatch_error: Mutex<Option<String>>,
}

impl HealthLog {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            dispatches_sealed: AtomicU64::new(0),
            dispatches_refused: AtomicU64::new(0),
            last_dispatch_error: Mutex::new(None),
        }
    }

    fn sealed(&self) {
        self.dispatches_sealed.fetch_add(1, Ordering::Relaxed);
    }

    /// `error` is the same string the operator was sent.
    fn refused(&self, error: &str) {
        self.dispatches_refused.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut last) = self.last_dispatch_error.lock() {
            *last = Some(error.to_owned());
        }
    }
}

impl Default for HealthLog {
    fn default() -> Self {
        Self::new()
    }
}

/// `GET /v1/ziggurat/health`. Counts are read from the vault; nothing is estimated.
#[derive(Debug, Serialize)]
pub struct DaemonHealth {
    pub ziggurat_version: &'static str,
    pub charter_sealed: bool,
    pub stele_genesis: bool,
    pub ledgers_open: bool,
    /// `null` when the ledgers are not open or could not be read.
    pub stele_rows: Option<u64>,
    pub stele_head_blake3_hex: Option<String>,
    pub baton_last_trace_id: Option<i64>,
    pub baton_head_envelope_hex: Option<String>,
    pub runner_up: bool,
    pub uptime_secs: u64,
    pub dispatches_sealed: u64,
    pub dispatches_refused: u64,
    pub last_dispatch_error: Option<String>,
    pub loopback_peer: String,
}

/// Both chain heads as read from the vault.
struct LedgerHeads {
    stele_rows: u64,
    /// Empty only if the stele has no rows.
    stele_head_hex: String,
    /// Last BATON trace id and its envelope hex.
    trace: Option<(i64, String)>,
}

/// Heads of both chains, or `None` on a read failure (logged, never invented).
fn ledger_heads(ledgers: &Mutex<Ledgers>) -> Option<LedgerHeads> {
    let ledgers = ledgers.lock().ok()?;
    let read = || -> Result<_, String> {
        let rows = ledgers.stele.len().map_err(|e| e.to_string())?;
        let head = ledgers
            .stele
            .last()
            .map_err(|e| e.to_string())?
            .map(|row| digest_hex(&row.blake3_digest))
            .unwrap_or_default();
        let trace = ledgers
            .baton
            .last()
            .map_err(|e| e.to_string())?
            .map(|t| (t.trace_id, digest_hex(&t.envelope)));
        Ok(LedgerHeads {
            stele_rows: rows,
            stele_head_hex: head,
            trace,
        })
    };
    match read() {
        Ok(heads) => Some(heads),
        Err(err) => {
            eprintln!("hcc-daemon: health: ledger read failed: {err}");
            None
        }
    }
}

/// `lense.daemon.health`
async fn daemon_health(
    State(state): State<Arc<AccordionSwitchboardState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Reply<DaemonHealth> {
    if !peer.ip().is_loopback() {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            format!("non-loopback peer {peer} refused"),
        ));
    }
    let heads = state.ledgers.as_ref().and_then(ledger_heads);
    let (stele_rows, stele_head, trace) = match heads {
        Some(h) => (
            Some(h.stele_rows),
            (!h.stele_head_hex.is_empty()).then_some(h.stele_head_hex),
            h.trace,
        ),
        None => (None, None, None),
    };
    let health = &state.health;
    Ok(Json(DaemonHealth {
        ziggurat_version: ZIGGURAT_VERSION,
        charter_sealed: state.charter.is_sealed(),
        stele_genesis: state.stele_genesis,
        ledgers_open: state.ledgers.is_some(),
        stele_rows,
        stele_head_blake3_hex: stele_head,
        baton_last_trace_id: trace.as_ref().map(|t| t.0),
        baton_head_envelope_hex: trace.map(|t| t.1),
        runner_up: probe_runner(state.runner_addr).await,
        uptime_secs: health.started.elapsed().as_secs(),
        dispatches_sealed: health.dispatches_sealed.load(Ordering::Relaxed),
        dispatches_refused: health.dispatches_refused.load(Ordering::Relaxed),
        last_dispatch_error: health
            .last_dispatch_error
            .lock()
            .ok()
            .and_then(|last| last.clone()),
        loopback_peer: peer.to_string(),
    }))
}

#[derive(Debug, Serialize)]
pub struct PillarStatus {
    pub module_id: String,
    pub status: &'static str,
    pub ziggurat_version: &'static str,
    pub charter_sealed: bool,
    pub charter_byte_len: Option<usize>,
    pub charter_blake3_hex: Option<String>,
    /// Omitted when sealed. Values are listed in `SPEC/ERRORS.md`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seal_error: Option<String>,
    pub stele_genesis: bool,
    pub loopback_peer: String,
    pub runner_up: bool,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

type Reply<T> = Result<Json<T>, (StatusCode, Json<ErrorBody>)>;

fn refuse(code: StatusCode, error: String) -> (StatusCode, Json<ErrorBody>) {
    (code, Json(ErrorBody { error }))
}

/// Plain TCP connect to the runner's loopback literal. Down is `false`, never an error.
pub async fn probe_runner(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_millis(250), TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

/// `lense.pillar.status`
async fn pillar_status(
    State(state): State<Arc<AccordionSwitchboardState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    UrlPath(module_id): UrlPath<String>,
) -> Reply<PillarStatus> {
    if !peer.ip().is_loopback() {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            format!("non-loopback peer {peer} refused"),
        ));
    }
    if !MODULE_IDS.contains(&module_id.as_str()) {
        return Err(refuse(
            StatusCode::NOT_FOUND,
            format!("unknown module_id {module_id}"),
        ));
    }
    let (status, byte_len, blake3_hex, seal_error) = match &state.charter {
        CharterStatus::Sealed(c) => (
            "ROYAL_SEAL_LOCKED",
            Some(c.byte_len()),
            Some(c.digest_hex()),
            None,
        ),
        CharterStatus::Unsealed {
            byte_len,
            blake3_hex,
            reason,
        } => (
            "UNSEALED",
            Some(*byte_len),
            Some(blake3_hex.clone()),
            Some(reason.to_string()),
        ),
        CharterStatus::Unreadable { .. } => {
            ("UNSEALED", None, None, Some(CHARTER_UNREADABLE.to_string()))
        }
    };
    Ok(Json(PillarStatus {
        module_id,
        status,
        ziggurat_version: ZIGGURAT_VERSION,
        charter_sealed: state.charter.is_sealed(),
        charter_byte_len: byte_len,
        charter_blake3_hex: blake3_hex,
        seal_error,
        stele_genesis: state.stele_genesis,
        loopback_peer: peer.to_string(),
        runner_up: probe_runner(state.runner_addr).await,
    }))
}

/// Dispatch refusals. Fixed strings, listed in `SPEC/ERRORS.md`.
pub mod dispatch_error {
    pub const NON_LOOPBACK: &str = "non-loopback peer refused";
    pub const TOO_LARGE: &str = "dispatch body exceeds 1048576 bytes";
    pub const BAD_JSON: &str =
        "dispatch body must be JSON {model, bsmc_namespace, terrace_level, operator_prompt}";
    pub const BAD_TERRACE: &str = "terrace_level must be 1..=5";
    pub const BAD_NAMESPACE: &str = "bsmc_namespace must be dot-separated";
    pub const BAD_MODEL: &str = "model must be 1-128 chars of [A-Za-z0-9_.:/-]";
    pub const EMPTY_PROMPT: &str = "operator_prompt must not be empty";
    pub const UNSEALED: &str = "charter not royal-sealed; dispatch forbidden";
    pub const NO_LEDGER: &str = "stele or BATON ledger not open; dispatch held";
    pub const LEDGER_LOCK: &str = "ledger lock poisoned";
    pub const LEDGER_WRITE: &str = "ledger append failed";

    pub const ALL: [&str; 11] = [
        NON_LOOPBACK,
        TOO_LARGE,
        BAD_JSON,
        BAD_TERRACE,
        BAD_NAMESPACE,
        BAD_MODEL,
        EMPTY_PROMPT,
        UNSEALED,
        NO_LEDGER,
        LEDGER_LOCK,
        LEDGER_WRITE,
    ];
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchRequest {
    pub model: String,
    pub bsmc_namespace: String,
    pub terrace_level: i64,
    pub operator_prompt: String,
}

#[derive(Debug, Serialize)]
pub struct DispatchSealed {
    pub status: &'static str,
    pub bsmc_namespace: String,
    pub terrace_level: u8,
    pub charter_blake3_hex: String,
    pub baton_envelope_blake3_hex: String,
    pub baton_trace_id: i64,
    pub stele_dispatch_row_id: i64,
    pub model_output: String,
}

fn model_ok(model: &str) -> bool {
    (1..=128).contains(&model.len())
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
}

/// Canonical stele payload for one sealed dispatch.
pub fn dispatch_payload(
    model: &str,
    namespace: &str,
    terrace: TerraceLevel,
    trace_id: i64,
    envelope_hex: &str,
) -> Vec<u8> {
    format!(
        "HCC-DISPATCH-v1\nmodel={model}\nbsmc_namespace={namespace}\nterrace_level={}\nbaton_trace_id={trace_id}\nbaton_envelope_hex={envelope_hex}\n",
        terrace.as_u8()
    )
    .into_bytes()
}

/// `genie.dispatch.local`: the handler records every outcome in `HealthLog`.
async fn ramgenie_dispatch(
    State(state): State<Arc<AccordionSwitchboardState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: Result<Bytes, BytesRejection>,
) -> Reply<DispatchSealed> {
    let outcome = dispatch_sealed(&state, peer, body).await;
    match &outcome {
        Ok(_) => state.health.sealed(),
        Err((_, Json(refusal))) => state.health.refused(&refusal.error),
    }
    outcome
}

async fn dispatch_sealed(
    state: &AccordionSwitchboardState,
    peer: SocketAddr,
    body: Result<Bytes, BytesRejection>,
) -> Reply<DispatchSealed> {
    use dispatch_error as e;
    if !peer.ip().is_loopback() {
        return Err(refuse(StatusCode::FORBIDDEN, e::NON_LOOPBACK.into()));
    }
    // Over the limit is 413 with the documented string; any other body failure is bad JSON.
    let body = body.map_err(|rejection| match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => refuse(StatusCode::PAYLOAD_TOO_LARGE, e::TOO_LARGE.into()),
        _ => refuse(StatusCode::BAD_REQUEST, e::BAD_JSON.into()),
    })?;
    let request: DispatchRequest = serde_json::from_slice(&body)
        .map_err(|_| refuse(StatusCode::BAD_REQUEST, e::BAD_JSON.into()))?;
    let terrace = u8::try_from(request.terrace_level)
        .ok()
        .and_then(|level| TerraceLevel::try_from(level).ok())
        .ok_or_else(|| refuse(StatusCode::BAD_REQUEST, e::BAD_TERRACE.into()))?;
    let namespace = BsmcNamespace::parse(&request.bsmc_namespace)
        .map_err(|_| refuse(StatusCode::BAD_REQUEST, e::BAD_NAMESPACE.into()))?;
    if !model_ok(&request.model) {
        return Err(refuse(StatusCode::BAD_REQUEST, e::BAD_MODEL.into()));
    }
    if request.operator_prompt.is_empty() {
        return Err(refuse(StatusCode::BAD_REQUEST, e::EMPTY_PROMPT.into()));
    }
    let CharterStatus::Sealed(charter) = &state.charter else {
        return Err(refuse(StatusCode::FORBIDDEN, e::UNSEALED.into()));
    };
    let Some(ledgers) = &state.ledgers else {
        return Err(refuse(StatusCode::SERVICE_UNAVAILABLE, e::NO_LEDGER.into()));
    };

    let output = ramgenie::generate(
        state.runner_addr,
        &request.model,
        charter.verified_prompt(),
        &request.operator_prompt,
        ramgenie::RUNNER_TIMEOUT,
    )
    .await
    .map_err(|err| refuse(StatusCode::BAD_GATEWAY, err.message()))?;

    let mut ledgers = ledgers
        .lock()
        .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR, e::LEDGER_LOCK.into()))?;
    let write_failed = |detail: String| {
        eprintln!("hcc-daemon: {}: {detail}", e::LEDGER_WRITE);
        refuse(StatusCode::INTERNAL_SERVER_ERROR, e::LEDGER_WRITE.into())
    };
    // The BATON trace and its stele row commit in one transaction on the vault file:
    // both land or neither does.
    let pubkey = ledgers.operator_pubkey;
    let model = request.model.clone();
    let (trace, stele_id) = ledgers
        .baton
        .record_then(
            charter.digest(),
            namespace.as_str(),
            terrace,
            request.operator_prompt.as_bytes(),
            output.as_bytes(),
            |tx, trace| {
                let payload = dispatch_payload(
                    &model,
                    namespace.as_str(),
                    terrace,
                    trace.trace_id,
                    &digest_hex(&trace.envelope),
                );
                hcc_iedb_stele::append_in(tx, "genie.dispatch.local", &payload, &pubkey, None)
                    .map_err(|err| LedgerWrite(err.to_string()))
            },
        )
        .map_err(|LedgerWrite(detail)| write_failed(detail))?;
    let envelope_hex = digest_hex(&trace.envelope);

    Ok(Json(DispatchSealed {
        status: "DISPATCH_SEALED",
        bsmc_namespace: request.bsmc_namespace,
        terrace_level: terrace.as_u8(),
        charter_blake3_hex: charter.digest_hex(),
        baton_envelope_blake3_hex: envelope_hex,
        baton_trace_id: trace.trace_id,
        stele_dispatch_row_id: stele_id,
        model_output: output,
    }))
}

/// Why a dispatch could not be recorded; logged, never sent (the wire gets `LEDGER_WRITE`).
struct LedgerWrite(String);

impl From<hcc_baton::BatonError> for LedgerWrite {
    fn from(err: hcc_baton::BatonError) -> Self {
        Self(err.to_string())
    }
}

/// Origins of the Tauri webview that hosts the origami cockpit (Linux/macOS, then Windows).
/// Only these get CORS headers; every other origin is served without them, so a browser
/// page elsewhere cannot read the daemon's replies.
pub const COCKPIT_ORIGINS: [&str; 3] = [
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
];

async fn cockpit_cors(request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .filter(|o| COCKPIT_ORIGINS.contains(o))
        .map(str::to_owned);
    let mut response = if request.method() == Method::OPTIONS {
        match origin {
            Some(_) => StatusCode::NO_CONTENT.into_response(),
            None => StatusCode::FORBIDDEN.into_response(),
        }
    } else {
        next.run(request).await
    };
    if let Some(value) = origin.and_then(|o| HeaderValue::from_str(&o).ok()) {
        let headers = response.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type"),
        );
        headers.insert(header::VARY, HeaderValue::from_static("origin"));
    }
    response
}

pub fn router(state: Arc<AccordionSwitchboardState>) -> Router {
    Router::new()
        .route("/v1/ziggurat/pillar/{module_id}/status", get(pillar_status))
        .route("/v1/ziggurat/health", get(daemon_health))
        .route(
            "/v1/ramgenie/dispatch",
            post(ramgenie_dispatch).layer(DefaultBodyLimit::max(DISPATCH_MAX_BODY_BYTES)),
        )
        .layer(middleware::from_fn(cockpit_cors))
        .with_state(state)
}

#[derive(Debug)]
pub enum DaemonError {
    NotLoopback(SocketAddr),
    Io(std::io::Error),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLoopback(addr) => write!(f, "refusing to bind non-loopback address {addr}"),
            Self::Io(err) => write!(f, "io: {err}"),
        }
    }
}

impl std::error::Error for DaemonError {}

/// Bind a loopback address only.
pub async fn bind_loopback(addr: SocketAddr) -> Result<TcpListener, DaemonError> {
    if !addr.ip().is_loopback() {
        return Err(DaemonError::NotLoopback(addr));
    }
    TcpListener::bind(addr).await.map_err(DaemonError::Io)
}

/// Full boot used by both `hcc-daemon` and the Tauri shell: verify the charter, open the
/// stele, bind 127.0.0.1:7749 and serve. Every step is logged with `log`.
pub async fn boot_and_serve(
    charter_path: &Path,
    db_path: &Path,
    log: impl Fn(&str),
) -> Result<(), DaemonError> {
    let charter = CharterStatus::boot(charter_path, hcc_seal::BAKED_SEAL.as_ref());
    log(&format!(
        "charter {}: {}",
        charter_path.display(),
        charter.summary()
    ));
    if !charter.is_sealed() {
        log("serving status only; inference stays refused");
    }
    let royal = hcc_seal::BAKED_SEAL.and_then(|baked| baked.royal);
    let stele = SteleBoot::boot(db_path, &charter, royal.as_ref());
    log(&format!("stele: {}", stele.note));
    let listener = bind_loopback(ACCORDION_ADDR).await?;
    if !probe_runner(RUNNER_ADDR).await {
        log(&format!(
            "warn: no model runner on {RUNNER_ADDR} (weather); continuing"
        ));
    }
    log(&format!("listening on http://{ACCORDION_ADDR}"));
    let state = Arc::new(AccordionSwitchboardState::new(
        charter,
        stele.genesis,
        stele.ledgers,
        RUNNER_ADDR,
    ));
    serve(listener, state).await
}

/// Serve until the process ends.
pub async fn serve(
    listener: TcpListener,
    state: Arc<AccordionSwitchboardState>,
) -> Result<(), DaemonError> {
    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app).await.map_err(DaemonError::Io)
}
