//! Eduba stele (M03 `vault.*`, M02 `accordion.persist.sqlite`).
//!
//! Append-only ledger in SQLite WAL. Each row's digest is
//! `BLAKE3(STELE_DOMAIN || prev_digest || payload)` and its `prev_digest` is the
//! previous row's digest (32 zero bytes for genesis). The chain is the proof;
//! the UPDATE/DELETE abort triggers are a seatbelt.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::Path;

use hcc_seal::{
    digest_hex, verify_royal_seal, Digest, RoyalSeal, RoyalSealed, SealError, SystemCharter,
    STELE_DOMAIN,
};
use hcc_types::BsmcNamespace;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

pub const GENESIS_PREV_DIGEST: Digest = [0u8; 32];
pub const GENESIS_NAMESPACE: &str = "ziggurat.daemon.boot";
pub const VERSION_TAG: &str = "1.0.0";
/// M07 `genesis.charter.version`: rows in this namespace move the vault to a new sealed charter.
/// Only [`Stele::record_charter_version`] may write it.
pub const CHARTER_VERSION_NAMESPACE: &str = "genesis.charter.version";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS hcc_precedent_stele (
  stele_id         INTEGER PRIMARY KEY AUTOINCREMENT,
  prev_digest      BLOB NOT NULL CHECK (length(prev_digest) = 32),
  module_namespace TEXT NOT NULL CHECK (instr(module_namespace, '.') > 0),
  version_tag      TEXT NOT NULL,
  domain_separator BLOB NOT NULL CHECK (domain_separator = X'4843432D5354454C4C452D763200'),
  payload          BLOB NOT NULL,
  payload_byte_len INTEGER NOT NULL CHECK (payload_byte_len = length(payload)),
  blake3_digest    BLOB NOT NULL UNIQUE CHECK (length(blake3_digest) = 32),
  ed25519_pubkey   BLOB NOT NULL CHECK (length(ed25519_pubkey) = 32),
  royal_seal_sig   BLOB CHECK (royal_seal_sig IS NULL OR length(royal_seal_sig) = 64),
  committed_at_utc TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TRIGGER IF NOT EXISTS hcc_precedent_stele_no_update
BEFORE UPDATE ON hcc_precedent_stele
BEGIN SELECT RAISE(ABORT, 'hcc_precedent_stele is append-only'); END;
CREATE TRIGGER IF NOT EXISTS hcc_precedent_stele_no_delete
BEFORE DELETE ON hcc_precedent_stele
BEGIN SELECT RAISE(ABORT, 'hcc_precedent_stele is append-only'); END;
CREATE TRIGGER IF NOT EXISTS hcc_precedent_stele_chain_link
BEFORE INSERT ON hcc_precedent_stele
WHEN NEW.prev_digest IS NOT COALESCE(
  (SELECT blake3_digest FROM hcc_precedent_stele ORDER BY stele_id DESC LIMIT 1),
  zeroblob(32))
BEGIN SELECT RAISE(ABORT, 'prev_digest does not link to the chain head'); END;
";

/// `BLAKE3(STELE_DOMAIN || prev_digest || payload)`.
pub fn stele_digest(prev_digest: &Digest, payload: &[u8]) -> Digest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(STELE_DOMAIN);
    hasher.update(prev_digest);
    hasher.update(payload);
    *hasher.finalize().as_bytes()
}

/// Canonical genesis payload: the constitution the ledger was opened under.
pub fn genesis_payload(charter_byte_len: usize, charter_digest: &Digest) -> Vec<u8> {
    format!(
        "HCC-GENESIS-v1\nziggurat_version={VERSION_TAG}\ncharter_byte_len={charter_byte_len}\ncharter_blake3_hex={}\n",
        digest_hex(charter_digest)
    )
    .into_bytes()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteleRow {
    pub stele_id: i64,
    pub prev_digest: Digest,
    pub module_namespace: String,
    pub version_tag: String,
    pub payload: Vec<u8>,
    pub blake3_digest: Digest,
    pub ed25519_pubkey: [u8; 32],
    pub royal_seal_sig: Option<[u8; 64]>,
    pub committed_at_utc: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Genesis {
    Created(SteleRow),
    Present(SteleRow),
}

impl Genesis {
    pub fn row(&self) -> &SteleRow {
        match self {
            Self::Created(row) | Self::Present(row) => row,
        }
    }
}

#[derive(Debug)]
pub enum SteleError {
    Sql(rusqlite::Error),
    Corrupt {
        stele_id: i64,
        column: &'static str,
    },
    ChainBroken {
        stele_id: i64,
    },
    GenesisMismatch,
    BadNamespace,
    Seal(SealError),
    /// `genesis.charter.version` is written only by `record_charter_version`.
    ReservedNamespace,
    /// A charter-version row (or the genesis row it builds on) does not hold up.
    BadCharterVersion {
        stele_id: i64,
        why: &'static str,
    },
    /// The vault is already on this charter.
    AlreadyCurrent,
}

impl fmt::Display for SteleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(err) => write!(f, "sqlite: {err}"),
            Self::Corrupt { stele_id, column } => {
                write!(
                    f,
                    "stele row {stele_id}: column {column} has the wrong size"
                )
            }
            Self::ChainBroken { stele_id } => {
                write!(f, "stele chain broken at row {stele_id}")
            }
            Self::GenesisMismatch => f.write_str(
                "stele charter lineage ends at a different charter than the one booting",
            ),
            Self::BadNamespace => f.write_str("module_namespace must contain a dot"),
            Self::Seal(err) => write!(f, "seal: {err}"),
            Self::ReservedNamespace => write!(
                f,
                "{CHARTER_VERSION_NAMESPACE} rows are written only by record_charter_version"
            ),
            Self::BadCharterVersion { stele_id, why } => {
                write!(f, "charter lineage broken at stele row {stele_id}: {why}")
            }
            Self::AlreadyCurrent => f.write_str("the vault is already on this charter"),
        }
    }
}

impl std::error::Error for SteleError {}

impl From<rusqlite::Error> for SteleError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sql(err)
    }
}

fn fixed<const N: usize>(
    stele_id: i64,
    column: &'static str,
    bytes: Vec<u8>,
) -> Result<[u8; N], SteleError> {
    bytes
        .try_into()
        .map_err(|_| SteleError::Corrupt { stele_id, column })
}

const ROW_COLUMNS: &str = "stele_id, prev_digest, module_namespace, version_tag, payload, \
     blake3_digest, ed25519_pubkey, royal_seal_sig, committed_at_utc, domain_separator, payload_byte_len";

struct RawRow {
    row: SteleRow,
    domain_separator: Vec<u8>,
    payload_byte_len: i64,
}

fn raw_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawTuple> {
    Ok((
        r.get(0)?,
        [r.get(1)?, r.get(4)?, r.get(5)?, r.get(6)?],
        [r.get(2)?, r.get(3)?, r.get(8)?],
        r.get(9)?,
        r.get(7)?,
        r.get(10)?,
    ))
}

type RawTuple = (
    i64,
    [Vec<u8>; 4],
    [String; 3],
    Vec<u8>,
    Option<Vec<u8>>,
    i64,
);

fn decode(
    (stele_id, blobs, texts, domain_separator, sig, payload_byte_len): RawTuple,
) -> Result<RawRow, SteleError> {
    let [prev, payload, digest, pubkey] = blobs;
    let [module_namespace, version_tag, committed_at_utc] = texts;
    let royal_seal_sig = match sig {
        Some(sig) => Some(fixed::<64>(stele_id, "royal_seal_sig", sig)?),
        None => None,
    };
    Ok(RawRow {
        row: SteleRow {
            stele_id,
            prev_digest: fixed(stele_id, "prev_digest", prev)?,
            module_namespace,
            version_tag,
            payload,
            blake3_digest: fixed(stele_id, "blake3_digest", digest)?,
            ed25519_pubkey: fixed(stele_id, "ed25519_pubkey", pubkey)?,
            royal_seal_sig,
            committed_at_utc,
        },
        domain_separator,
        payload_byte_len,
    })
}

/// An open Eduba stele.
pub struct Stele {
    conn: Connection,
}

impl Stele {
    /// Open (or create) a file-backed stele in WAL mode.
    pub fn open(path: &Path) -> Result<Self, SteleError> {
        Self::init(Connection::open(path)?)
    }

    /// In-memory stele for tests. SQLite reports journal mode `memory` here.
    pub fn open_in_memory() -> Result<Self, SteleError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, SteleError> {
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn journal_mode(&self) -> Result<String, SteleError> {
        Ok(self
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))?)
    }

    pub fn len(&self) -> Result<u64, SteleError> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM hcc_precedent_stele", [], |r| r.get(0))?;
        u64::try_from(n).map_err(|_| SteleError::Corrupt {
            stele_id: 0,
            column: "count",
        })
    }

    pub fn is_empty(&self) -> Result<bool, SteleError> {
        Ok(self.len()? == 0)
    }

    pub fn first(&self) -> Result<Option<SteleRow>, SteleError> {
        self.one("ORDER BY stele_id ASC LIMIT 1")
    }

    pub fn last(&self) -> Result<Option<SteleRow>, SteleError> {
        self.one("ORDER BY stele_id DESC LIMIT 1")
    }

    pub fn row(&self, stele_id: i64) -> Result<Option<SteleRow>, SteleError> {
        let sql = format!("SELECT {ROW_COLUMNS} FROM hcc_precedent_stele WHERE stele_id = ?1");
        match self
            .conn
            .query_row(&sql, params![stele_id], raw_row)
            .optional()?
        {
            Some(raw) => Ok(Some(decode(raw)?.row)),
            None => Ok(None),
        }
    }

    fn one(&self, order: &str) -> Result<Option<SteleRow>, SteleError> {
        let sql = format!("SELECT {ROW_COLUMNS} FROM hcc_precedent_stele {order}");
        match self.conn.query_row(&sql, [], raw_row).optional()? {
            Some(raw) => Ok(Some(decode(raw)?.row)),
            None => Ok(None),
        }
    }

    /// `scribe.stele.append`: chain `payload` onto the head inside one IMMEDIATE transaction.
    pub fn append(
        &mut self,
        module_namespace: &str,
        payload: &[u8],
        ed25519_pubkey: &[u8; 32],
        royal_seal_sig: Option<&[u8; 64]>,
    ) -> Result<SteleRow, SteleError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stele_id = append_in(
            &tx,
            module_namespace,
            payload,
            ed25519_pubkey,
            royal_seal_sig,
        )?;
        tx.commit()?;
        self.row(stele_id)?
            .ok_or(SteleError::ChainBroken { stele_id })
    }

    /// Recompute every digest and link from genesis. Returns the row count.
    pub fn verify_chain(&self) -> Result<u64, SteleError> {
        let sql = format!("SELECT {ROW_COLUMNS} FROM hcc_precedent_stele ORDER BY stele_id ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut expected_prev = GENESIS_PREV_DIGEST;
        let mut count = 0u64;
        while let Some(r) = rows.next()? {
            let raw = decode(raw_row(r)?)?;
            let row = &raw.row;
            let len_ok = usize::try_from(raw.payload_byte_len).ok() == Some(row.payload.len());
            if row.prev_digest != expected_prev
                || raw.domain_separator != STELE_DOMAIN[..]
                || !len_ok
                || row.blake3_digest != stele_digest(&row.prev_digest, &row.payload)
            {
                return Err(SteleError::ChainBroken {
                    stele_id: row.stele_id,
                });
            }
            expected_prev = row.blake3_digest;
            count += 1;
        }
        Ok(count)
    }

    /// Append the genesis row for a royal-sealed charter, or confirm the existing one
    /// was written under this same charter.
    pub fn ensure_genesis(
        &mut self,
        charter: &SystemCharter<RoyalSealed>,
        royal: &RoyalSeal,
    ) -> Result<Genesis, SteleError> {
        verify_royal_seal(royal, charter.digest()).map_err(SteleError::Seal)?;
        if let Some(first) = self.first()? {
            let lineage = self.lineage()?;
            let current = lineage
                .last()
                .is_some_and(|tip| tip.matches(charter.byte_len(), charter.digest()));
            return if current {
                Ok(Genesis::Present(first))
            } else {
                Err(SteleError::GenesisMismatch)
            };
        }
        let payload = genesis_payload(charter.byte_len(), charter.digest());
        let row = self.append(
            GENESIS_NAMESPACE,
            &payload,
            &royal.operator_pubkey,
            Some(&royal.signature),
        )?;
        Ok(Genesis::Created(row))
    }

    /// True when row 1 is a well-formed genesis that embeds this charter's length and digest.
    pub fn genesis_matches(
        &self,
        charter_byte_len: usize,
        charter_digest: &Digest,
    ) -> Result<bool, SteleError> {
        Ok(match self.first()? {
            None => false,
            Some(row) => {
                row.prev_digest == GENESIS_PREV_DIGEST
                    && row.module_namespace == GENESIS_NAMESPACE
                    && row.payload == genesis_payload(charter_byte_len, charter_digest)
                    && row.blake3_digest == stele_digest(&row.prev_digest, &row.payload)
            }
        })
    }

    /// Every charter the vault has run under, oldest first: the genesis charter, then one epoch
    /// per `genesis.charter.version` row. Each row must be canonical, continue from the previous
    /// tip to a strictly higher version, and carry the genesis operator's Ed25519 signature over
    /// its new charter digest. Empty for an empty stele.
    pub fn lineage(&self) -> Result<Vec<CharterEpoch>, SteleError> {
        let Some(genesis) = self.first()? else {
            return Ok(Vec::new());
        };
        let bad = |why| SteleError::BadCharterVersion {
            stele_id: genesis.stele_id,
            why,
        };
        if genesis.module_namespace != GENESIS_NAMESPACE
            || genesis.prev_digest != GENESIS_PREV_DIGEST
        {
            return Err(bad("row 1 is not a genesis row"));
        }
        let fields =
            payload_fields(&genesis.payload, "HCC-GENESIS-v1").ok_or(bad("genesis payload"))?;
        let mut tip = CharterEpoch {
            version: field(&fields, "ziggurat_version").ok_or(bad("genesis version"))?,
            byte_len: field(&fields, "charter_byte_len")
                .and_then(|v| v.parse().ok())
                .ok_or(bad("genesis byte_len"))?,
            digest: field(&fields, "charter_blake3_hex")
                .and_then(|h| digest_from_hex(&h))
                .ok_or(bad("genesis digest"))?,
            stele_id: genesis.stele_id,
        };
        if genesis.payload != genesis_payload(tip.byte_len, &tip.digest) {
            return Err(bad("genesis payload is not canonical"));
        }
        let operator = genesis.ed25519_pubkey;
        let mut lineage = Vec::new();
        let sql = format!(
            "SELECT {ROW_COLUMNS} FROM hcc_precedent_stele WHERE module_namespace = ?1 ORDER BY stele_id ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params![CHARTER_VERSION_NAMESPACE])?;
        while let Some(r) = rows.next()? {
            let row = decode(raw_row(r)?)?.row;
            let bad = |why| SteleError::BadCharterVersion {
                stele_id: row.stele_id,
                why,
            };
            let fields = payload_fields(&row.payload, "HCC-CHARTER-VERSION-v1")
                .ok_or(bad("payload header"))?;
            let next = CharterEpoch {
                version: field(&fields, "to_version").ok_or(bad("to_version"))?,
                byte_len: field(&fields, "to_charter_byte_len")
                    .and_then(|v| v.parse().ok())
                    .ok_or(bad("to_charter_byte_len"))?,
                digest: field(&fields, "to_charter_blake3_hex")
                    .and_then(|h| digest_from_hex(&h))
                    .ok_or(bad("to_charter_blake3_hex"))?,
                stele_id: row.stele_id,
            };
            if row.payload
                != charter_version_payload(&tip, &next.version, next.byte_len, &next.digest)
            {
                return Err(bad("does not continue from the previous charter"));
            }
            if !version_greater(&next.version, &tip.version) {
                return Err(bad("version does not increase"));
            }
            if row.ed25519_pubkey != operator {
                return Err(bad("signed by a different operator key than genesis"));
            }
            let signature = row.royal_seal_sig.ok_or(bad("no royal seal"))?;
            verify_royal_seal(
                &RoyalSeal {
                    operator_pubkey: operator,
                    signature,
                },
                &next.digest,
            )
            .map_err(|_| bad("royal seal does not verify"))?;
            lineage.push(std::mem::replace(&mut tip, next));
        }
        lineage.push(tip);
        Ok(lineage)
    }

    /// `genesis.charter.version`: move this vault from its current charter to `charter`, which
    /// the operator has royal-sealed. Appends one row signed with that seal. The stele keeps every
    /// earlier row; booting under any older charter is refused afterwards.
    pub fn record_charter_version(
        &mut self,
        charter: &SystemCharter<RoyalSealed>,
        royal: &RoyalSeal,
        to_version: &str,
    ) -> Result<SteleRow, SteleError> {
        verify_royal_seal(royal, charter.digest()).map_err(SteleError::Seal)?;
        let lineage = self.lineage()?;
        let tip = lineage.last().ok_or(SteleError::GenesisMismatch)?;
        if tip.matches(charter.byte_len(), charter.digest()) {
            return Err(SteleError::AlreadyCurrent);
        }
        let genesis = self.first()?.ok_or(SteleError::GenesisMismatch)?;
        if royal.operator_pubkey != genesis.ed25519_pubkey {
            return Err(SteleError::Seal(SealError::BadPublicKey));
        }
        if !version_greater(to_version, &tip.version) {
            return Err(SteleError::BadCharterVersion {
                stele_id: tip.stele_id,
                why: "new version must be MAJOR.MINOR.PATCH and higher than the current one",
            });
        }
        let payload =
            charter_version_payload(tip, to_version, charter.byte_len(), charter.digest());
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stele_id = append_row_in(
            &tx,
            CHARTER_VERSION_NAMESPACE,
            &payload,
            &royal.operator_pubkey,
            Some(&royal.signature),
        )?;
        tx.commit()?;
        self.row(stele_id)?
            .ok_or(SteleError::ChainBroken { stele_id })
    }

    #[cfg(test)]
    fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// One charter the vault has run under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharterEpoch {
    pub version: String,
    pub byte_len: usize,
    pub digest: Digest,
    /// The genesis row, or the `genesis.charter.version` row that started this epoch.
    pub stele_id: i64,
}

impl CharterEpoch {
    pub fn matches(&self, byte_len: usize, digest: &Digest) -> bool {
        self.byte_len == byte_len && &self.digest == digest
    }
}

/// Canonical `genesis.charter.version` payload.
pub fn charter_version_payload(
    from: &CharterEpoch,
    to_version: &str,
    to_byte_len: usize,
    to_digest: &Digest,
) -> Vec<u8> {
    format!(
        "HCC-CHARTER-VERSION-v1\nfrom_version={}\nfrom_charter_byte_len={}\nfrom_charter_blake3_hex={}\n\
         to_version={to_version}\nto_charter_byte_len={to_byte_len}\nto_charter_blake3_hex={}\n",
        from.version,
        from.byte_len,
        digest_hex(&from.digest),
        digest_hex(to_digest)
    )
    .into_bytes()
}

/// `MAJOR.MINOR.PATCH`, each 1..=5 ASCII digits.
fn version_parts(version: &str) -> Option<[u32; 3]> {
    let mut parts = version.split('.');
    let mut out = [0u32; 3];
    for slot in &mut out {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 5 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// True when both parse and `next` is strictly higher than `current`.
pub fn version_greater(next: &str, current: &str) -> bool {
    match (version_parts(next), version_parts(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// `key=value` lines after a fixed header line; `None` unless every line has that shape.
fn payload_fields(payload: &[u8], header: &str) -> Option<Vec<(String, String)>> {
    let text = std::str::from_utf8(payload).ok()?;
    let mut lines = text.strip_suffix('\n')?.split('\n');
    if lines.next()? != header {
        return None;
    }
    lines
        .map(|l| l.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())))
        .collect()
}

fn field(fields: &[(String, String)], key: &str) -> Option<String> {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn digest_from_hex(hex: &str) -> Option<Digest> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(hex.as_bytes().chunks(2)) {
        let pair = std::str::from_utf8(pair).ok()?;
        *slot = u8::from_str_radix(pair, 16).ok()?;
    }
    (digest_hex(&out) == hex).then_some(out)
}

/// Chain one row onto the stele inside a transaction the caller owns, on any connection to the
/// vault file. Lets another ledger (BATON) commit its row and the stele row atomically.
/// Returns the new `stele_id`; nothing is visible until the caller commits.
pub fn append_in(
    tx: &rusqlite::Transaction<'_>,
    module_namespace: &str,
    payload: &[u8],
    ed25519_pubkey: &[u8; 32],
    royal_seal_sig: Option<&[u8; 64]>,
) -> Result<i64, SteleError> {
    if module_namespace == CHARTER_VERSION_NAMESPACE {
        return Err(SteleError::ReservedNamespace);
    }
    append_row_in(
        tx,
        module_namespace,
        payload,
        ed25519_pubkey,
        royal_seal_sig,
    )
}

fn append_row_in(
    tx: &rusqlite::Transaction<'_>,
    module_namespace: &str,
    payload: &[u8],
    ed25519_pubkey: &[u8; 32],
    royal_seal_sig: Option<&[u8; 64]>,
) -> Result<i64, SteleError> {
    BsmcNamespace::parse(module_namespace).map_err(|_| SteleError::BadNamespace)?;
    let head: Option<Vec<u8>> = tx
        .query_row(
            "SELECT blake3_digest FROM hcc_precedent_stele ORDER BY stele_id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let prev_digest = match head {
        Some(bytes) => fixed::<32>(0, "blake3_digest", bytes)?,
        None => GENESIS_PREV_DIGEST,
    };
    let digest = stele_digest(&prev_digest, payload);
    let payload_len = i64::try_from(payload.len()).map_err(|_| SteleError::Corrupt {
        stele_id: 0,
        column: "payload_byte_len",
    })?;
    tx.execute(
        "INSERT INTO hcc_precedent_stele
           (prev_digest, module_namespace, version_tag, domain_separator, payload,
            payload_byte_len, blake3_digest, ed25519_pubkey, royal_seal_sig)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            &prev_digest[..],
            module_namespace,
            VERSION_TAG,
            &STELE_DOMAIN[..],
            payload,
            payload_len,
            &digest[..],
            &ed25519_pubkey[..],
            royal_seal_sig.map(|s| &s[..]),
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

#[cfg(test)]
mod tests;
