//! GET /v1/ziggurat/pillar/{module_id}/status. Keys are generated in-test.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use ed25519_dalek::{Signer, SigningKey};
use hcc_daemon::{bind_loopback, router, AccordionSwitchboardState, CharterStatus, SteleBoot};
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
    let state = AccordionSwitchboardState::new(charter, false, None, runner_addr);
    get_with(state, peer, module_id).await
}

async fn get_with(
    state: AccordionSwitchboardState,
    peer: SocketAddr,
    module_id: &str,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let state = Arc::new(state);
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
    assert!(
        body.get("seal_error").is_none(),
        "seal_error must be omitted when sealed"
    );
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
    assert_eq!(
        body["seal_error"],
        "charter byte length does not match baked seal"
    );
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
    assert_eq!(body["seal_error"], "charter file unreadable");
    assert!(
        !body.to_string().contains("nonexistent"),
        "no paths on the wire"
    );
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
    let state = Arc::new(AccordionSwitchboardState::new(
        CharterStatus::from_bytes(SAMPLE.to_vec(), None),
        false,
        None,
        dead_port().await?,
    ));
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

#[test]
fn unreadable_string_is_documented() -> Result<(), std::io::Error> {
    let doc = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../SPEC/ERRORS.md"),
    )?;
    assert!(doc.contains(&format!("| `{}` |", hcc_daemon::CHARTER_UNREADABLE)));
    Ok(())
}

#[test]
fn index_lists_every_served_module_id() -> Result<(), std::io::Error> {
    let index =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../INDEX.md"))?;
    for id in hcc_daemon::MODULE_IDS {
        assert!(
            index.contains(&format!("| `{id}` |")),
            "INDEX.md §4 is missing {id}"
        );
    }
    Ok(())
}

fn scratch_db(name: &str) -> Result<std::path::PathBuf, std::io::Error> {
    let dir = std::env::temp_dir().join(format!("hcc-daemon-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("eduba_vault.db"))
}

#[tokio::test]
async fn sealed_boot_writes_genesis_and_reports_it() -> TestResult {
    let db = scratch_db("sealed")?;
    let expected = sealed_constants(SAMPLE);
    let royal = expected.royal;
    for _boot in 0..2 {
        let charter = CharterStatus::from_bytes(SAMPLE.to_vec(), Some(&expected));
        let stele = SteleBoot::boot(&db, &charter, royal.as_ref());
        assert!(stele.genesis, "{}", stele.note);
        let state =
            AccordionSwitchboardState::new(charter, stele.genesis, None, dead_port().await?);
        let (_, body) = get_with(state, loopback_peer(), "M09-RamGenie").await?;
        assert_eq!(body["charter_sealed"], true);
        assert_eq!(body["stele_genesis"], true);
        drop(stele);
    }
    let stele = hcc_iedb_stele::Stele::open(&db)?;
    assert_eq!(
        stele.len()?,
        1,
        "second boot must not append a second genesis"
    );
    let row = stele.first()?.ok_or("no genesis row")?;
    let payload = String::from_utf8(row.payload)?;
    assert!(payload.contains(&digest_hex(&charter_digest(SAMPLE))));
    // Windows cannot delete a file SQLite still holds open (os error 32).
    drop(stele);
    if let Some(dir) = db.parent() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[tokio::test]
async fn unsealed_boot_never_opens_the_stele() -> TestResult {
    let db = scratch_db("unsealed")?;
    let charter = CharterStatus::from_bytes(SAMPLE.to_vec(), None);
    let stele = SteleBoot::boot(&db, &charter, None);
    assert!(!stele.genesis);
    assert!(!db.exists(), "unsealed boot must not create the vault");
    let state = AccordionSwitchboardState::new(charter, stele.genesis, None, dead_port().await?);
    let (_, body) = get_with(state, loopback_peer(), "M09-RamGenie").await?;
    assert_eq!(body["stele_genesis"], false);
    if let Some(dir) = db.parent() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[tokio::test]
async fn vault_from_another_charter_reports_no_genesis() -> TestResult {
    let db = scratch_db("other")?;
    let first = sealed_constants(b"first charter\n");
    let first_charter = CharterStatus::from_bytes(b"first charter\n".to_vec(), Some(&first));
    assert!(SteleBoot::boot(&db, &first_charter, first.royal.as_ref()).genesis);
    let expected = sealed_constants(SAMPLE);
    let charter = CharterStatus::from_bytes(SAMPLE.to_vec(), Some(&expected));
    let stele = SteleBoot::boot(&db, &charter, expected.royal.as_ref());
    assert!(!stele.genesis);
    assert!(stele.note.contains("different charter"), "{}", stele.note);
    drop(stele);
    if let Some(dir) = db.parent() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[tokio::test]
async fn bumped_charter_keeps_the_vault_and_retires_the_old_one() -> TestResult {
    let db = scratch_db("bump")?;
    let v100 = sealed_constants(b"first charter\n");
    let v101 = sealed_constants(SAMPLE);
    let first = CharterStatus::from_bytes(b"first charter\n".to_vec(), Some(&v100));
    assert!(SteleBoot::boot(&db, &first, v100.royal.as_ref()).genesis);
    {
        let mut stele = hcc_iedb_stele::Stele::open(&db)?;
        let charter = hcc_seal::SystemCharter::load(SAMPLE.to_vec())
            .verify(Some(&v101))
            .map_err(|r| r.error.to_string())?;
        let royal = v101.royal.ok_or("no royal seal")?;
        stele.record_charter_version(&charter, &royal, "1.0.1")?;
    }
    let bumped = CharterStatus::from_bytes(SAMPLE.to_vec(), Some(&v101));
    let stele = SteleBoot::boot(&db, &bumped, v101.royal.as_ref());
    assert!(stele.genesis, "{}", stele.note);
    assert!(stele.ledgers.is_some(), "{}", stele.note);
    assert!(
        stele
            .note
            .contains("charter v1.0.1 (2 version(s) in lineage)"),
        "{}",
        stele.note
    );
    drop(stele);
    let stele = SteleBoot::boot(&db, &first, v100.royal.as_ref());
    assert!(
        !stele.genesis,
        "the retired charter must not boot: {}",
        stele.note
    );
    drop(stele);
    if let Some(dir) = db.parent() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

async fn with_origin(
    method: &str,
    origin: Option<&str>,
) -> Result<axum::response::Response, Box<dyn std::error::Error>> {
    let state = Arc::new(AccordionSwitchboardState::new(
        CharterStatus::from_bytes(SAMPLE.to_vec(), None),
        false,
        None,
        dead_port().await?,
    ));
    let mut request = Request::builder()
        .method(method)
        .uri("/v1/ziggurat/pillar/M09-RamGenie/status")
        .extension(ConnectInfo(loopback_peer()));
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    Ok(router(state).oneshot(request.body(Body::empty())?).await?)
}

#[tokio::test]
async fn cors_is_granted_only_to_the_cockpit_webview() -> TestResult {
    for origin in hcc_daemon::COCKPIT_ORIGINS {
        let response = with_origin("GET", Some(origin)).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("access-control-allow-origin"),
            Some(&origin.parse()?),
            "{origin}"
        );
        let preflight = with_origin("OPTIONS", Some(origin)).await?;
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            preflight.headers().get("access-control-allow-headers"),
            Some(&"content-type".parse()?)
        );
    }
    for origin in [Some("http://evil.example"), Some("null"), None] {
        let response = with_origin("GET", origin).await?;
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none(),
            "{origin:?}"
        );
    }
    let preflight = with_origin("OPTIONS", Some("http://evil.example")).await?;
    assert_eq!(preflight.status(), StatusCode::FORBIDDEN);
    Ok(())
}
