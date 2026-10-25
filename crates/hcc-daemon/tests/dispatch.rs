//! POST /v1/ramgenie/dispatch against a fake runner on a loopback port. Keys are generated in-test.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use ed25519_dalek::{Signer, SigningKey};
use hcc_baton::baton_envelope;
use hcc_daemon::{dispatch_error, router, AccordionSwitchboardState, CharterStatus, SteleBoot};
use hcc_iedb_stele::Stele;
use hcc_seal::{charter_digest, digest_hex, RoyalSeal, SealConstants};
use hcc_types::{BsmcNamespace, TerraceLevel};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHARTER: &[u8] = b"dispatch test charter\n";
const GOOD: &str = r#"{"model":"ziggurat:1.0.0","bsmc_namespace":"seed.scaffold.emit","terrace_level":3,"operator_prompt":"Scaffold crate capsnap."}"#;

fn sealed_constants() -> SealConstants {
    let key = SigningKey::from_bytes(blake3::hash(b"dispatch in-test key").as_bytes());
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
    let dir = std::env::temp_dir().join(format!("hcc-dispatch-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("eduba_vault.db"))
}

/// Fake Ollama: echoes what it saw so the test can check the injected system prompt.
async fn fake_runner(reply: Value, code: StatusCode) -> Result<SocketAddr, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = Router::new().route(
        "/api/chat",
        post(move |Json(req): Json<Value>| {
            let reply = reply.clone();
            async move {
                let system = req["messages"][0]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let mut body = reply;
                if let Some(msg) = body.get_mut("message") {
                    msg["content"] = json!(format!(
                        "{}|system_len={}",
                        msg["content"].as_str().unwrap_or_default(),
                        system.len()
                    ));
                }
                (code, Json(body))
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(addr)
}

async fn dead_port() -> Result<SocketAddr, std::io::Error> {
    TcpListener::bind("127.0.0.1:0").await?.local_addr()
}

fn sealed_state(db: &std::path::Path, runner_addr: SocketAddr) -> AccordionSwitchboardState {
    let expected = sealed_constants();
    let charter = CharterStatus::from_bytes(CHARTER.to_vec(), Some(&expected));
    let boot = SteleBoot::boot(db, &charter, expected.royal.as_ref());
    AccordionSwitchboardState::new(charter, boot.genesis, boot.ledgers, runner_addr)
}

async fn post_dispatch(
    state: AccordionSwitchboardState,
    peer: SocketAddr,
    body: &str,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let request = Request::post("/v1/ramgenie/dispatch")
        .header("content-type", "application/json")
        .extension(ConnectInfo(peer))
        .body(Body::from(body.to_string()))?;
    let response = router(Arc::new(state)).oneshot(request).await?;
    let code = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await?;
    Ok((code, serde_json::from_slice(&bytes)?))
}

fn loopback() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 50000))
}

#[tokio::test]
async fn sealed_dispatch_records_baton_and_stele() -> TestResult {
    let db = scratch("ok")?;
    let runner = fake_runner(
        json!({"message": {"role": "assistant", "content": "crate written"}}),
        StatusCode::OK,
    )
    .await?;
    let (code, body) = post_dispatch(sealed_state(&db, runner), loopback(), GOOD).await?;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "DISPATCH_SEALED");
    assert_eq!(body["terrace_level"], 3);
    assert_eq!(
        body["charter_blake3_hex"],
        digest_hex(&charter_digest(CHARTER))
    );
    let output = format!("crate written|system_len={}", CHARTER.len());
    assert_eq!(
        body["model_output"],
        output.as_str(),
        "charter must be the system prompt"
    );

    let expected = baton_envelope(
        &charter_digest(CHARTER),
        &BsmcNamespace::parse("seed.scaffold.emit")?,
        TerraceLevel::Workforce,
        blake3::hash(b"Scaffold crate capsnap.").as_bytes(),
        blake3::hash(output.as_bytes()).as_bytes(),
    );
    assert_eq!(body["baton_envelope_blake3_hex"], digest_hex(&expected));
    assert_eq!(body["baton_trace_id"], 1);
    assert_eq!(body["stele_dispatch_row_id"], 2);

    let stele = Stele::open(&db)?;
    assert_eq!(stele.verify_chain()?, 2);
    let row = stele.last()?.ok_or("no dispatch row")?;
    assert_eq!(row.module_namespace, "genie.dispatch.local");
    assert!(String::from_utf8(row.payload)?
        .contains(&format!("baton_envelope_hex={}\n", digest_hex(&expected))));
    let baton = hcc_baton::BatonLedger::open(&db)?;
    assert_eq!(baton.replay(&charter_digest(CHARTER))?, 1);
    // Windows cannot delete a file SQLite still holds open (os error 32).
    drop(stele);
    drop(baton);
    if let Some(dir) = db.parent() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[tokio::test]
async fn runner_down_is_502_and_nothing_is_appended() -> TestResult {
    let db = scratch("down")?;
    let (code, body) =
        post_dispatch(sealed_state(&db, dead_port().await?), loopback(), GOOD).await?;
    assert_eq!(code, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"], "runner unreachable on 127.0.0.1:11434");
    assert!(body.get("baton_envelope_blake3_hex").is_none());
    assert_eq!(Stele::open(&db)?.len()?, 1, "only genesis");
    assert!(hcc_baton::BatonLedger::open(&db)?.last()?.is_none());
    if let Some(dir) = db.parent() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[tokio::test]
async fn runner_error_status_and_bad_body_are_502() -> TestResult {
    for (reply, status, wire) in [
        (
            json!({"error": "model \"ziggurat:1.0.0\" not found, try pulling it first"}),
            StatusCode::NOT_FOUND,
            "runner returned a non-success HTTP status: HTTP 404: model \"ziggurat:1.0.0\" not found, try pulling it first",
        ),
        (
            json!({"done": true}),
            StatusCode::OK,
            "runner response has no message.content",
        ),
    ] {
        let db = scratch("bad")?;
        let runner = fake_runner(reply, status).await?;
        let (code, body) = post_dispatch(sealed_state(&db, runner), loopback(), GOOD).await?;
        assert_eq!(code, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"], wire);
        assert_eq!(Stele::open(&db)?.len()?, 1);
        if let Some(dir) = db.parent() {
            std::fs::remove_dir_all(dir)?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn unsealed_charter_cannot_dispatch() -> TestResult {
    let runner = fake_runner(json!({"message": {"content": "x"}}), StatusCode::OK).await?;
    let state = AccordionSwitchboardState::new(
        CharterStatus::from_bytes(CHARTER.to_vec(), None),
        false,
        None,
        runner,
    );
    let (code, body) = post_dispatch(state, loopback(), GOOD).await?;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], dispatch_error::UNSEALED);
    Ok(())
}

#[tokio::test]
async fn requests_are_validated_before_the_runner_is_called() -> TestResult {
    let cases = [
        (
            r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":0,"operator_prompt":"p"}"#,
            dispatch_error::BAD_TERRACE,
        ),
        (
            r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":6,"operator_prompt":"p"}"#,
            dispatch_error::BAD_TERRACE,
        ),
        (
            r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":-1,"operator_prompt":"p"}"#,
            dispatch_error::BAD_TERRACE,
        ),
        (
            r#"{"model":"m","bsmc_namespace":"nodot","terrace_level":3,"operator_prompt":"p"}"#,
            dispatch_error::BAD_NAMESPACE,
        ),
        (
            r#"{"model":"bad model\n","bsmc_namespace":"a.b","terrace_level":3,"operator_prompt":"p"}"#,
            dispatch_error::BAD_MODEL,
        ),
        (
            r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":3,"operator_prompt":""}"#,
            dispatch_error::EMPTY_PROMPT,
        ),
        (
            r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":3}"#,
            dispatch_error::BAD_JSON,
        ),
        (
            r#"{"model":"m","bsmc_namespace":"a.b","terrace_level":3,"operator_prompt":"p","x":1}"#,
            dispatch_error::BAD_JSON,
        ),
        ("not json", dispatch_error::BAD_JSON),
    ];
    for (body, expect) in cases {
        let db = scratch("validate")?;
        let (code, reply) =
            post_dispatch(sealed_state(&db, dead_port().await?), loopback(), body).await?;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(reply["error"], expect, "{body}");
        if let Some(dir) = db.parent() {
            std::fs::remove_dir_all(dir)?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn non_loopback_peer_cannot_dispatch() -> TestResult {
    let db = scratch("peer")?;
    let runner = fake_runner(json!({"message": {"content": "x"}}), StatusCode::OK).await?;
    let peer = SocketAddr::from(([192, 168, 1, 20], 4000));
    let (code, body) = post_dispatch(sealed_state(&db, runner), peer, GOOD).await?;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], dispatch_error::NON_LOOPBACK);
    assert_eq!(Stele::open(&db)?.len()?, 1);
    if let Some(dir) = db.parent() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[tokio::test]
async fn sealed_without_ledger_is_held() -> TestResult {
    let runner = fake_runner(json!({"message": {"content": "x"}}), StatusCode::OK).await?;
    let expected = sealed_constants();
    let state = AccordionSwitchboardState::new(
        CharterStatus::from_bytes(CHARTER.to_vec(), Some(&expected)),
        false,
        None,
        runner,
    );
    let (code, body) = post_dispatch(state, loopback(), GOOD).await?;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], dispatch_error::NO_LEDGER);
    Ok(())
}

#[tokio::test]
async fn installed_ollama_names_pass_the_model_allow_list() -> TestResult {
    // Names from a real `ollama list`: a tag, and a hf.co path with underscores.
    for model in [
        "cogito:latest",
        "hf.co/kusonooyasumi/qwen-2.5-coder-1.5b-svelte:Q4_K_M",
    ] {
        let db = scratch("names")?;
        let body = json!({
            "model": model,
            "bsmc_namespace": "genie.dispatch.local",
            "terrace_level": 3,
            "operator_prompt": "p",
        })
        .to_string();
        let (code, reply) =
            post_dispatch(sealed_state(&db, dead_port().await?), loopback(), &body).await?;
        // Validation passed; the request got as far as the (absent) runner.
        assert_eq!(code, StatusCode::BAD_GATEWAY, "{model}");
        assert_eq!(
            reply["error"], "runner unreachable on 127.0.0.1:11434",
            "{model}"
        );
        if let Some(dir) = db.parent() {
            std::fs::remove_dir_all(dir)?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn runner_error_detail_is_trimmed_and_flattened() -> TestResult {
    let db = scratch("long")?;
    let long = format!("line one\nline two {}", "x".repeat(500));
    let runner = fake_runner(json!({ "error": long }), StatusCode::INTERNAL_SERVER_ERROR).await?;
    let (code, body) = post_dispatch(sealed_state(&db, runner), loopback(), GOOD).await?;
    assert_eq!(code, StatusCode::BAD_GATEWAY);
    let error = body["error"].as_str().unwrap_or_default();
    let prefix = "runner returned a non-success HTTP status: HTTP 500: line one line two x";
    assert!(error.starts_with(prefix), "{error}");
    assert!(error.ends_with('…') && !error.contains('\n'), "{error}");
    assert_eq!(
        error.chars().count(),
        "runner returned a non-success HTTP status: HTTP 500: "
            .chars()
            .count()
            + hcc_daemon::ramgenie::RUNNER_DETAIL_MAX_CHARS
            + 1
    );
    assert_eq!(Stele::open(&db)?.len()?, 1);
    if let Some(dir) = db.parent() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[test]
fn dispatch_strings_are_documented() -> Result<(), std::io::Error> {
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../SPEC/ERRORS.md"),
    )?;
    for s in dispatch_error::ALL {
        assert!(
            doc.contains(&format!("| `{s}` |")),
            "SPEC/ERRORS.md missing {s}"
        );
    }
    for err in hcc_daemon::ramgenie::RunnerError::ALL {
        assert!(
            doc.contains(&format!("| `{}` |", err.wire())),
            "SPEC/ERRORS.md missing {}",
            err.wire()
        );
    }
    Ok(())
}
