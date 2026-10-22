//! RamGenie runner wire (M09 `genie.dispatch.local`).
//!
//! One HTTP/1.1 request to the local runner's loopback literal: no DNS, no TLS,
//! no HTTP client crate beyond hyper. Ollama `/api/chat` shape, `stream: false`.

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use tokio::net::TcpStream;

pub const RUNNER_CHAT_PATH: &str = "/api/chat";
/// Local generation can be slow on CPU; still bounded.
pub const RUNNER_TIMEOUT: Duration = Duration::from_secs(300);
pub const RUNNER_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// How much of a runner's error reply is passed back to the operator.
pub const RUNNER_DETAIL_MAX_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerError {
    Unreachable,
    Timeout,
    /// The runner's HTTP status and its own error text (Ollama's `error` field, or the raw
    /// body), trimmed to `RUNNER_DETAIL_MAX_CHARS`.
    Status(u16, String),
    Protocol,
    BadResponse,
}

impl RunnerError {
    /// Fixed wire strings, listed in `SPEC/ERRORS.md`.
    pub const ALL: [RunnerError; 5] = [
        RunnerError::Unreachable,
        RunnerError::Timeout,
        RunnerError::Status(0, String::new()),
        RunnerError::Protocol,
        RunnerError::BadResponse,
    ];

    pub fn wire(&self) -> &'static str {
        match self {
            Self::Unreachable => "runner unreachable on 127.0.0.1:11434",
            Self::Timeout => "runner timed out",
            Self::Status(..) => "runner returned a non-success HTTP status",
            Self::Protocol => "runner HTTP exchange failed",
            Self::BadResponse => "runner response has no message.content",
        }
    }

    /// The `error` value sent to the operator: the fixed wire string, and for a non-2xx reply
    /// the runner's status and what it said, quoted as received (never invented).
    pub fn message(&self) -> String {
        match self {
            Self::Status(code, detail) if detail.is_empty() => {
                format!("{}: HTTP {code}", self.wire())
            }
            Self::Status(code, detail) => format!("{}: HTTP {code}: {detail}", self.wire()),
            _ => self.wire().to_owned(),
        }
    }
}

/// Ollama's `{"error": "..."}` text if present, else the body itself; control characters
/// dropped and cut to `RUNNER_DETAIL_MAX_CHARS`.
fn runner_detail(body: &[u8]) -> String {
    let text = match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => match map.get("error") {
            Some(Value::String(err)) => err.clone(),
            _ => String::from_utf8_lossy(body).into_owned(),
        },
        _ => String::from_utf8_lossy(body).into_owned(),
    };
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let clean = clean.trim();
    if clean.chars().count() > RUNNER_DETAIL_MAX_CHARS {
        let cut: String = clean.chars().take(RUNNER_DETAIL_MAX_CHARS).collect();
        format!("{cut}…")
    } else {
        clean.to_owned()
    }
}

/// Send the sealed charter as the system message and the operator prompt as the user message.
pub async fn generate(
    addr: SocketAddr,
    model: &str,
    system_prompt: &str,
    operator_prompt: &str,
    timeout: Duration,
) -> Result<String, RunnerError> {
    match tokio::time::timeout(
        timeout,
        exchange(addr, model, system_prompt, operator_prompt),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => Err(RunnerError::Timeout),
    }
}

async fn exchange(
    addr: SocketAddr,
    model: &str,
    system_prompt: &str,
    operator_prompt: &str,
) -> Result<String, RunnerError> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|_| RunnerError::Unreachable)?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| RunnerError::Protocol)?;
    let driver = tokio::spawn(conn);

    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": operator_prompt },
        ],
    });
    let request = Request::post(RUNNER_CHAT_PATH)
        .header(HOST, addr.to_string())
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .map_err(|_| RunnerError::Protocol)?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|_| RunnerError::Protocol)?;
    let status = response.status();
    let collected = Limited::new(response.into_body(), RUNNER_MAX_RESPONSE_BYTES)
        .collect()
        .await
        .map_err(|_| RunnerError::Protocol)?
        .to_bytes();
    driver.abort();
    if !status.is_success() {
        return Err(RunnerError::Status(
            status.as_u16(),
            runner_detail(&collected),
        ));
    }
    let parsed: Value = serde_json::from_slice(&collected).map_err(|_| RunnerError::BadResponse)?;
    parsed
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(RunnerError::BadResponse)
}
