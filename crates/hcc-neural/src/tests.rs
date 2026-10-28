use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use hcc_iedb_stele::Stele;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::embed::parse_embeddings;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PUBKEY: [u8; 32] = [7; 32];
const MODEL: &str = "nomic-embed-text:latest";
const DIM: usize = 64;
const WAIT: Duration = Duration::from_secs(10);

/// A scratch vault directory, removed on drop. Callers drop every connection first (Windows).
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Result<Self, std::io::Error> {
        let dir = std::env::temp_dir().join(format!("hcc-neural-{tag}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn vault(&self) -> PathBuf {
        self.0.join("eduba_vault.db")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Deterministic bag-of-words vector: each lowercase word adds 1 at a BLAKE3-chosen slot.
fn bag(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; DIM];
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let h = blake3::hash(word.to_lowercase().as_bytes());
        let slot = usize::from(h.as_bytes()[0]) % DIM;
        v[slot] += 1.0;
    }
    v
}

#[derive(Clone, Copy)]
enum Reply {
    /// Ollama-shaped embeddings, sent with Transfer-Encoding: chunked like Go's net/http.
    Embeddings,
    /// One vector fewer than inputs.
    Short,
    NotFound,
}

/// A fake loopback runner serving `count` requests. Returns its address.
async fn fake_runner(reply: Reply, count: usize) -> Result<SocketAddr, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        for _ in 0..count {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let body = loop {
                let Ok(n) = socket.read(&mut chunk).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if body.len() >= len {
                        break body.to_owned();
                    }
                }
            };
            let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let inputs: Vec<String> = request["input"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let response = match reply {
                Reply::NotFound => {
                    let json = format!(
                        "{{\"error\":\"model \\\"{}\\\" not found, try pulling it first\"}}",
                        request["model"].as_str().unwrap_or("")
                    );
                    format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                        json.len()
                    )
                }
                Reply::Embeddings | Reply::Short => {
                    let take = match reply {
                        Reply::Short => inputs.len().saturating_sub(1),
                        _ => inputs.len(),
                    };
                    let vectors: Vec<Vec<f32>> = inputs.iter().take(take).map(|t| bag(t)).collect();
                    let json =
                        serde_json::json!({ "model": request["model"], "embeddings": vectors })
                            .to_string();
                    let mut out = String::from(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                    );
                    for piece in json.as_bytes().chunks(1000) {
                        out.push_str(&format!(
                            "{:x}\r\n{}\r\n",
                            piece.len(),
                            String::from_utf8_lossy(piece)
                        ));
                    }
                    out.push_str("0\r\n\r\n");
                    out
                }
            };
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    Ok(addr)
}

fn seed(store: &mut LessonStore) -> Result<(), NeuralError> {
    store.store(
        "seed.scaffold.emit",
        "WAL first",
        "Open sqlite in WAL mode before creating tables",
        &PUBKEY,
    )?;
    store.store(
        "wasi.sandbox.lock",
        "Fuel caps",
        "Every wasm module runs under a fuel cap and a memory cap",
        &PUBKEY,
    )?;
    store.store(
        "krypt.fe_key.encrypt",
        "Wrap the FEK",
        "The file key is stored only wrapped under the data key",
        &PUBKEY,
    )?;
    Ok(())
}

#[test]
fn store_appends_lesson_and_stele_row_atomically() -> TestResult {
    let scratch = Scratch::new("store")?;
    let mut store = LessonStore::open(&scratch.vault())?;
    let lesson = store.store(
        "seed.scaffold.emit",
        "WAL first",
        "Open sqlite in WAL mode",
        &PUBKEY,
    )?;
    assert_eq!(lesson.lesson_id, 1);
    assert_eq!(lesson.stele_id, 1);
    let stele = Stele::open(&scratch.vault())?;
    assert_eq!(stele.verify_chain()?, 1);
    let row = stele.row(1)?.ok_or("stele row missing")?;
    assert_eq!(row.module_namespace, LESSON_NAMESPACE);
    assert_eq!(
        row.payload,
        lesson_payload(1, "seed.scaffold.emit", "WAL first", &lesson.body_blake3)
    );
    assert!(!String::from_utf8(row.payload)?.contains("Open sqlite"));

    assert!(matches!(
        store.store(
            "seed.scaffold.emit",
            "again",
            "Open sqlite in WAL mode",
            &PUBKEY
        ),
        Err(NeuralError::Duplicate { lesson_id: 1 })
    ));
    assert!(matches!(
        store.store("nodot", "t", "b", &PUBKEY),
        Err(NeuralError::BadNamespace)
    ));
    assert!(matches!(
        store.store("a.b", "", "b", &PUBKEY),
        Err(NeuralError::BadTitle)
    ));
    assert!(matches!(
        store.store("a.b", "t\nx", "b", &PUBKEY),
        Err(NeuralError::BadTitle)
    ));
    assert!(matches!(
        store.store("a.b", "t", "", &PUBKEY),
        Err(NeuralError::BadBody)
    ));
    let huge = "x".repeat(BODY_MAX_BYTES + 1);
    assert!(matches!(
        store.store("a.b", "t", &huge, &PUBKEY),
        Err(NeuralError::BadBody)
    ));
    assert_eq!(store.len()?, 1);
    assert_eq!(stele.verify_chain()?, 1);

    let conn = store.connection();
    assert!(conn
        .execute("UPDATE hcc_neural_lesson SET title = 'x'", [])
        .is_err());
    assert!(conn.execute("DELETE FROM hcc_neural_lesson", []).is_err());
    drop(stele);
    drop(store);
    Ok(())
}

#[test]
fn nearest_is_exact_cosine_with_stable_ties() -> TestResult {
    let scratch = Scratch::new("nearest")?;
    let mut store = LessonStore::open(&scratch.vault())?;
    seed(&mut store)?;
    assert_eq!(store.pending(MODEL)?.len(), 3);
    store.upsert_vector(1, MODEL, &[1.0, 0.0, 0.0])?;
    store.upsert_vector(2, MODEL, &[0.0, 1.0, 0.0])?;
    store.upsert_vector(3, MODEL, &[1.0, 0.0, 0.0])?;
    assert!(store.pending(MODEL)?.is_empty());
    assert_eq!(store.pending("other:model")?.len(), 3);

    let hits = store.nearest(MODEL, &[2.0, 0.1, 0.0], 3)?;
    let ids: Vec<i64> = hits.iter().map(|h| h.lesson.lesson_id).collect();
    assert_eq!(ids, [1, 3, 2]);
    assert!((hits[0].score - hits[1].score).abs() < 1e-12);
    assert!(hits[0].score > hits[2].score);
    // A query of another dimension sees nothing; the store never guesses.
    assert!(store.nearest(MODEL, &[1.0, 0.0], 3)?.is_empty());

    store.upsert_vector(2, MODEL, &[1.0, 0.0, 0.0])?;
    assert_eq!(
        store.nearest(MODEL, &[1.0, 0.0, 0.0], 1)?[0]
            .lesson
            .lesson_id,
        1
    );

    for bad in [
        vec![],
        vec![0.0, 0.0],
        vec![f32::NAN, 1.0],
        vec![f32::INFINITY],
    ] {
        assert!(matches!(
            store.upsert_vector(1, MODEL, &bad),
            Err(NeuralError::BadVector)
        ));
    }
    assert!(matches!(
        store.upsert_vector(9, MODEL, &[1.0]),
        Err(NeuralError::NoLesson(9))
    ));
    assert!(matches!(
        store.upsert_vector(1, "bad model", &[1.0]),
        Err(NeuralError::BadModel)
    ));
    assert!(matches!(
        store.nearest(MODEL, &[1.0, 0.0, 0.0], 0),
        Err(NeuralError::BadTopK)
    ));
    assert!(matches!(
        store.nearest(MODEL, &[1.0, 0.0, 0.0], TOP_K_MAX + 1),
        Err(NeuralError::BadTopK)
    ));
    drop(store);
    Ok(())
}

#[tokio::test]
async fn index_and_retrieve_through_a_loopback_runner() -> TestResult {
    let scratch = Scratch::new("runner")?;
    let mut store = LessonStore::open(&scratch.vault())?;
    seed(&mut store)?;
    let addr = fake_runner(Reply::Embeddings, 3).await?;
    assert_eq!(index_pending(&mut store, addr, MODEL, WAIT).await?, 3);
    assert_eq!(index_pending(&mut store, addr, MODEL, WAIT).await?, 0);

    let hits = retrieve(
        &store,
        addr,
        MODEL,
        "which fuel cap does a wasm module get",
        2,
        WAIT,
    )
    .await?;
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].lesson.title, "Fuel caps");
    assert!(hits[0].score > hits[1].score);
    let expected = cosine(
        &bag("which fuel cap does a wasm module get"),
        &bag(&hits[0].lesson.body),
    )
    .ok_or("zero vector")?;
    assert!((hits[0].score - expected).abs() < 1e-6);
    drop(store);
    Ok(())
}

#[tokio::test]
async fn runner_failures_are_typed_and_write_nothing() -> TestResult {
    let scratch = Scratch::new("down")?;
    let mut store = LessonStore::open(&scratch.vault())?;
    seed(&mut store)?;

    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        listener.local_addr()?
    };
    assert!(matches!(
        index_pending(&mut store, closed, MODEL, WAIT).await,
        Err(NeuralError::Embed(EmbedError::Unreachable))
    ));
    assert!(matches!(
        retrieve(&store, closed, MODEL, "fuel", 1, WAIT).await,
        Err(NeuralError::Embed(EmbedError::Unreachable))
    ));

    let missing = fake_runner(Reply::NotFound, 1).await?;
    match index_pending(&mut store, missing, MODEL, WAIT).await {
        Err(NeuralError::Embed(EmbedError::Status(404, detail))) => {
            assert!(detail.contains("not found"), "{detail}");
        }
        other => return Err(format!("expected a 404, got {other:?}").into()),
    }

    let short = fake_runner(Reply::Short, 1).await?;
    assert!(matches!(
        index_pending(&mut store, short, MODEL, WAIT).await,
        Err(NeuralError::EmbedCount { sent: 3, got: 2 })
    ));
    assert_eq!(store.pending(MODEL)?.len(), 3);

    let remote: SocketAddr = "192.0.2.1:11434".parse()?;
    assert_eq!(
        embed(remote, MODEL, &["x"], WAIT).await,
        Err(EmbedError::NotLoopback)
    );
    assert_eq!(
        embed(closed, MODEL, &[], WAIT).await,
        Err(EmbedError::BadInputs)
    );
    assert_eq!(
        embed(closed, "a b", &["x"], WAIT).await,
        Err(EmbedError::BadModel)
    );
    drop(store);
    Ok(())
}

#[test]
fn embeddings_parser_refuses_anything_but_numbers() {
    assert_eq!(
        parse_embeddings(br#"{"embeddings":[[1,2.5],[0,-1]]}"#),
        Ok(vec![vec![1.0, 2.5], vec![0.0, -1.0]])
    );
    for bad in [
        &br#"{"embedding":[1]}"#[..],
        br#"{"embeddings":[["1"]]}"#,
        br#"{"embeddings":[1]}"#,
        br#"{"embeddings":[[1e400]]}"#,
        b"not json",
    ] {
        assert_eq!(parse_embeddings(bad), Err(EmbedError::BadResponse));
    }
}

#[test]
fn default_runner_is_the_loopback_literal() {
    assert_eq!(RUNNER_ADDR.to_string(), "127.0.0.1:11434");
}

/// Operator check against the real local runner. Skipped by default; run with
/// `HCC_NEURAL_LIVE_MODEL=nomic-embed-text cargo test -p hcc-neural -- --ignored live_runner --nocapture`.
#[tokio::test]
#[ignore = "needs a local runner on 127.0.0.1:11434 and HCC_NEURAL_LIVE_MODEL"]
async fn live_runner_round_trip() -> TestResult {
    let model = std::env::var("HCC_NEURAL_LIVE_MODEL")?;
    let scratch = Scratch::new("live")?;
    let mut store = LessonStore::open(&scratch.vault())?;
    seed(&mut store)?;
    let wait = Duration::from_secs(300);
    let indexed = index_pending(&mut store, RUNNER_ADDR, &model, wait).await?;
    assert_eq!(indexed, 3);
    let hits = retrieve(
        &store,
        RUNNER_ADDR,
        &model,
        "how much fuel may a wasm module burn",
        3,
        wait,
    )
    .await?;
    for hit in &hits {
        println!("{:.4}  {}", hit.score, hit.lesson.title);
    }
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].lesson.title, "Fuel caps");
    drop(store);
    Ok(())
}

#[test]
fn upsert_vectors_is_all_or_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let scratch = std::env::temp_dir().join(format!("hcc-neural-batch-{}", std::process::id()));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch)?;
    }
    std::fs::create_dir_all(&scratch)?;
    let path = scratch.join("eduba_vault.db");
    {
        let mut store = LessonStore::open(&path)?;
        let a = store.store("vault.store.lesson", "a", "first body", &[1u8; 32])?;
        let b = store.store("vault.store.lesson", "b", "second body", &[1u8; 32])?;
        let good = [1.0f32, 0.0];
        let bad: [f32; 0] = [];
        assert!(matches!(
            store.upsert_vectors("m", &[(a.lesson_id, &good), (b.lesson_id, &bad)]),
            Err(NeuralError::BadVector)
        ));
        assert_eq!(
            store.pending("m")?.len(),
            2,
            "nothing written on a bad batch"
        );
        assert!(matches!(
            store.upsert_vectors("m", &[(a.lesson_id, &good), (99, &good)]),
            Err(NeuralError::NoLesson(99))
        ));
        assert_eq!(
            store.pending("m")?.len(),
            2,
            "rolled back on a missing lesson"
        );
        store.upsert_vectors("m", &[(a.lesson_id, &good), (b.lesson_id, &good)])?;
        assert!(store.pending("m")?.is_empty());
    }
    std::fs::remove_dir_all(&scratch)?;
    Ok(())
}
