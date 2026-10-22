//! GET /v1/ziggurat/pillar/{module_id}/status. Keys are generated in-test.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use ed25519_dalek::{Signer, SigningKey};
use hcc_daemon::{bind_loopback, router, AccordionSwitchboardState, CharterStatus};
use hcc_seal::{charter_digest, digest_hex, RoyalSeal, SealConstants, BAKED_SEAL};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tower::ServiceExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SAMPLE: &[u8] = b"sample charter\n";

fn loopback_peer() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 40000))
}

/// A loopback port with nothing listening.
async fn dead_port() -> Result<SocketAddr, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    listener.local_addr()
}

fn sealed_constants(bytes: &[u8]) -> SealConstants {
    let key = SigningKey::from_bytes(&blake3::hash(b"in-test key seed").into());
    let digest = charter_digest(bytes);
    SealConstants {
        byte_len: bytes.len(),
        blake3: digest,
        royal: Some(RoyalSeal {
            operator_pubkey: key.verifying_key().to_bytes(),
            signature: key.sign(&digest).to_bytes(),
        }),
    }
}

async fn get(
    charter: CharterStatus,
    runner_addr: SocketAddr,
    peer: SocketAddr,
    module_id: &str,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let state = Arc::new(AccordionSwitchboardState {
        charter,
        runner_addr,
    });
    let request = Request::get(format!("/v1/ziggurat/pillar/{module_id}/status"))
        .extension(ConnectInfo(peer))
        .body(Body::empty())?;
    let response = router(state).oneshot(request).await?;
    let code = response.status();
    let body = to_bytes(response.into_body(), 64 * 1024).await?;
    Ok((code, serde_json::from_slice(&body)?))
}

#[tokio::test]
async fn sealed_charter_reports_computed_digest() -> TestResult {
    let expected = sealed_constants(SAMPLE);
    let charter = CharterStatus::from_bytes(SAMPLE.to_vec(), Some(&expected));
    let (code, body) = get(charter, dead_port().await?, loopback_peer(), "M09-RamGenie").await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["status"], "ROYAL_SEAL_LOCKED");
    assert_eq!(body["charter_sealed"], true);
    assert_eq!(body["charter_byte_len"], SAMPLE.len());
    assert_eq!(
        body["charter_blake3_hex"],
        digest_hex(&charter_digest(SAMPLE))
    );
    assert_eq!(body["seal_error"], Value::Null);
    assert_eq!(body["loopback_peer"], "127.0.0.1:40000");
    assert_eq!(body["ziggurat_version"], "1.0.0");
    assert_eq!(body["runner_up"], false);
    Ok(())
}

#[tokio::test]
async fn real_charter_matches_baked_seal() -> TestResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../charters/ZIGGURAT_v1.0.0.txt");
    let bytes = std::fs::read(&path)?;
    let charter = CharterStatus::boot(&path, BAKED_SEAL.as_ref());
    let (code, body) = get(charter, dead_port().await?, loopback_peer(), "M09-RamGenie").await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["charter_byte_len"], bytes.len());
    assert_eq!(
        body["charter_blake3_hex"],
        digest_hex(&charter_digest(&bytes))
    );
    if let Some(baked) = BAKED_SEAL {
        assert_eq!(body["charter_blake3_hex"], digest_hex(&baked.blake3));
        assert_eq!(body["charter_sealed"], baked.royal.is_some());
    }
    Ok(())
}

#[tokio::test]
async fn unsealed_charter_is_reported_unsealed() -> TestResult {
    let expected = sealed_constants(SAMPLE);
    let charter = CharterStatus::from_bytes(b"tampered charter\n".to_vec(), Some(&expected));
    let (code, body) = get(charter, dead_port().await?, loopback_peer(), "M11-Lense").await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["status"], "UNSEALED");
    assert_eq!(body["charter_sealed"], false);
    assert_eq!(
        body["charter_blake3_hex"],
        digest_hex(&charter_digest(b"tampered charter\n"))
    );
    assert!(body["seal_error"].is_string());
    Ok(())
}

#[tokio::test]
async fn missing_charter_has_no_invented_digest() -> TestResult {
    let charter = CharterStatus::boot(Path::new("/nonexistent/charter.txt"), None);
    let (code, body) = get(
        charter,
        dead_port().await?,
        loopback_peer(),
        "M02-Accordion",
    )
    .await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["charter_sealed"], false);
    assert_eq!(body["charter_byte_len"], Value::Null);
    assert_eq!(body["charter_blake3_hex"], Value::Null);
    assert!(body["seal_error"].is_string());
    Ok(())
}

#[tokio::test]
async fn non_loopback_peer_is_refused() -> TestResult {
    let charter = CharterStatus::from_bytes(SAMPLE.to_vec(), None);
    let peer = SocketAddr::from(([10, 0, 0, 7], 5555));
    let (code, body) = get(charter, dead_port().await?, peer, "M09-RamGenie").await?;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert!(body.get("charter_blake3_hex").is_none());
    Ok(())
}

#[tokio::test]
async fn unknown_module_is_not_found() -> TestResult {
    let charter = CharterStatus::from_bytes(SAMPLE.to_vec(), None);
    let (code, _) = get(charter, dead_port().await?, loopback_peer(), "M13-Ghost").await?;
    assert_eq!(code, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn runner_up_follows_the_socket() -> TestResult {
    let runner = TcpListener::bind("127.0.0.1:0").await?;
    let charter = CharterStatus::from_bytes(SAMPLE.to_vec(), None);
    let (_, body) = get(
        charter,
        runner.local_addr()?,
        loopback_peer(),
        "M09-RamGenie",
    )
    .await?;
    assert_eq!(body["runner_up"], true);
    Ok(())
}

#[tokio::test]
async fn refuses_to_bind_non_loopback() {
    let addr = SocketAddr::from(([0, 0, 0, 0], 0));
    assert!(bind_loopback(addr).await.is_err());
}

#[tokio::test]
async fn serves_over_a_real_loopback_socket() -> TestResult {
    let listener = bind_loopback(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let addr = listener.local_addr()?;
    let state = Arc::new(AccordionSwitchboardState {
        charter: CharterStatus::from_bytes(SAMPLE.to_vec(), None),
        runner_addr: dead_port().await?,
    });
    tokio::spawn(hcc_daemon::serve(listener, state));
    let mut stream = TcpStream::connect(addr).await?;
    stream
        .write_all(b"GET /v1/ziggurat/pillar/M09-RamGenie/status HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await?;
    assert!(raw.starts_with("HTTP/1.1 200"));
    assert!(raw.contains(&digest_hex(&charter_digest(SAMPLE))));
    assert!(raw.contains("\"loopback_peer\":\"127.0.0.1:"));
    Ok(())
}
