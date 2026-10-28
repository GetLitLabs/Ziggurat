//! Neural Highway (M01 `neural.index.upsert`, `neural.retrieve.lesson`) and the lesson store
//! (M03 `vault.store.lesson`).
//!
//! Lessons live in the vault file next to the stele, append-only; storing one chains an
//! `HCC-LESSON-v1` row (hashes only) onto the stele in the same transaction. Vectors come from
//! the local runner's `/api/embed` on a loopback literal, are kept per model as little-endian
//! f32, and are searched by exact brute-force cosine. No ANN index, no latency claims.
//! Runner down means a typed `EmbedError`; nothing is retrieved from vectors that do not exist.
#![forbid(unsafe_code)]

pub mod embed;

use std::fmt;
use std::path::Path;

use hcc_iedb_stele::{append_in, SteleError};
use hcc_seal::{digest_hex, Digest};
use hcc_types::BsmcNamespace;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

pub use embed::{embed, EmbedError, RUNNER_ADDR};

/// Namespace of the stele row that records a stored lesson.
pub const LESSON_NAMESPACE: &str = "vault.store.lesson";
pub const TITLE_MAX_CHARS: usize = 200;
pub const BODY_MAX_BYTES: usize = 64 * 1024;
pub const MODEL_MAX: usize = 128;
pub const DIM_MAX: usize = 16_384;
pub const TOP_K_MAX: usize = 64;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS hcc_neural_lesson (
  lesson_id        INTEGER PRIMARY KEY AUTOINCREMENT,
  module_namespace TEXT NOT NULL CHECK (instr(module_namespace, '.') > 0),
  title            TEXT NOT NULL,
  body             TEXT NOT NULL,
  body_blake3      BLOB NOT NULL UNIQUE CHECK (length(body_blake3) = 32),
  stele_id         INTEGER NOT NULL,
  stored_at_utc    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TRIGGER IF NOT EXISTS hcc_neural_lesson_no_update
BEFORE UPDATE ON hcc_neural_lesson
BEGIN SELECT RAISE(ABORT, 'hcc_neural_lesson is append-only'); END;
CREATE TRIGGER IF NOT EXISTS hcc_neural_lesson_no_delete
BEFORE DELETE ON hcc_neural_lesson
BEGIN SELECT RAISE(ABORT, 'hcc_neural_lesson is append-only'); END;
CREATE TABLE IF NOT EXISTS hcc_neural_index (
  lesson_id   INTEGER NOT NULL REFERENCES hcc_neural_lesson(lesson_id),
  model       TEXT NOT NULL,
  dim         INTEGER NOT NULL CHECK (dim BETWEEN 1 AND 16384),
  vector      BLOB NOT NULL CHECK (length(vector) = 4 * dim),
  body_blake3 BLOB NOT NULL CHECK (length(body_blake3) = 32),
  indexed_at_utc TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (lesson_id, model)
);
";

#[derive(Debug)]
pub enum NeuralError {
    Sql(rusqlite::Error),
    Stele(SteleError),
    Embed(EmbedError),
    BadNamespace,
    BadTitle,
    BadBody,
    BadModel,
    /// Empty, too long, non-finite or all-zero vector.
    BadVector,
    BadTopK,
    Duplicate {
        lesson_id: i64,
    },
    NoLesson(i64),
    /// The runner returned a different number of vectors than texts sent.
    EmbedCount {
        sent: usize,
        got: usize,
    },
    Corrupt {
        lesson_id: i64,
        column: &'static str,
    },
}

impl fmt::Display for NeuralError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(err) => write!(f, "sqlite: {err}"),
            Self::Stele(err) => write!(f, "stele: {err}"),
            Self::Embed(err) => write!(f, "embed: {err}"),
            Self::BadNamespace => f.write_str("bsmc_namespace must be dot-separated"),
            Self::BadTitle => write!(
                f,
                "title must be 1..={TITLE_MAX_CHARS} characters, no control characters"
            ),
            Self::BadBody => write!(f, "body must be 1..={BODY_MAX_BYTES} bytes of UTF-8"),
            Self::BadModel => write!(f, "model must be 1..={MODEL_MAX} of [A-Za-z0-9._:/-]"),
            Self::BadVector => write!(
                f,
                "vector must be 1..={DIM_MAX} finite floats, not all zero"
            ),
            Self::BadTopK => write!(f, "k must be 1..={TOP_K_MAX}"),
            Self::Duplicate { lesson_id } => {
                write!(
                    f,
                    "an identical lesson body is already stored as lesson {lesson_id}"
                )
            }
            Self::NoLesson(id) => write!(f, "no lesson {id}"),
            Self::EmbedCount { sent, got } => {
                write!(f, "runner returned {got} embeddings for {sent} inputs")
            }
            Self::Corrupt { lesson_id, column } => {
                write!(f, "lesson {lesson_id}: column {column} is malformed")
            }
        }
    }
}

impl std::error::Error for NeuralError {}

impl From<rusqlite::Error> for NeuralError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sql(err)
    }
}

impl From<EmbedError> for NeuralError {
    fn from(err: EmbedError) -> Self {
        Self::Embed(err)
    }
}

/// Model names go into a JSON body and an index key: 1..=128 of `[A-Za-z0-9._:/-]`.
pub fn model_ok(model: &str) -> bool {
    (1..=MODEL_MAX).contains(&model.len())
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
}

fn title_ok(title: &str) -> bool {
    let n = title.chars().count();
    (1..=TITLE_MAX_CHARS).contains(&n) && !title.chars().any(char::is_control)
}

/// Canonical stele payload for a stored lesson: hashes and ids only, never the text.
pub fn lesson_payload(
    lesson_id: i64,
    namespace: &str,
    title: &str,
    body_blake3: &Digest,
) -> Vec<u8> {
    format!(
        "HCC-LESSON-v1\nlesson_id={lesson_id}\nbsmc_namespace={namespace}\ntitle_blake3_hex={}\nbody_blake3_hex={}\n",
        digest_hex(blake3::hash(title.as_bytes()).as_bytes()),
        digest_hex(body_blake3)
    )
    .into_bytes()
}

fn vector_ok(vector: &[f32]) -> bool {
    (1..=DIM_MAX).contains(&vector.len())
        && vector.iter().all(|x| x.is_finite())
        && vector.iter().any(|x| *x != 0.0)
}

fn to_blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(lesson_id: i64, blob: &[u8]) -> Result<Vec<f32>, NeuralError> {
    let (chunks, rest) = blob.as_chunks::<4>();
    if !rest.is_empty() {
        return Err(NeuralError::Corrupt {
            lesson_id,
            column: "vector",
        });
    }
    Ok(chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

/// Exact cosine similarity, accumulated in f64. `None` when lengths differ or a norm is zero.
pub fn cosine(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    Some(dot / (na.sqrt() * nb.sqrt()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lesson {
    pub lesson_id: i64,
    pub module_namespace: String,
    pub title: String,
    pub body: String,
    pub body_blake3: Digest,
    pub stele_id: i64,
    pub stored_at_utc: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub lesson: Lesson,
    pub score: f64,
}

const LESSON_COLUMNS: &str =
    "lesson_id, module_namespace, title, body, body_blake3, stele_id, stored_at_utc";

/// `(lesson_id, module_namespace, title, body, body_blake3, stele_id, stored_at_utc)`.
type RawLesson = (i64, String, String, String, Vec<u8>, i64, String);

fn lesson_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawLesson> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
    ))
}

fn decode(
    (lesson_id, module_namespace, title, body, hash, stele_id, stored_at_utc): RawLesson,
) -> Result<Lesson, NeuralError> {
    let body_blake3: Digest = hash.try_into().map_err(|_| NeuralError::Corrupt {
        lesson_id,
        column: "body_blake3",
    })?;
    if *blake3::hash(body.as_bytes()).as_bytes() != body_blake3 {
        return Err(NeuralError::Corrupt {
            lesson_id,
            column: "body",
        });
    }
    Ok(Lesson {
        lesson_id,
        module_namespace,
        title,
        body,
        body_blake3,
        stele_id,
        stored_at_utc,
    })
}

/// Lessons and their vectors in a vault file.
pub struct LessonStore {
    conn: Connection,
}

impl LessonStore {
    /// Open the lesson tables inside the vault file at `path` (the same file as the stele).
    pub fn open(path: &Path) -> Result<Self, NeuralError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(hcc_iedb_stele::BUSY_TIMEOUT)?;
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        // Storing a lesson chains a stele row, so the stele table must exist here too.
        drop(hcc_iedb_stele::Stele::open(path).map_err(NeuralError::Stele)?);
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// `vault.store.lesson`: append one lesson and its stele row atomically.
    pub fn store(
        &mut self,
        namespace: &str,
        title: &str,
        body: &str,
        ed25519_pubkey: &[u8; 32],
    ) -> Result<Lesson, NeuralError> {
        let namespace = BsmcNamespace::parse(namespace).map_err(|_| NeuralError::BadNamespace)?;
        if !title_ok(title) {
            return Err(NeuralError::BadTitle);
        }
        if body.is_empty() || body.len() > BODY_MAX_BYTES {
            return Err(NeuralError::BadBody);
        }
        let body_blake3 = *blake3::hash(body.as_bytes()).as_bytes();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT lesson_id FROM hcc_neural_lesson WHERE body_blake3 = ?1",
                params![&body_blake3[..]],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(lesson_id) = existing {
            return Err(NeuralError::Duplicate { lesson_id });
        }
        let next_id: i64 = tx.query_row(
            "SELECT COALESCE(MAX(lesson_id), 0) + 1 FROM hcc_neural_lesson",
            [],
            |r| r.get(0),
        )?;
        let payload = lesson_payload(next_id, namespace.as_str(), title, &body_blake3);
        let stele_id = append_in(&tx, LESSON_NAMESPACE, &payload, ed25519_pubkey, None)
            .map_err(NeuralError::Stele)?;
        tx.execute(
            "INSERT INTO hcc_neural_lesson (lesson_id, module_namespace, title, body, body_blake3, stele_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![next_id, namespace.as_str(), title, body, &body_blake3[..], stele_id],
        )?;
        tx.commit()?;
        self.lesson(next_id)?.ok_or(NeuralError::NoLesson(next_id))
    }

    pub fn lesson(&self, lesson_id: i64) -> Result<Option<Lesson>, NeuralError> {
        let sql = format!("SELECT {LESSON_COLUMNS} FROM hcc_neural_lesson WHERE lesson_id = ?1");
        match self
            .conn
            .query_row(&sql, params![lesson_id], lesson_row)
            .optional()?
        {
            Some(raw) => Ok(Some(decode(raw)?)),
            None => Ok(None),
        }
    }

    pub fn len(&self) -> Result<u64, NeuralError> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM hcc_neural_lesson", [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    pub fn is_empty(&self) -> Result<bool, NeuralError> {
        Ok(self.len()? == 0)
    }

    /// Lessons with no vector for `model`, or whose vector was made from different bytes.
    pub fn pending(&self, model: &str) -> Result<Vec<Lesson>, NeuralError> {
        if !model_ok(model) {
            return Err(NeuralError::BadModel);
        }
        let sql = format!(
            "SELECT {LESSON_COLUMNS} FROM hcc_neural_lesson l
             WHERE NOT EXISTS (SELECT 1 FROM hcc_neural_index i
                               WHERE i.lesson_id = l.lesson_id AND i.model = ?1
                                 AND i.body_blake3 = l.body_blake3)
             ORDER BY lesson_id ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![model], lesson_row)?;
        rows.map(|r| decode(r?)).collect()
    }

    /// `neural.index.upsert`: set the vector for one lesson under one model.
    pub fn upsert_vector(
        &mut self,
        lesson_id: i64,
        model: &str,
        vector: &[f32],
    ) -> Result<(), NeuralError> {
        self.upsert_vectors(model, &[(lesson_id, vector)])
    }

    /// Set several vectors under one model in one transaction: every vector is checked first,
    /// and any error writes none of them.
    pub fn upsert_vectors(
        &mut self,
        model: &str,
        vectors: &[(i64, &[f32])],
    ) -> Result<(), NeuralError> {
        if !model_ok(model) {
            return Err(NeuralError::BadModel);
        }
        if vectors.iter().any(|(_, v)| !vector_ok(v)) {
            return Err(NeuralError::BadVector);
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (lesson_id, vector) in vectors {
            let sql =
                format!("SELECT {LESSON_COLUMNS} FROM hcc_neural_lesson WHERE lesson_id = ?1");
            let lesson = match tx
                .query_row(&sql, params![lesson_id], lesson_row)
                .optional()?
            {
                Some(raw) => decode(raw)?,
                None => return Err(NeuralError::NoLesson(*lesson_id)),
            };
            let dim = i64::try_from(vector.len()).map_err(|_| NeuralError::BadVector)?;
            tx.execute(
                "INSERT INTO hcc_neural_index (lesson_id, model, dim, vector, body_blake3)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (lesson_id, model) DO UPDATE SET
                   dim = excluded.dim, vector = excluded.vector, body_blake3 = excluded.body_blake3,
                   indexed_at_utc = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
                params![
                    lesson_id,
                    model,
                    dim,
                    to_blob(vector),
                    &lesson.body_blake3[..]
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Exact top-`k` by cosine over every vector stored for `model` with the query's dimension.
    /// Ties break by lower `lesson_id`, so the order is deterministic.
    pub fn nearest(&self, model: &str, query: &[f32], k: usize) -> Result<Vec<Hit>, NeuralError> {
        if !model_ok(model) {
            return Err(NeuralError::BadModel);
        }
        if !vector_ok(query) {
            return Err(NeuralError::BadVector);
        }
        if !(1..=TOP_K_MAX).contains(&k) {
            return Err(NeuralError::BadTopK);
        }
        let dim = i64::try_from(query.len()).map_err(|_| NeuralError::BadVector)?;
        let mut stmt = self.conn.prepare(
            "SELECT lesson_id, vector FROM hcc_neural_index
             WHERE model = ?1 AND dim = ?2 ORDER BY lesson_id ASC",
        )?;
        let rows = stmt.query_map(params![model, dim], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut scored: Vec<(i64, f64)> = Vec::new();
        for row in rows {
            let (lesson_id, blob) = row?;
            let vector = from_blob(lesson_id, &blob)?;
            if let Some(score) = cosine(query, &vector) {
                scored.push((lesson_id, score));
            }
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(k);
        scored
            .into_iter()
            .map(|(lesson_id, score)| {
                let lesson = self
                    .lesson(lesson_id)?
                    .ok_or(NeuralError::NoLesson(lesson_id))?;
                Ok(Hit { lesson, score })
            })
            .collect()
    }

    #[cfg(test)]
    fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// `neural.index.upsert` against the runner: embed every pending lesson for `model`.
/// Returns how many vectors were written. Nothing is written if the runner fails.
pub async fn index_pending(
    store: &mut LessonStore,
    addr: std::net::SocketAddr,
    model: &str,
    timeout: std::time::Duration,
) -> Result<usize, NeuralError> {
    let pending = store.pending(model)?;
    if pending.is_empty() {
        return Ok(0);
    }
    let texts: Vec<&str> = pending.iter().map(|l| l.body.as_str()).collect();
    let vectors = embed(addr, model, &texts, timeout).await?;
    if vectors.len() != pending.len() {
        return Err(NeuralError::EmbedCount {
            sent: pending.len(),
            got: vectors.len(),
        });
    }
    for (lesson, vector) in pending.iter().zip(&vectors) {
        store.upsert_vector(lesson.lesson_id, model, vector)?;
    }
    Ok(pending.len())
}

/// `neural.retrieve.lesson`: embed `query` with `model` and return the exact top-`k` lessons.
pub async fn retrieve(
    store: &LessonStore,
    addr: std::net::SocketAddr,
    model: &str,
    query: &str,
    k: usize,
    timeout: std::time::Duration,
) -> Result<Vec<Hit>, NeuralError> {
    if query.is_empty() || query.len() > BODY_MAX_BYTES {
        return Err(NeuralError::BadBody);
    }
    let mut vectors = embed(addr, model, &[query], timeout).await?;
    if vectors.len() != 1 {
        return Err(NeuralError::EmbedCount {
            sent: 1,
            got: vectors.len(),
        });
    }
    let vector = vectors
        .pop()
        .ok_or(NeuralError::EmbedCount { sent: 1, got: 0 })?;
    store.nearest(model, &vector, k)
}

#[cfg(test)]
mod tests;
