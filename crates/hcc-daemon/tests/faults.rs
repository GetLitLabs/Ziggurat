//! Fault injection: every fallback path must answer with a documented error, append nothing,
//! and never panic. Runner down or hung, corrupt or locked vault, bad seal, bad charter,
//! malformed dispatch, non-loopback peers. Keys are generated in-test.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use ed25519_dalek::{Signer, SigningKey};
use hcc_daemon::ramgenie::{self, RunnerError};
use hcc_daemon::{
    bind_loopback, dispatch_error, router, AccordionSwitchboardState, CharterStatus, DaemonError,
    SteleBoot, VAULT_BUSY_TIMEOUT,
};
use hcc_iedb_stele::Stele;
use hcc_seal::{charter_digest, RoyalSeal, SealConstants};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tower::ServiceExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHARTER: &[u8] = b"fault injection charter\n";
const GOOD: &str = r#"{"model":"ziggurat:1.0.0","bsmc_namespace":"seed.scaffold.emit","terrace_level":3,"operator_prompt":"Scaffold crate capsnap."}"#;

fn key(seed: &[u8]) -> SigningKey {
    SigningKey::from_bytes(blake3::hash(seed).as_bytes())
}

fn sealed_constants() -> SealConstants {
    let key = key(b"faults in-test key");
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

/// A fresh directory per test; the vault path inside it is not created.
fn scratch(name: &str) -> Result<PathBuf, std::io::Error> {
    let dir = std::env::temp_dir().join(format!("hcc-faults-{name}-{}", std::process::id()));
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

async fn fake_runner(content: &'static str) -> Result<SocketAddr, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = Router::new().route(
        "/api/chat",
        post(move || async move { Json(json!({"message": {"content": content}})) }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(addr)
}

/// A port with nothing listening.
async fn dead_port() -> Result<SocketAddr, std::io::Error> {
    TcpListener::bind("127.0.0.1:0").await?.local_addr()
}

/// A raw TCP runner that reads the request and then does `act` with the socket.
async fn raw_runner<F, Fut>(act: F) -> Result<SocketAddr, std::io::Error>
where
    F: Fn(tokio::net::TcpStream) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let act = Arc::new(act);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let act = Arc::clone(&act);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                act(socket).await;
            });
        }
    });
    Ok(addr)
}

fn sealed_state(db: &Path, runner_addr: SocketAddr) -> AccordionSwitchboardState {
    let expected = sealed_constants();
    let charter = CharterStatus::from_bytes(CHARTER.to_vec(), Some(&expected));
    let boot = SteleBoot::boot(db, &charter, expected.royal.as_ref());
    AccordionSwitchboardState {
        charter,
        stele_genesis: boot.genesis,
        ledgers: boot.ledgers.map(Mutex::new),
        runner_addr,
    }
}

async fn send(
    state: Arc<AccordionSwitchboardState>,
    request: Request<Body>,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let response = router(state).oneshot(request).await?;
    let code = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await?;
    let body = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{code}: reply is not JSON ({e}): {bytes:?}"))?;
    Ok((code, body))
}

fn dispatch_request(peer: SocketAddr, body: Body) -> Result<Request<Body>, axum::http::Error> {
    Request::post("/v1/ramgenie/dispatch")
        .header("content-type", "application/json")
        .extension(ConnectInfo(peer))
        .body(body)
}

async fn dispatch(
    state: AccordionSwitchboardState,
    peer: SocketAddr,
    body: &str,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    send(
        Arc::new(state),
        dispatch_request(peer, Body::from(body.to_string()))?,
    )
    .await
}

async fn status(
    state: AccordionSwitchboardState,
    peer: SocketAddr,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let request = Request::get("/v1/ziggurat/pillar/M02-Accordion/status")
        .extension(ConnectInfo(peer))
        .body(Body::empty())?;
    send(Arc::new(state), request).await
}

fn loopback() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 50000))
}

// ---------------------------------------------------------------- runner faults

#[tokio::test]
async fn runner_that_never_answers_times_out() -> TestResult {
    let addr = raw_runner(|socket| async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(socket);
    })
    .await?;
    let started = std::time::Instant::now();
    let outcome = ramgenie::generate(addr, "m", "sys", "p", Duration::from_millis(200)).await;
    assert_eq!(outcome, Err(RunnerError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
async fn runner_that_hangs_up_mid_request_is_a_protocol_error() -> TestResult {
    let addr = raw_runner(|socket| async move { drop(socket) }).await?;
    let outcome = ramgenie::generate(addr, "m", "sys", "p", Duration::from_secs(5)).await;
    assert_eq!(outcome, Err(RunnerError::Protocol));
    Ok(())
}

#[tokio::test]
async fn runner_that_sends_garbage_is_a_protocol_error() -> TestResult {
    let addr = raw_runner(|mut socket| async move {
        let _ = socket.write_all(b"SSH-2.0-not-http\r\n\r\n").await;
    })
    .await?;
    let outcome = ramgenie::generate(addr, "m", "sys", "p", Duration::from_secs(5)).await;
    assert_eq!(outcome, Err(RunnerError::Protocol));
    Ok(())
}

#[tokio::test]
async fn runner_reply_over_the_cap_is_refused() -> TestResult {
    let addr = raw_runner(|mut socket| async move {
        let len = ramgenie::RUNNER_MAX_RESPONSE_BYTES + 1;
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {len}\r\n\r\n"
        );
        let _ = socket.write_all(head.as_bytes()).await;
        let chunk = vec![b' '; 64 * 1024];
        let mut sent = 0;
        while sent < len {
            let n = chunk.len().min(len - sent);
            if socket.write_all(&chunk[..n]).await.is_err() {
                return;
            }
            sent += n;
        }
    })
    .await?;
    let outcome = ramgenie::generate(addr, "m", "sys", "p", Duration::from_secs(10)).await;
    assert_eq!(outcome, Err(RunnerError::Protocol));
    Ok(())
}

#[tokio::test]
async fn runner_success_without_content_is_bad_response() -> TestResult {
    let addr = raw_runner(|mut socket| async move {
        let body = r#"{"message":{"role":"assistant"}}"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(reply.as_bytes()).await;
    })
    .await?;
    let outcome = ramgenie::generate(addr, "m", "sys", "p", Duration::from_secs(5)).await;
    assert_eq!(outcome, Err(RunnerError::BadResponse));
    Ok(())
}

#[tokio::test]
async fn runner_down_through_the_route_appends_nothing() -> TestResult {
    let db = scratch("down")?;
    let (code, body) = dispatch(sealed_state(&db, dead_port().await?), loopback(), GOOD).await?;
    assert_eq!(code, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"], RunnerError::Unreachable.wire());
    assert_eq!(Stele::open(&db)?.len()?, 1, "genesis only");
    cleanup(&db)?;
    Ok(())
}

// ---------------------------------------------------------------- vault faults

#[tokio::test]
async fn corrupt_vault_holds_dispatch_and_reports_no_genesis() -> TestResult {
    let db = scratch("corrupt")?;
    std::fs::write(&db, vec![0xA5u8; 8192])?;
    let state = sealed_state(&db, fake_runner("x").await?);
    assert!(!state.stele_genesis);
    assert!(state.ledgers.is_none());
    let (code, body) = dispatch(state, loopback(), GOOD).await?;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], dispatch_error::NO_LEDGER);

    let (code, body) = status(sealed_state(&db, dead_port().await?), loopback()).await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["charter_sealed"], true, "seal is still honest");
    assert_eq!(body["stele_genesis"], false);
    assert_eq!(
        std::fs::read(&db)?,
        vec![0xA5u8; 8192],
        "corrupt file untouched"
    );
    cleanup(&db)?;
    Ok(())
}

#[tokio::test]
async fn truncated_vault_holds_dispatch() -> TestResult {
    let db = scratch("truncated")?;
    drop(sealed_state(&db, dead_port().await?));
    let bytes = std::fs::read(&db)?;
    let _ = std::fs::remove_file(db.with_extension("db-wal"));
    let _ = std::fs::remove_file(db.with_extension("db-shm"));
    std::fs::write(&db, &bytes[..bytes.len().min(100)])?;
    let state = sealed_state(&db, fake_runner("x").await?);
    assert!(state.ledgers.is_none());
    let (code, body) = dispatch(state, loopback(), GOOD).await?;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], dispatch_error::NO_LEDGER);
    cleanup(&db)?;
    Ok(())
}

#[tokio::test]
async fn vault_path_that_is_a_directory_holds_dispatch() -> TestResult {
    let db = scratch("dir")?;
    std::fs::create_dir_all(&db)?;
    let boot = sealed_state(&db, dead_port().await?);
    assert!(!boot.stele_genesis && boot.ledgers.is_none());
    cleanup(&db)?;
    Ok(())
}

/// Another process (a backup tool, a second daemon) holds the vault's write lock briefly.
/// The dispatch waits for it instead of failing after the model has already run.
#[tokio::test]
async fn briefly_locked_vault_waits_and_records() -> TestResult {
    let db = scratch("locked-brief")?;
    let state = sealed_state(&db, fake_runner("ok").await?);
    let holder = rusqlite::Connection::open(&db)?;
    holder.execute_batch("BEGIN IMMEDIATE")?;
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        holder.execute_batch("COMMIT")
    });
    let (code, body) = dispatch(state, loopback(), GOOD).await?;
    release
        .join()
        .map_err(|_| "lock holder thread panicked")??;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "DISPATCH_SEALED");
    assert_eq!(Stele::open(&db)?.len()?, 2);
    cleanup(&db)?;
    Ok(())
}

/// A lock that outlives the busy timeout fails with the documented 500, and both the BATON
/// trace and its stele row are absent.
#[tokio::test]
async fn vault_locked_past_the_timeout_appends_nothing() -> TestResult {
    let db = scratch("locked-long")?;
    let state = sealed_state(&db, fake_runner("ok").await?);
    let holder = rusqlite::Connection::open(&db)?;
    holder.execute_batch("BEGIN IMMEDIATE")?;
    let (code, body) = dispatch(state, loopback(), GOOD).await?;
    holder.execute_batch("ROLLBACK")?;
    drop(holder);
    assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], dispatch_error::LEDGER_WRITE);
    assert_eq!(Stele::open(&db)?.len()?, 1, "genesis only");
    let baton = hcc_baton::BatonLedger::open(&db)?;
    assert!(baton.last()?.is_none(), "no BATON trace");
    cleanup(&db)?;
    Ok(())
}

#[test]
fn busy_timeout_is_bounded_and_shared() {
    assert_eq!(VAULT_BUSY_TIMEOUT, hcc_iedb_stele::BUSY_TIMEOUT);
    assert_eq!(VAULT_BUSY_TIMEOUT, hcc_baton::BUSY_TIMEOUT);
    assert!(VAULT_BUSY_TIMEOUT >= Duration::from_secs(1));
    assert!(VAULT_BUSY_TIMEOUT <= Duration::from_secs(10));
}

// ---------------------------------------------------------------- seal and charter faults

fn constants_with(charter: &[u8], royal: Option<RoyalSeal>) -> SealConstants {
    SealConstants {
        byte_len: charter.len(),
        blake3: charter_digest(charter),
        royal,
    }
}

/// Each bad seal or charter boots UNSEALED with its documented reason, never opens (or
/// creates) the vault, and refuses dispatch with 403.
#[tokio::test]
async fn bad_seals_and_charters_stay_unsealed_and_never_open_the_vault() -> TestResult {
    let good = sealed_constants();
    let other = key(b"some other operator");
    let digest = charter_digest(CHARTER);
    let crlf = CHARTER
        .strip_suffix(b"\n")
        .map(|c| [c, b"\r\n"].concat())
        .ok_or("charter ends in LF")?;
    let bom = [b"\xEF\xBB\xBF".as_slice(), CHARTER].concat();
    let mut flipped = CHARTER.to_vec();
    flipped[0] ^= 0x20;
    let mut bad_sig = good.royal.ok_or("royal")?;
    bad_sig.signature[0] ^= 1;

    let cases: Vec<(&str, Vec<u8>, Option<SealConstants>, &str)> = vec![
        (
            "not baked",
            CHARTER.to_vec(),
            None,
            "baked seal constants absent",
        ),
        (
            "CRLF",
            crlf,
            Some(good),
            "charter byte length does not match baked seal",
        ),
        (
            "BOM",
            bom,
            Some(good),
            "charter byte length does not match baked seal",
        ),
        (
            "one byte flipped",
            flipped,
            Some(good),
            "charter digest does not match baked seal",
        ),
        (
            "no royal seal",
            CHARTER.to_vec(),
            Some(constants_with(CHARTER, None)),
            "no operator Ed25519 royal seal baked",
        ),
        (
            "signature by another key",
            CHARTER.to_vec(),
            Some(constants_with(
                CHARTER,
                Some(RoyalSeal {
                    operator_pubkey: good.royal.ok_or("royal")?.operator_pubkey,
                    signature: other.sign(&digest).to_bytes(),
                }),
            )),
            "operator Ed25519 signature does not verify",
        ),
        (
            "flipped signature bit",
            CHARTER.to_vec(),
            Some(constants_with(CHARTER, Some(bad_sig))),
            "operator Ed25519 signature does not verify",
        ),
        (
            "not UTF-8",
            b"\xFF\xFE charter\n".to_vec(),
            Some(constants_with(
                b"\xFF\xFE charter\n",
                Some(RoyalSeal {
                    operator_pubkey: other.verifying_key().to_bytes(),
                    signature: other
                        .sign(&charter_digest(b"\xFF\xFE charter\n"))
                        .to_bytes(),
                }),
            )),
            "charter is not valid UTF-8",
        ),
    ];
    for (name, bytes, expected, reason) in cases {
        let db = scratch("seal")?;
        let make = || {
            let charter = CharterStatus::from_bytes(bytes.clone(), expected.as_ref());
            let boot = SteleBoot::boot(&db, &charter, expected.and_then(|e| e.royal).as_ref());
            AccordionSwitchboardState {
                charter,
                stele_genesis: boot.genesis,
                ledgers: boot.ledgers.map(Mutex::new),
                runner_addr: SocketAddr::from(([127, 0, 0, 1], 9)),
            }
        };
        let (code, body) = status(make(), loopback()).await?;
        assert_eq!(code, StatusCode::OK, "{name}");
        assert_eq!(body["status"], "UNSEALED", "{name}");
        assert_eq!(body["charter_sealed"], false, "{name}");
        assert_eq!(body["seal_error"], reason, "{name}");
        assert_eq!(body["stele_genesis"], false, "{name}");
        let (code, body) = dispatch(make(), loopback(), GOOD).await?;
        assert_eq!(code, StatusCode::FORBIDDEN, "{name}");
        assert_eq!(body["error"], dispatch_error::UNSEALED, "{name}");
        assert!(!db.exists(), "{name}: vault must not be created");
        cleanup(&db)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- malformed dispatch

/// Bodies that must be refused with 400 before the runner (a dead port here) is touched.
#[tokio::test]
async fn malformed_dispatch_never_reaches_the_runner() -> TestResult {
    let max_model = "m".repeat(129);
    let long_model = format!(
        r#"{{"model":"{max_model}","bsmc_namespace":"a.b","terrace_level":3,"operator_prompt":"p"}}"#
    );
    let cases: Vec<(String, &str)> = [
        ("", dispatch_error::BAD_JSON),
        ("[]", dispatch_error::BAD_JSON),
        ("null", dispatch_error::BAD_JSON),
        (r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":"3","operator_prompt":"p"}"#, dispatch_error::BAD_JSON),
        (r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":3.5,"operator_prompt":"p"}"#, dispatch_error::BAD_JSON),
        (r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":99999999999999999999,"operator_prompt":"p"}"#, dispatch_error::BAD_JSON),
        (r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":3,"operator_prompt":null}"#, dispatch_error::BAD_JSON),
        (r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":3,"operator_prompt":"p","operator_prompt":"q"}"#, dispatch_error::BAD_JSON),
        (r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":9223372036854775807,"operator_prompt":"p"}"#, dispatch_error::BAD_TERRACE),
        (r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":-9223372036854775808,"operator_prompt":"p"}"#, dispatch_error::BAD_TERRACE),
        (r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":256,"operator_prompt":"p"}"#, dispatch_error::BAD_TERRACE),
        (r#"{"model":"m","bsmc_namespace":".a","terrace_level":3,"operator_prompt":"p"}"#, dispatch_error::BAD_NAMESPACE),
        (r#"{"model":"m","bsmc_namespace":"a.","terrace_level":3,"operator_prompt":"p"}"#, dispatch_error::BAD_NAMESPACE),
        (r#"{"model":"m","bsmc_namespace":"a..b","terrace_level":3,"operator_prompt":"p"}"#, dispatch_error::BAD_NAMESPACE),
        (r#"{"model":"m","bsmc_namespace":"","terrace_level":3,"operator_prompt":"p"}"#, dispatch_error::BAD_NAMESPACE),
        (r#"{"model":"","bsmc_namespace":"a.b","terrace_level":3,"operator_prompt":"p"}"#, dispatch_error::BAD_MODEL),
        (r#"{"model":"mé","bsmc_namespace":"a.b","terrace_level":3,"operator_prompt":"p"}"#, dispatch_error::BAD_MODEL),
        (r#"{"model":"m?x=1","bsmc_namespace":"a.b","terrace_level":3,"operator_prompt":"p"}"#, dispatch_error::BAD_MODEL),
        (long_model.as_str(), dispatch_error::BAD_MODEL),
    ]
    .into_iter()
    .map(|(b, e)| (b.to_string(), e))
    .collect();
    for (body, expect) in cases {
        let db = scratch("malformed")?;
        let (code, reply) =
            dispatch(sealed_state(&db, dead_port().await?), loopback(), &body).await?;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(reply["error"], expect, "{body}");
        assert_eq!(Stele::open(&db)?.len()?, 1, "{body}");
        cleanup(&db)?;
    }
    Ok(())
}

/// A body over the dispatch limit gets the documented JSON 413, not axum's plain-text one.
#[tokio::test]
async fn oversized_dispatch_body_is_a_documented_413() -> TestResult {
    let db = scratch("oversize")?;
    let prompt = "p".repeat(hcc_daemon::DISPATCH_MAX_BODY_BYTES + 1);
    let body = json!({
        "model": "m",
        "bsmc_namespace": "a.b",
        "terrace_level": 3,
        "operator_prompt": prompt,
    })
    .to_string();
    let (code, reply) = dispatch(sealed_state(&db, dead_port().await?), loopback(), &body).await?;
    assert_eq!(code, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply["error"], dispatch_error::TOO_LARGE);
    assert_eq!(Stele::open(&db)?.len()?, 1);
    cleanup(&db)?;
    Ok(())
}

/// A prompt just under the limit still validates and reaches the runner.
#[tokio::test]
async fn large_prompt_under_the_limit_reaches_the_runner() -> TestResult {
    let db = scratch("under-limit")?;
    let prompt = "p".repeat(hcc_daemon::DISPATCH_MAX_BODY_BYTES - 200);
    let body = json!({
        "model": "m",
        "bsmc_namespace": "a.b",
        "terrace_level": 3,
        "operator_prompt": prompt,
    })
    .to_string();
    let (code, reply) = dispatch(sealed_state(&db, dead_port().await?), loopback(), &body).await?;
    assert_eq!(code, StatusCode::BAD_GATEWAY, "{reply}");
    cleanup(&db)?;
    Ok(())
}

// ---------------------------------------------------------------- peers and binding

#[tokio::test]
async fn non_loopback_peers_are_refused_everywhere() -> TestResult {
    let peers: [SocketAddr; 5] = [
        SocketAddr::from(([10, 0, 0, 5], 1)),
        SocketAddr::from(([0, 0, 0, 0], 1)),
        SocketAddr::from(([169, 254, 1, 1], 1)),
        "[fe80::1]:1".parse()?,
        // IPv4-mapped loopback is refused: the daemon binds IPv4 127.0.0.1 only, so a
        // mapped address means something unexpected sits in between.
        "[::ffff:127.0.0.1]:1".parse()?,
    ];
    for peer in peers {
        let db = scratch("peers")?;
        let (code, body) = dispatch(sealed_state(&db, fake_runner("x").await?), peer, GOOD).await?;
        assert_eq!(code, StatusCode::FORBIDDEN, "{peer}");
        assert_eq!(body["error"], dispatch_error::NON_LOOPBACK, "{peer}");
        assert_eq!(Stele::open(&db)?.len()?, 1, "{peer}");
        let (code, _) = status(sealed_state(&db, dead_port().await?), peer).await?;
        assert_eq!(code, StatusCode::FORBIDDEN, "{peer}");
        cleanup(&db)?;
    }
    let db = scratch("v6-loopback")?;
    let (code, _) = status(sealed_state(&db, dead_port().await?), "[::1]:1".parse()?).await?;
    assert_eq!(code, StatusCode::OK, "::1 is loopback");
    cleanup(&db)?;
    Ok(())
}

#[tokio::test]
async fn port_already_taken_is_an_error_not_a_panic() -> TestResult {
    let held = TcpListener::bind("127.0.0.1:0").await?;
    let addr = held.local_addr()?;
    match bind_loopback(addr).await {
        Err(DaemonError::Io(_)) => Ok(()),
        Err(other) => Err(format!("expected Io, got {other}").into()),
        Ok(_) => Err("second bind on a held port succeeded".into()),
    }
}

#[tokio::test]
async fn unspecified_and_lan_binds_are_refused() -> TestResult {
    for addr in ["0.0.0.0:7749", "[::]:7749", "192.168.1.2:7749"] {
        let addr: SocketAddr = addr.parse()?;
        assert!(
            matches!(bind_loopback(addr).await, Err(DaemonError::NotLoopback(_))),
            "{addr}"
        );
    }
    Ok(())
}

#[test]
fn too_large_string_names_the_real_limit() {
    let limit = hcc_daemon::DISPATCH_MAX_BODY_BYTES.to_string();
    assert!(dispatch_error::TOO_LARGE.contains(&limit));
}
