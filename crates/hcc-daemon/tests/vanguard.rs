//! POST /v1/ramgenie/vanguard and GET /v1/ramgenie/jobs. Keys are generated in-test; the
//! runner is a fake on a loopback port.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use ed25519_dalek::{Signer, SigningKey};
use hcc_baton::baton_envelope;
use hcc_daemon::vanguard_wire::job_error;
use hcc_daemon::{
    dispatch_error, load_rmk, router, AccordionSwitchboardState, BootConfig, CharterStatus,
    Factory, SteleBoot,
};
use hcc_iedb_stele::Stele;
use hcc_krypt::RootMasterKey;
use hcc_seal::{charter_digest, digest_hex, RoyalSeal, SealConstants};
use hcc_types::{BsmcNamespace, TerraceLevel};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHARTER: &[u8] = b"vanguard wire test charter\n";

fn sealed_constants() -> SealConstants {
    let key = SigningKey::from_bytes(blake3::hash(b"vanguard wire in-test key").as_bytes());
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

/// Scratch dir holding the vault; removed on drop (after every connection is closed).
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Result<Self, std::io::Error> {
        let dir =
            std::env::temp_dir().join(format!("hcc-vanguard-wire-{name}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn db(&self) -> PathBuf {
        self.0.join("eduba_vault.db")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sealed_state(
    db: &Path,
    runner: SocketAddr,
    rmk: Option<RootMasterKey>,
) -> Arc<AccordionSwitchboardState> {
    let expected = sealed_constants();
    let charter = CharterStatus::from_bytes(CHARTER.to_vec(), Some(&expected));
    let boot = SteleBoot::boot(db, &charter, expected.royal.as_ref());
    Arc::new(
        AccordionSwitchboardState::new(charter, boot.genesis, boot.ledgers, runner).with_factory(
            Factory {
                vault_path: Some(db.to_path_buf()),
                workspace_root: Some(repo_root()),
                rmk,
            },
        ),
    )
}

fn loopback() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 50001))
}

async fn send(
    state: &Arc<AccordionSwitchboardState>,
    peer: SocketAddr,
    method: &str,
    path: &str,
    body: &str,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .extension(ConnectInfo(peer))
        .body(Body::from(body.to_string()))?;
    let response = router(Arc::clone(state)).oneshot(request).await?;
    let code = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 22).await?;
    Ok((code, serde_json::from_slice(&bytes)?))
}

async fn job(
    state: &Arc<AccordionSwitchboardState>,
    body: &str,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    send(state, loopback(), "POST", "/v1/ramgenie/vanguard", body).await
}

fn handoff(name: &str, terrace: u8, payload: Value) -> String {
    json!({ "job": name, "bsmc_namespace": name, "terrace_level": terrace, "payload": payload })
        .to_string()
}

async fn dead_port() -> Result<SocketAddr, std::io::Error> {
    TcpListener::bind("127.0.0.1:0").await?.local_addr()
}

#[tokio::test]
async fn job_table_lists_every_row_with_status_and_executor() -> TestResult {
    let scratch = Scratch::new("table")?;
    let state = sealed_state(&scratch.db(), dead_port().await?, None);
    let (code, body) = send(&state, loopback(), "GET", "/v1/ramgenie/jobs", "").await?;
    assert_eq!(code, StatusCode::OK);
    let jobs = body["jobs"].as_array().ok_or("no jobs")?;
    assert_eq!(jobs.len(), hcc_vanguard::VANGUARD_TABLE.len());
    assert_eq!(
        body["live"].as_u64().unwrap_or(0) + body["typed"].as_u64().unwrap_or(0),
        jobs.len() as u64
    );
    let scribe = jobs
        .iter()
        .find(|j| j["name"] == "scribe.stele.append")
        .ok_or("no scribe")?;
    assert_eq!(scribe["status"], "LIVE");
    assert_eq!(scribe["exec"], "ledger");
    let chare = jobs
        .iter()
        .find(|j| j["name"] == "chare.provenance.tag")
        .ok_or("no chare")?;
    assert_eq!(chare["status"], "TYPED");
    let far = SocketAddr::from(([192, 168, 1, 9], 50001));
    let (code, _) = send(&state, far, "GET", "/v1/ramgenie/jobs", "").await?;
    assert_eq!(code, StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn sealed_job_is_baton_traced_and_chained() -> TestResult {
    let scratch = Scratch::new("scribe")?;
    let state = sealed_state(&scratch.db(), dead_port().await?, None);
    let body = handoff("scribe.stele.append", 1, json!({ "text": "operator note" }));
    let (code, reply) = job(&state, &body).await?;
    assert_eq!(code, StatusCode::OK, "{reply}");
    assert_eq!(reply["status"], "JOB_SEALED");
    assert_eq!(reply["result"]["stele_id"], 2, "the job's own row");
    assert_eq!(reply["stele_job_row_id"], 3, "then the BATON record row");
    assert_eq!(reply["baton_trace_id"], 1);

    let result_bytes = serde_json::to_vec(&reply["result"])?;
    assert_eq!(
        reply["result_blake3_hex"],
        digest_hex(blake3::hash(&result_bytes).as_bytes())
    );
    let expected = baton_envelope(
        &charter_digest(CHARTER),
        &BsmcNamespace::parse("scribe.stele.append")?,
        TerraceLevel::Foundation,
        blake3::hash(body.as_bytes()).as_bytes(),
        blake3::hash(&result_bytes).as_bytes(),
    );
    assert_eq!(reply["baton_envelope_blake3_hex"], digest_hex(&expected));

    let stele = Stele::open(&scratch.db())?;
    assert_eq!(stele.verify_chain()?, 3);
    let row = stele.last()?.ok_or("no row")?;
    assert_eq!(row.module_namespace, "genie.dispatch.local");
    let text = String::from_utf8(row.payload)?;
    assert!(text.starts_with("HCC-VANGUARD-v1\njob=scribe.stele.append\n"));
    assert!(text.contains(&format!("baton_envelope_hex={}\n", digest_hex(&expected))));
    let baton = hcc_baton::BatonLedger::open(&scratch.db())?;
    assert_eq!(baton.replay(&charter_digest(CHARTER))?, 1);

    let (_, health) = send(&state, loopback(), "GET", "/v1/ziggurat/health", "").await?;
    assert_eq!(health["jobs_sealed"], 1);
    assert_eq!(health["jobs_refused"], 0);
    drop((stele, baton, state));
    Ok(())
}

#[tokio::test]
async fn refusals_record_nothing() -> TestResult {
    let scratch = Scratch::new("refuse")?;
    let state = sealed_state(&scratch.db(), dead_port().await?, None);
    let cases = [
        (
            "not json".to_string(),
            StatusCode::BAD_REQUEST,
            job_error::BAD_HANDOFF.to_string(),
        ),
        (
            r#"{"job":"scribe.stele.append","bsmc_namespace":"a.b","terrace_level":1,"extra":1}"#
                .to_string(),
            StatusCode::BAD_REQUEST,
            job_error::BAD_HANDOFF.to_string(),
        ),
        (
            handoff("no.such.job", 1, json!({})),
            StatusCode::BAD_REQUEST,
            "unknown job no.such.job".into(),
        ),
        (
            handoff("chare.provenance.tag", 1, json!({})),
            StatusCode::NOT_IMPLEMENTED,
            "chare.provenance.tag is TYPED: unimplemented in v1.0.0".into(),
        ),
        (
            handoff("scribe.stele.append", 9, json!({ "text": "x" })),
            StatusCode::BAD_REQUEST,
            "terrace_level 9 is outside 1..=5".into(),
        ),
        (
            handoff("krypt.stratum.init", 1, json!({ "label": "k" })),
            StatusCode::SERVICE_UNAVAILABLE,
            String::new(),
        ),
        (
            handoff(
                "zerotrace.key.destroy",
                1,
                json!({ "key_id": "0123456789abcdef0123456789abcdef" }),
            ),
            StatusCode::NOT_FOUND,
            String::new(),
        ),
        (
            handoff(
                "healer.diagnose.heap",
                2,
                json!({ "fixture": "../Cargo.toml" }),
            ),
            StatusCode::BAD_REQUEST,
            String::new(),
        ),
    ];
    for (body, status, error) in &cases {
        let (code, reply) = job(&state, body).await?;
        assert_eq!(code, *status, "{body} -> {reply}");
        if !error.is_empty() {
            assert_eq!(reply["error"], error.as_str());
        }
        assert!(reply.get("baton_envelope_blake3_hex").is_none());
    }
    let stele = Stele::open(&scratch.db())?;
    assert_eq!(stele.len()?, 1, "only genesis");
    assert!(hcc_baton::BatonLedger::open(&scratch.db())?
        .last()?
        .is_none());
    let (_, health) = send(&state, loopback(), "GET", "/v1/ziggurat/health", "").await?;
    assert_eq!(health["jobs_refused"], cases.len());
    assert_eq!(health["jobs_sealed"], 0);
    let far = SocketAddr::from(([10, 0, 0, 2], 50001));
    let (code, reply) = send(
        &state,
        far,
        "POST",
        "/v1/ramgenie/vanguard",
        &handoff("wasi.sandbox.lock", 2, json!({})),
    )
    .await?;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert_eq!(reply["error"], dispatch_error::NON_LOOPBACK);
    drop((stele, state));
    Ok(())
}

#[tokio::test]
async fn unsealed_charter_refuses_every_job() -> TestResult {
    let scratch = Scratch::new("unsealed")?;
    let charter = CharterStatus::from_bytes(b"tampered\n".to_vec(), Some(&sealed_constants()));
    let state = Arc::new(
        AccordionSwitchboardState::new(charter, false, None, dead_port().await?).with_factory(
            Factory {
                vault_path: Some(scratch.db()),
                workspace_root: Some(repo_root()),
                rmk: None,
            },
        ),
    );
    let (code, reply) = job(&state, &handoff("wasi.sandbox.lock", 2, json!({}))).await?;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert_eq!(reply["error"], dispatch_error::UNSEALED);
    Ok(())
}

#[tokio::test]
async fn krypt_key_lifecycle_through_the_wire() -> TestResult {
    let scratch = Scratch::new("krypt")?;
    let state = sealed_state(
        &scratch.db(),
        dead_port().await?,
        Some(RootMasterKey::generate()?),
    );
    let (code, made) = job(
        &state,
        &handoff("krypt.stratum.init", 1, json!({ "label": "lessons.v1" })),
    )
    .await?;
    assert_eq!(code, StatusCode::OK, "{made}");
    let key_id = made["result"]["key_id"]
        .as_str()
        .ok_or("no key id")?
        .to_owned();
    let (code, shred) = job(
        &state,
        &handoff("krypt.death.keyDestroy", 1, json!({ "key_id": key_id })),
    )
    .await?;
    assert_eq!(code, StatusCode::OK, "{shred}");
    assert_eq!(shred["result"]["key_status"], "DESTROYED");
    let (code, again) = job(
        &state,
        &handoff("zerotrace.key.destroy", 1, json!({ "key_id": key_id })),
    )
    .await?;
    assert_eq!(code, StatusCode::CONFLICT, "{again}");
    let (code, verify) = job(&state, &handoff("krypt.stele.verify", 1, json!({}))).await?;
    assert_eq!(code, StatusCode::OK, "{verify}");
    // genesis, stratum row, its job row, certificate, its job row
    assert_eq!(verify["result"]["chain_rows_verified"], 5);
    Ok(())
}

#[tokio::test]
async fn healer_and_lock_jobs_run_the_committed_fixture() -> TestResult {
    let scratch = Scratch::new("healer")?;
    let state = sealed_state(&scratch.db(), dead_port().await?, None);
    let (code, reply) = job(
        &state,
        &handoff(
            "healer.diagnose.heap",
            2,
            json!({ "fixture": "healer_empty.wasm" }),
        ),
    )
    .await?;
    assert_eq!(code, StatusCode::OK, "{reply}");
    let fixture = std::fs::read(repo_root().join("fixtures/healer_empty.wasm"))?;
    assert_eq!(
        reply["result"]["wasm_blake3_hex"],
        digest_hex(blake3::hash(&fixture).as_bytes())
    );
    let (code, lock) = job(&state, &handoff("wasi.sandbox.lock", 2, json!({}))).await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(lock["result"]["fuel"], 10_000_000);
    Ok(())
}

/// Fake `/api/embed`: one fixed 2-d vector per input, picked by its first byte.
async fn fake_embed() -> Result<SocketAddr, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = Router::new().route(
        "/api/embed",
        post(|Json(req): Json<Value>| async move {
            let vectors: Vec<Value> = req["input"]
                .as_array()
                .map(|inputs| {
                    inputs
                        .iter()
                        .map(|t| match t.as_str().and_then(|t| t.bytes().next()) {
                            Some(b'S') => json!([1.0, 0.0]),
                            _ => json!([0.0, 1.0]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Json(json!({ "embeddings": vectors }))
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(addr)
}

#[tokio::test]
async fn lessons_store_index_and_retrieve_through_the_wire() -> TestResult {
    let scratch = Scratch::new("lessons")?;
    let down = sealed_state(&scratch.db(), dead_port().await?, None);
    for (title, body) in [
        ("seal", "Seal before you dispatch."),
        ("baton", "BATON replays every trace."),
    ] {
        let (code, reply) = job(
            &down,
            &handoff(
                "vault.store.lesson",
                3,
                json!({ "title": title, "body": body }),
            ),
        )
        .await?;
        assert_eq!(code, StatusCode::OK, "{reply}");
    }
    let upsert = handoff(
        "neural.index.upsert",
        3,
        json!({ "model": "nomic-embed-text" }),
    );
    let (code, reply) = job(&down, &upsert).await?;
    assert_eq!(code, StatusCode::BAD_GATEWAY, "{reply}");
    drop(down);

    let up = sealed_state(&scratch.db(), fake_embed().await?, None);
    let (code, reply) = job(&up, &upsert).await?;
    assert_eq!(code, StatusCode::OK, "{reply}");
    assert_eq!(reply["result"]["indexed"], 2);
    let (code, reply) = job(
        &up,
        &handoff(
            "neural.retrieve.lesson",
            3,
            json!({ "model": "nomic-embed-text", "query": "Should I seal?", "k": 1 }),
        ),
    )
    .await?;
    assert_eq!(code, StatusCode::OK, "{reply}");
    assert_eq!(reply["result"]["hits"][0]["title"], "seal");
    let stele = Stele::open(&scratch.db())?;
    // genesis + 2 × (lesson row + job row) + 2 runner job rows
    assert_eq!(stele.verify_chain()?, 7);
    drop((stele, up));
    Ok(())
}

#[test]
fn rmk_file_must_be_exactly_32_bytes() -> TestResult {
    let scratch = Scratch::new("rmk")?;
    let path = scratch.0.join("krypt.rmk");
    assert!(load_rmk(&path)?.is_none(), "absent file is not an error");
    std::fs::write(&path, [7u8; 31])?;
    assert!(load_rmk(&path).is_err());
    std::fs::write(&path, [7u8; 32])?;
    let rmk = load_rmk(&path)?.ok_or("no rmk")?;
    assert_eq!(rmk.expose(), &[7u8; 32]);
    Ok(())
}

#[test]
fn boot_flags_parse_and_reject_unknowns() -> TestResult {
    let args: Vec<String> = ["--db", "v.db", "--rmk", "k.rmk", "--workspace", "w"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let config = BootConfig::from_args(&args)?;
    assert_eq!(config.db_path, PathBuf::from("v.db"));
    assert_eq!(config.rmk_path, PathBuf::from("k.rmk"));
    assert_eq!(config.workspace_root, PathBuf::from("w"));
    assert_eq!(
        config.charter_path,
        PathBuf::from(hcc_daemon::DEFAULT_CHARTER_PATH)
    );
    assert!(BootConfig::from_args(&["--db".to_string()]).is_err());
    assert!(BootConfig::from_args(&["--bogus".to_string(), "x".to_string()]).is_err());
    Ok(())
}

#[test]
fn job_strings_are_documented() -> TestResult {
    let doc = std::fs::read_to_string(repo_root().join("SPEC/ERRORS.md"))?;
    for s in job_error::ALL {
        assert!(
            doc.contains(&format!("`{s}`")),
            "SPEC/ERRORS.md missing {s}"
        );
    }
    assert!(doc.contains("`POST /v1/ramgenie/vanguard` errors"));
    Ok(())
}

async fn send_from(
    state: &Arc<AccordionSwitchboardState>,
    origin: &str,
    content_type: &str,
    body: &str,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/ramgenie/vanguard")
        .header("content-type", content_type)
        .header("origin", origin)
        .extension(ConnectInfo(loopback()))
        .body(Body::from(body.to_string()))?;
    let response = router(Arc::clone(state)).oneshot(request).await?;
    let code = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 22).await?;
    Ok((code, serde_json::from_slice(&bytes)?))
}

#[tokio::test]
async fn foreign_origin_posts_are_refused_before_any_job_runs() -> TestResult {
    let scratch = Scratch::new("origin")?;
    let state = sealed_state(&scratch.db(), dead_port().await?, None);
    let body = handoff(
        "scribe.stele.append",
        1,
        json!({ "text": "from a web page" }),
    );
    for origin in ["https://evil.example", "null", "http://127.0.0.1:8080"] {
        let (code, reply) = send_from(&state, origin, "text/plain", &body).await?;
        assert_eq!(code, StatusCode::FORBIDDEN, "{origin}");
        assert_eq!(reply["error"], dispatch_error::FOREIGN_ORIGIN);
    }
    let dispatch = json!({ "model": "m", "bsmc_namespace": "a.b", "terrace_level": 1,
        "operator_prompt": "x" })
    .to_string();
    let request = Request::builder()
        .method("POST")
        .uri("/v1/ramgenie/dispatch")
        .header("content-type", "text/plain")
        .header("origin", "https://evil.example")
        .extension(ConnectInfo(loopback()))
        .body(Body::from(dispatch))?;
    let response = router(Arc::clone(&state)).oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        Stele::open(&scratch.db())?.verify_chain()?,
        1,
        "genesis only"
    );

    // A foreign page may still GET (it just cannot read the reply: no CORS headers).
    let (code, _) = send(&state, loopback(), "GET", "/v1/ramgenie/jobs", "").await?;
    assert_eq!(code, StatusCode::OK);
    // The cockpit's own origin runs the job.
    let (code, reply) =
        send_from(&state, "http://tauri.localhost", "application/json", &body).await?;
    assert_eq!(code, StatusCode::OK, "{reply}");
    Ok(())
}

#[tokio::test]
async fn a_deterministic_job_repeats_with_a_new_trace_each_time() -> TestResult {
    let scratch = Scratch::new("repeat")?;
    let state = sealed_state(&scratch.db(), dead_port().await?, None);
    let body = handoff("wasi.sandbox.lock", 2, json!({}));
    let (code, first) = job(&state, &body).await?;
    assert_eq!(code, StatusCode::OK, "{first}");
    assert_eq!(first["result"]["baton_prev_envelope_hex"], Value::Null);
    let (code, second) = job(&state, &body).await?;
    assert_eq!(code, StatusCode::OK, "{second}");
    assert_eq!(second["baton_trace_id"], 2);
    assert_ne!(
        first["baton_envelope_blake3_hex"],
        second["baton_envelope_blake3_hex"]
    );
    assert_eq!(
        second["result"]["baton_prev_envelope_hex"],
        first["baton_envelope_blake3_hex"]
    );
    let baton = hcc_baton::BatonLedger::open(&scratch.db())?;
    assert_eq!(baton.replay(&charter_digest(CHARTER))?, 2);
    Ok(())
}
