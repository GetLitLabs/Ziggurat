//! BATON trace ledger (M10 `baton.trace.record`, `baton.state.replay`).
//!
//! Envelope, exactly as the charter freezes it:
//! `BLAKE3(BATON_DOMAIN || charter_digest || ns || terrace_u8 || prompt_hash || response_hash)`
//! where `prompt_hash = BLAKE3(prompt)` and `response_hash = BLAKE3(response)`.
//! Everything after `ns` is fixed-width, so the concatenation is unambiguous.
//!
//! Each ledger row's `prev_digest` is the previous row's envelope (32 zero bytes
//! first). Replay recomputes every envelope from the stored hashes and the sealed
//! charter digest, and checks links and sequence numbers.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::Path;

use hcc_seal::{Digest, BATON_DOMAIN};
use hcc_types::{BsmcNamespace, TerraceLevel};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

pub const GENESIS_PREV_DIGEST: Digest = [0u8; 32];

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS hcc_baton_trace_ledger (
  trace_id           INTEGER PRIMARY KEY AUTOINCREMENT,
  prev_digest        BLOB NOT NULL CHECK (length(prev_digest) = 32),
  module_namespace   TEXT NOT NULL CHECK (instr(module_namespace, '.') > 0),
  sequence_number    INTEGER NOT NULL,
  terrace_level      INTEGER NOT NULL CHECK (terrace_level BETWEEN 1 AND 5),
  input_state_hash   BLOB NOT NULL CHECK (length(input_state_hash) = 32),
  output_state_hash  BLOB NOT NULL CHECK (length(output_state_hash) = 32),
  baton_envelope_sig BLOB NOT NULL UNIQUE CHECK (length(baton_envelope_sig) = 32),
  recorded_at_utc    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TRIGGER IF NOT EXISTS hcc_baton_trace_ledger_no_update
BEFORE UPDATE ON hcc_baton_trace_ledger
BEGIN SELECT RAISE(ABORT, 'hcc_baton_trace_ledger is append-only'); END;
CREATE TRIGGER IF NOT EXISTS hcc_baton_trace_ledger_no_delete
BEFORE DELETE ON hcc_baton_trace_ledger
BEGIN SELECT RAISE(ABORT, 'hcc_baton_trace_ledger is append-only'); END;
CREATE TRIGGER IF NOT EXISTS hcc_baton_trace_ledger_chain_link
BEFORE INSERT ON hcc_baton_trace_ledger
WHEN NEW.prev_digest IS NOT COALESCE(
  (SELECT baton_envelope_sig FROM hcc_baton_trace_ledger ORDER BY trace_id DESC LIMIT 1),
  zeroblob(32))
  OR NEW.sequence_number IS NOT 1 + COALESCE(
  (SELECT sequence_number FROM hcc_baton_trace_ledger ORDER BY trace_id DESC LIMIT 1), 0)
BEGIN SELECT RAISE(ABORT, 'trace does not link to the ledger head'); END;
";

/// The frozen BATON envelope.
pub fn baton_envelope(
    charter_digest: &Digest,
    namespace: &BsmcNamespace<'_>,
    terrace: TerraceLevel,
    prompt_hash: &Digest,
    response_hash: &Digest,
) -> Digest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(BATON_DOMAIN);
    hasher.update(charter_digest);
    hasher.update(namespace.as_str().as_bytes());
    hasher.update(&[terrace.as_u8()]);
    hasher.update(prompt_hash);
    hasher.update(response_hash);
    *hasher.finalize().as_bytes()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    pub trace_id: i64,
    pub prev_digest: Digest,
    pub module_namespace: String,
    pub sequence_number: i64,
    pub terrace_level: TerraceLevel,
    pub input_state_hash: Digest,
    pub output_state_hash: Digest,
    pub envelope: Digest,
    pub recorded_at_utc: String,
}

#[derive(Debug)]
pub enum BatonError {
    Sql(rusqlite::Error),
    BadNamespace,
    BadTerrace(i64),
    Corrupt { trace_id: i64, column: &'static str },
    ReplayMismatch { trace_id: i64 },
}

impl fmt::Display for BatonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(err) => write!(f, "sqlite: {err}"),
            Self::BadNamespace => f.write_str("module_namespace must contain a dot"),
            Self::BadTerrace(level) => write!(f, "terrace_level {level} is outside 1..=5"),
            Self::Corrupt { trace_id, column } => {
                write!(f, "trace {trace_id}: column {column} has the wrong size")
            }
            Self::ReplayMismatch { trace_id } => {
                write!(f, "BATON replay diverges at trace {trace_id}")
            }
        }
    }
}

impl std::error::Error for BatonError {}

impl From<rusqlite::Error> for BatonError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sql(err)
    }
}

fn digest(trace_id: i64, column: &'static str, bytes: Vec<u8>) -> Result<Digest, BatonError> {
    bytes
        .try_into()
        .map_err(|_| BatonError::Corrupt { trace_id, column })
}

const COLUMNS: &str = "trace_id, prev_digest, module_namespace, sequence_number, terrace_level, \
     input_state_hash, output_state_hash, baton_envelope_sig, recorded_at_utc";

type RawTrace = (
    i64,
    Vec<u8>,
    String,
    i64,
    i64,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    String,
);

fn raw(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawTrace> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
    ))
}

fn decode(raw: RawTrace) -> Result<Trace, BatonError> {
    let (
        trace_id,
        prev,
        module_namespace,
        sequence_number,
        level,
        input,
        output,
        envelope,
        recorded_at_utc,
    ) = raw;
    let terrace_level = u8::try_from(level)
        .ok()
        .and_then(|l| TerraceLevel::try_from(l).ok())
        .ok_or(BatonError::BadTerrace(level))?;
    Ok(Trace {
        trace_id,
        prev_digest: digest(trace_id, "prev_digest", prev)?,
        module_namespace,
        sequence_number,
        terrace_level,
        input_state_hash: digest(trace_id, "input_state_hash", input)?,
        output_state_hash: digest(trace_id, "output_state_hash", output)?,
        envelope: digest(trace_id, "baton_envelope_sig", envelope)?,
        recorded_at_utc,
    })
}

pub struct BatonLedger {
    conn: Connection,
}

impl BatonLedger {
    /// Open (or create) the ledger inside the Eduba vault file, in WAL mode.
    pub fn open(path: &Path) -> Result<Self, BatonError> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, BatonError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, BatonError> {
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn last(&self) -> Result<Option<Trace>, BatonError> {
        let sql =
            format!("SELECT {COLUMNS} FROM hcc_baton_trace_ledger ORDER BY trace_id DESC LIMIT 1");
        match self.conn.query_row(&sql, [], raw).optional()? {
            Some(row) => Ok(Some(decode(row)?)),
            None => Ok(None),
        }
    }

    /// `baton.trace.record`: hash prompt and response, compute the envelope, chain it on.
    pub fn record(
        &mut self,
        charter_digest: &Digest,
        namespace: &str,
        terrace: TerraceLevel,
        prompt: &[u8],
        response: &[u8],
    ) -> Result<Trace, BatonError> {
        let ns = BsmcNamespace::parse(namespace).map_err(|_| BatonError::BadNamespace)?;
        let prompt_hash = *blake3::hash(prompt).as_bytes();
        let response_hash = *blake3::hash(response).as_bytes();
        let envelope = baton_envelope(charter_digest, &ns, terrace, &prompt_hash, &response_hash);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let head: Option<(Vec<u8>, i64)> = tx
            .query_row(
                "SELECT baton_envelope_sig, sequence_number FROM hcc_baton_trace_ledger
                 ORDER BY trace_id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (prev_digest, sequence_number) = match head {
            Some((bytes, seq)) => (digest(0, "baton_envelope_sig", bytes)?, seq + 1),
            None => (GENESIS_PREV_DIGEST, 1),
        };
        tx.execute(
            "INSERT INTO hcc_baton_trace_ledger
               (prev_digest, module_namespace, sequence_number, terrace_level,
                input_state_hash, output_state_hash, baton_envelope_sig)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &prev_digest[..],
                namespace,
                sequence_number,
                i64::from(terrace.as_u8()),
                &prompt_hash[..],
                &response_hash[..],
                &envelope[..],
            ],
        )?;
        let trace_id = tx.last_insert_rowid();
        tx.commit()?;
        let sql = format!("SELECT {COLUMNS} FROM hcc_baton_trace_ledger WHERE trace_id = ?1");
        decode(self.conn.query_row(&sql, params![trace_id], raw)?)
    }

    /// `baton.state.replay`: recompute every envelope under `charter_digest`. Returns the count.
    pub fn replay(&self, charter_digest: &Digest) -> Result<u64, BatonError> {
        let sql = format!("SELECT {COLUMNS} FROM hcc_baton_trace_ledger ORDER BY trace_id ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut expected_prev = GENESIS_PREV_DIGEST;
        let mut count: u64 = 0;
        while let Some(r) = rows.next()? {
            let trace = decode(raw(r)?)?;
            let mismatch = BatonError::ReplayMismatch {
                trace_id: trace.trace_id,
            };
            let ns = BsmcNamespace::parse(&trace.module_namespace)
                .map_err(|_| BatonError::BadNamespace)?;
            let recomputed = baton_envelope(
                charter_digest,
                &ns,
                trace.terrace_level,
                &trace.input_state_hash,
                &trace.output_state_hash,
            );
            count += 1;
            if trace.prev_digest != expected_prev
                || u64::try_from(trace.sequence_number).ok() != Some(count)
                || trace.envelope != recomputed
            {
                return Err(mismatch);
            }
            expected_prev = trace.envelope;
        }
        Ok(count)
    }

    #[cfg(test)]
    fn connection(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests;
