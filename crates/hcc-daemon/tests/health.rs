//! GET /v1/ziggurat/health: chain heads read from the vault, dispatch counters, last refusal.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use ed25519_dalek::{Signer, SigningKey};
use hcc_baton::BatonLedger;
use hcc_daemon::{dispatch_error, router, AccordionSwitchboardState, CharterStatus, SteleBoot};
use hcc_iedb_stele::Stele;
use hcc_seal::{charter_digest, digest_hex, RoyalSeal, SealConstants};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHARTER: &[u8] = b"health test charter\n";
const GOOD: &str = r#"{"model":"m","bsmc_namespace":"seed.scaffold.emit","terrace_level":3,"operator_prompt":"p"}"#;

fn sealed_constants() -> SealConstants {
    let key = SigningKey::from_bytes(blake3::hash(b"health in-test key").as_bytes());
    let digest = charter_digest(CHARTER);
    SealConstants {
        byte_len: CHARTER.len(),
        blake3: digest,
        royal: Some(RoyalSeal {
            operator_pubkey: key.verifying_key().to_bytes(),
            signature: key.sign(&digest).to_bytes(),
        }),
    }
}

fn scratch(name: &str) -> Result<PathBuf, std::io::Error> {
    let dir = std::env::temp_dir().join(format!("hcc-health-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("eduba_vault.db"))
}

fn cleanup(db: &Path) -> Result<(), std::io::Error> {
    match db.parent() {
        Some(dir) => std::fs::remove_dir_all(dir),
        None => Ok(()),
    }
}

async fn fake_runner() -> Result<SocketAddr, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = Router::new().route(
        "/api/chat",
        post(|| async { Json(json!({"message": {"content": "ok"}})) }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(addr)
}

async fn dead_port() -> Result<SocketAddr, std::io::Error> {
    TcpListener::bind("127.0.0.1:0").await?.local_addr()
}

fn sealed_state(db: &Path, runner: SocketAddr) -> AccordionSwitchboardState {
    let expected = sealed_constants();
    let charter = CharterStatus::from_bytes(CHARTER.to_vec(), Some(&expected));
    let boot = SteleBoot::boot(db, &charter, expected.royal.as_ref());
    AccordionSwitchboardState::new(charter, boot.genesis, boot.ledgers, runner)
}

fn loopback() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 50000))
}

async fn call(
    state: &Arc<AccordionSwitchboardState>,
    peer: SocketAddr,
    request: axum::http::request::Builder,
    body: Body,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let request = request.extension(ConnectInfo(peer)).body(body)?;
    let response = router(Arc::clone(state)).oneshot(request).await?;
    let code = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await?;
    Ok((code, serde_json::from_slice(&bytes)?))
}

async fn health(
    state: &Arc<AccordionSwitchboardState>,
    peer: SocketAddr,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    call(
        state,
        peer,
        Request::get("/v1/ziggurat/health"),
        Body::empty(),
    )
    .await
}

async fn dispatch(
    state: &Arc<AccordionSwitchboardState>,
    body: &str,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    call(
        state,
        loopback(),
        Request::post("/v1/ramgenie/dispatch").header("content-type", "application/json"),
        Body::from(body.to_string()),
    )
    .await
}

#[tokio::test]
async fn fresh_sealed_vault_reports_genesis_head_and_no_traces() -> TestResult {
    let db = scratch("fresh")?;
    let state = Arc::new(sealed_state(&db, dead_port().await?));
    let (code, body) = health(&state, loopback()).await?;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["ziggurat_version"], "1.0.0");
    assert_eq!(body["charter_sealed"], true);
    assert_eq!(body["stele_genesis"], true);
    assert_eq!(body["ledgers_open"], true);
    assert_eq!(body["stele_rows"], 1);
    let genesis = Stele::open(&db)?.last()?.ok_or("genesis row")?;
    assert_eq!(
        body["stele_head_blake3_hex"],
        digest_hex(&genesis.blake3_digest)
    );
    assert_eq!(body["baton_last_trace_id"], Value::Null);
    assert_eq!(body["baton_head_envelope_hex"], Value::Null);
    assert_eq!(body["runner_up"], false);
    assert_eq!(body["dispatches_sealed"], 0);
    assert_eq!(body["dispatches_refused"], 0);
    assert_eq!(body["last_dispatch_error"], Value::Null);
    assert_eq!(body["loopback_peer"], "127.0.0.1:50000");
    drop(state);
    cleanup(&db)?;
    Ok(())
}

#[tokio::test]
async fn heads_and_counters_follow_real_dispatches() -> TestResult {
    let db = scratch("counts")?;
    let state = Arc::new(sealed_state(&db, fake_runner().await?));
    let (code, sealed) = dispatch(&state, GOOD).await?;
    assert_eq!(code, StatusCode::OK, "{sealed}");
    let (code, _) = dispatch(&state, "not json").await?;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    let (_, body) = health(&state, loopback()).await?;
    assert_eq!(body["runner_up"], true);
    assert_eq!(body["stele_rows"], 2);
    assert_eq!(body["dispatches_sealed"], 1);
    assert_eq!(body["dispatches_refused"], 1);
    assert_eq!(body["last_dispatch_error"], dispatch_error::BAD_JSON);
    assert_eq!(body["baton_last_trace_id"], sealed["baton_trace_id"]);
    assert_eq!(
        body["baton_head_envelope_hex"],
        sealed["baton_envelope_blake3_hex"]
    );
    let stele_head = Stele::open(&db)?.last()?.ok_or("stele head")?;
    assert_eq!(
        body["stele_head_blake3_hex"],
        digest_hex(&stele_head.blake3_digest)
    );
    let trace = BatonLedger::open(&db)?.last()?.ok_or("trace")?;
    assert_eq!(body["baton_head_envelope_hex"], digest_hex(&trace.envelope));
    drop(trace);
    drop(state);
    cleanup(&db)?;
    Ok(())
}

#[tokio::test]
async fn unsealed_charter_reports_no_ledgers_and_counts_refusals() -> TestResult {
    let state = Arc::new(AccordionSwitchboardState::new(
        CharterStatus::from_bytes(CHARTER.to_vec(), None),
        false,
        None,
        dead_port().await?,
    ));
    let (code, _) = dispatch(&state, GOOD).await?;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let (code, body) = health(&state, loopback()).await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["charter_sealed"], false);
    assert_eq!(body["ledgers_open"], false);
    assert_eq!(body["stele_rows"], Value::Null, "no invented counts");
    assert_eq!(body["stele_head_blake3_hex"], Value::Null);
    assert_eq!(body["dispatches_refused"], 1);
    assert_eq!(body["last_dispatch_error"], dispatch_error::UNSEALED);
    Ok(())
}

#[tokio::test]
async fn health_refuses_non_loopback_peers() -> TestResult {
    let state = Arc::new(AccordionSwitchboardState::new(
        CharterStatus::from_bytes(CHARTER.to_vec(), None),
        false,
        None,
        dead_port().await?,
    ));
    for peer in ["10.0.0.5:1", "[fe80::1]:1", "[::ffff:127.0.0.1]:1"] {
        let (code, _) = health(&state, peer.parse()?).await?;
        assert_eq!(code, StatusCode::FORBIDDEN, "{peer}");
    }
    Ok(())
}

#[test]
fn index_documents_the_health_route() -> Result<(), std::io::Error> {
    let index =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../INDEX.md"))?;
    assert!(index.contains("`/v1/ziggurat/health`"));
    assert!(index.contains("`lense.daemon.health`"));
    Ok(())
}

/// A commit from another connection between the health reads is not seen: the BATON head
/// and the stele count and head all come from the snapshot the read transaction started on.
#[tokio::test]
async fn heads_come_from_one_snapshot_despite_a_concurrent_writer() -> TestResult {
    let db = scratch("snapshot")?;
    drop(sealed_state(&db, dead_port().await?));
    let genesis = Stele::open(&db)?.last()?.ok_or("genesis row")?;
    let mut reader = BatonLedger::open(&db)?;
    let (before, after) = reader.snapshot_then(|tx, trace| {
        assert!(trace.is_none());
        let before = hcc_iedb_stele::head_in(tx)?;
        let mut writer = Stele::open(&db)?;
        writer.append("test.concurrent.writer", b"x", &[7u8; 32], None)?;
        drop(writer);
        let after = hcc_iedb_stele::head_in(tx)?;
        Ok::<_, Box<dyn std::error::Error>>((before, after))
    })?;
    assert_eq!(before, (1, Some(genesis.blake3_digest)));
    assert_eq!(after, before, "the commit landed outside this snapshot");
    drop(reader);
    assert_eq!(Stele::open(&db)?.len()?, 2, "the writer did commit");
    cleanup(&db)?;
    Ok(())
}
