//! Local embeddings (M01): one HTTP/1.1 `POST /api/embed` to the runner on a loopback literal.
//! No DNS, no TLS, no client crate beyond hyper. Ollama shape:
//! `{"model", "input": [..]}` → `{"embeddings": [[f32, ..], ..]}`.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use tokio::net::TcpStream;

use crate::model_ok;

pub const RUNNER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 11434);
pub const RUNNER_EMBED_PATH: &str = "/api/embed";
pub const EMBED_MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub const EMBED_MAX_INPUTS: usize = 256;
const DETAIL_MAX_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedError {
    /// Zero egress: only a loopback address is ever dialled.
    NotLoopback,
    BadModel,
    BadInputs,
    Unreachable,
    Timeout,
    /// The runner's HTTP status and its own error text, quoted as received.
    Status(u16, String),
    Protocol,
    BadResponse,
}

impl fmt::Display for EmbedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLoopback => f.write_str("runner address is not loopback"),
            Self::BadModel => f.write_str("model must be 1..=128 of [A-Za-z0-9._:/-]"),
            Self::BadInputs => write!(f, "embed takes 1..={EMBED_MAX_INPUTS} inputs"),
            Self::Unreachable => f.write_str("runner unreachable"),
            Self::Timeout => f.write_str("runner timed out"),
            Self::Status(code, detail) if detail.is_empty() => {
                write!(f, "runner returned HTTP {code}")
            }
            Self::Status(code, detail) => write!(f, "runner returned HTTP {code}: {detail}"),
            Self::Protocol => f.write_str("runner HTTP exchange failed"),
            Self::BadResponse => f.write_str("runner response has no embeddings array of numbers"),
        }
    }
}

impl std::error::Error for EmbedError {}

fn detail(body: &[u8]) -> String {
    let text = match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => match map.get("error") {
            Some(Value::String(err)) => err.clone(),
            _ => String::from_utf8_lossy(body).into_owned(),
        },
        _ => String::from_utf8_lossy(body).into_owned(),
    };
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(DETAIL_MAX_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Parse `{"embeddings": [[n, ..], ..]}`. Any non-number entry refuses the whole response.
pub fn parse_embeddings(body: &[u8]) -> Result<Vec<Vec<f32>>, EmbedError> {
    let parsed: Value = serde_json::from_slice(body).map_err(|_| EmbedError::BadResponse)?;
    let rows = parsed
        .get("embeddings")
        .and_then(Value::as_array)
        .ok_or(EmbedError::BadResponse)?;
    rows.iter()
        .map(|row| {
            row.as_array()
                .ok_or(EmbedError::BadResponse)?
                .iter()
                .map(|x| {
                    x.as_f64()
                        .map(|v| v as f32)
                        .filter(|v| v.is_finite())
                        .ok_or(EmbedError::BadResponse)
                })
                .collect()
        })
        .collect()
}

/// Embed `inputs` with `model` on the runner at `addr` (must be loopback).
pub async fn embed(
    addr: SocketAddr,
    model: &str,
    inputs: &[&str],
    timeout: Duration,
) -> Result<Vec<Vec<f32>>, EmbedError> {
    if !addr.ip().is_loopback() {
        return Err(EmbedError::NotLoopback);
    }
    if !model_ok(model) {
        return Err(EmbedError::BadModel);
    }
    if inputs.is_empty() || inputs.len() > EMBED_MAX_INPUTS {
        return Err(EmbedError::BadInputs);
    }
    match tokio::time::timeout(timeout, exchange(addr, model, inputs)).await {
        Ok(outcome) => outcome,
        Err(_) => Err(EmbedError::Timeout),
    }
}

async fn exchange(
    addr: SocketAddr,
    model: &str,
    inputs: &[&str],
) -> Result<Vec<Vec<f32>>, EmbedError> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|_| EmbedError::Unreachable)?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| EmbedError::Protocol)?;
    let driver = tokio::spawn(conn);
    let body = json!({ "model": model, "input": inputs });
    let request = Request::post(RUNNER_EMBED_PATH)
        .header(HOST, addr.to_string())
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .map_err(|_| EmbedError::Protocol)?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|_| EmbedError::Protocol)?;
    let status = response.status();
    let collected = Limited::new(response.into_body(), EMBED_MAX_RESPONSE_BYTES)
        .collect()
        .await
        .map_err(|_| EmbedError::Protocol)?
        .to_bytes();
    driver.abort();
    if !status.is_success() {
        return Err(EmbedError::Status(status.as_u16(), detail(&collected)));
    }
    parse_embeddings(&collected)
}
