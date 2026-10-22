//! Accordion.be (M02) and Lense pillar status (M11 `lense.pillar.status`).
//!
//! Boot reads the charter, checks it against the xtask-baked seal, and binds
//! 127.0.0.1:7749 whether or not the seal holds. A charter that fails the seal
//! is served honestly as unsealed; nothing here can forge a sealed state.
#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Path as UrlPath, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use hcc_seal::{digest_hex, RoyalSealed, SealConstants, SealError, SystemCharter};
use serde::Serialize;
use tokio::net::{TcpListener, TcpStream};

pub const ZIGGURAT_VERSION: &str = "1.0.0";
/// Accordion.be bind address. Loopback literal: no DNS.
pub const ACCORDION_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7749));
/// Local model runner (weather). Loopback literal: no DNS.
pub const RUNNER_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 11434));
pub const DEFAULT_CHARTER_PATH: &str = "charters/ZIGGURAT_v1.0.0.txt";

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

/// Shared daemon state (`AccordionSwitchboardState`).
pub struct AccordionSwitchboardState {
    pub charter: CharterStatus,
    pub runner_addr: SocketAddr,
}

#[derive(Debug, Serialize)]
pub struct PillarStatus {
    pub module_id: String,
    pub status: &'static str,
    pub ziggurat_version: &'static str,
    pub charter_sealed: bool,
    pub charter_byte_len: Option<usize>,
    pub charter_blake3_hex: Option<String>,
    pub seal_error: Option<String>,
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
        CharterStatus::Unreadable { reason } => ("UNSEALED", None, None, Some(reason.clone())),
    };
    Ok(Json(PillarStatus {
        module_id,
        status,
        ziggurat_version: ZIGGURAT_VERSION,
        charter_sealed: state.charter.is_sealed(),
        charter_byte_len: byte_len,
        charter_blake3_hex: blake3_hex,
        seal_error,
        loopback_peer: peer.to_string(),
        runner_up: probe_runner(state.runner_addr).await,
    }))
}

pub fn router(state: Arc<AccordionSwitchboardState>) -> Router {
    Router::new()
        .route("/v1/ziggurat/pillar/{module_id}/status", get(pillar_status))
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

/// Serve until the process ends.
pub async fn serve(
    listener: TcpListener,
    state: Arc<AccordionSwitchboardState>,
) -> Result<(), DaemonError> {
    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app).await.map_err(DaemonError::Io)
}
