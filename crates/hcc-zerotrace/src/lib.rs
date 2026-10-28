//! Zero-Trace (M08 `zerotrace.key.destroy`, `krypt.death.keyDestroy`).
//!
//! The Krypt keyring lives in the vault file next to the stele: one row per stratum chain
//! holding only what `hcc_krypt::init_stratum` persists (KEK salt, DEK wrapped under the KEK,
//! FEK wrapped under the DEK). No plaintext key is ever stored.
//!
//! Wipe = crypto-shred. `Keyring::destroy` overwrites the salt and both wrapped keys with NULL
//! on a connection running `PRAGMA secure_delete = ON` (SQLite zeroes the freed cell bytes),
//! appends a deletion certificate to the stele in the same transaction, then runs
//! `PRAGMA wal_checkpoint(TRUNCATE)`. With the salt and wrapped DEK gone, the RMK can no longer
//! reach the DEK or FEK, so every blob sealed under that FEK is unreadable.
//!
//! Scope, stated in every certificate: this vault file only. Copies of the file taken before the
//! certificate still hold the envelope, and no claim is made about flash wear-levelling or
//! filesystem journals below SQLite.
#![forbid(unsafe_code)]

use std::fmt;
use std::path::Path;

use hcc_iedb_stele::{append_in, SteleError, GENESIS_NAMESPACE, GENESIS_PREV_DIGEST};
use hcc_krypt::{
    init_stratum, open_stratum, FileEncryptionKey, KryptError, RootMasterKey, StratumEnvelope,
    WrappedKey, NONCE_LEN, SALT_LEN, WRAPPED_KEY_LEN,
};
use hcc_seal::{digest_hex, Digest};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

/// Domain for the envelope fingerprint kept after shredding.
pub const ZEROTRACE_DOMAIN: &[u8; 17] = b"HCC-ZEROTRACE-v1\x00";
/// Namespace of the deletion-certificate stele row.
pub const CERTIFICATE_NAMESPACE: &str = "zerotrace.key.destroy";
pub const LABEL_MAX: usize = 64;
/// Namespace of the stele row that records a new key (`krypt.stratum.init`).
pub const STRATUM_NAMESPACE: &str = "krypt.stratum.init";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS hcc_krypt_keyring (
  key_id               TEXT PRIMARY KEY CHECK (length(key_id) = 32),
  label                TEXT NOT NULL,
  envelope_blake3      BLOB NOT NULL UNIQUE CHECK (length(envelope_blake3) = 32),
  salt                 BLOB CHECK (salt IS NULL OR length(salt) = 32),
  dek_nonce            BLOB CHECK (dek_nonce IS NULL OR length(dek_nonce) = 12),
  wrapped_dek          BLOB CHECK (wrapped_dek IS NULL OR length(wrapped_dek) = 48),
  fek_nonce            BLOB CHECK (fek_nonce IS NULL OR length(fek_nonce) = 12),
  wrapped_fek          BLOB CHECK (wrapped_fek IS NULL OR length(wrapped_fek) = 48),
  created_at_utc       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  destroyed_at_utc     TEXT,
  certificate_stele_id INTEGER,
  CHECK (
    (destroyed_at_utc IS NULL AND certificate_stele_id IS NULL AND salt IS NOT NULL
      AND dek_nonce IS NOT NULL AND wrapped_dek IS NOT NULL
      AND fek_nonce IS NOT NULL AND wrapped_fek IS NOT NULL)
    OR
    (destroyed_at_utc IS NOT NULL AND certificate_stele_id IS NOT NULL AND salt IS NULL
      AND dek_nonce IS NULL AND wrapped_dek IS NULL
      AND fek_nonce IS NULL AND wrapped_fek IS NULL)
  )
);
CREATE TRIGGER IF NOT EXISTS hcc_krypt_keyring_shred_only
BEFORE UPDATE ON hcc_krypt_keyring
WHEN OLD.destroyed_at_utc IS NOT NULL
  OR NEW.destroyed_at_utc IS NULL
  OR NEW.key_id IS NOT OLD.key_id
  OR NEW.label IS NOT OLD.label
  OR NEW.envelope_blake3 IS NOT OLD.envelope_blake3
  OR NEW.created_at_utc IS NOT OLD.created_at_utc
BEGIN SELECT RAISE(ABORT, 'hcc_krypt_keyring rows change only by crypto-shred'); END;
CREATE TRIGGER IF NOT EXISTS hcc_krypt_keyring_no_delete
BEFORE DELETE ON hcc_krypt_keyring
BEGIN SELECT RAISE(ABORT, 'hcc_krypt_keyring keeps shredded rows as tombstones'); END;
";

#[derive(Debug)]
pub enum ZeroTraceError {
    Sql(rusqlite::Error),
    Krypt(KryptError),
    Stele(SteleError),
    /// Labels are 1..=64 of `[A-Za-z0-9._-]`, so a certificate line cannot be forged by a label.
    BadLabel,
    NotFound(String),
    /// The key was crypto-shredded; its certificate is this stele row.
    Destroyed {
        key_id: String,
        certificate_stele_id: i64,
    },
    /// A certificate needs a constitution: stele row 1 must be the genesis row.
    NoGenesis,
    Corrupt {
        key_id: String,
        column: &'static str,
    },
}

impl fmt::Display for ZeroTraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(err) => write!(f, "sqlite: {err}"),
            Self::Krypt(err) => write!(f, "krypt: {err}"),
            Self::Stele(err) => write!(f, "stele: {err}"),
            Self::BadLabel => write!(f, "label must be 1..={LABEL_MAX} of [A-Za-z0-9._-]"),
            Self::NotFound(key_id) => write!(f, "no key {key_id} in the keyring"),
            Self::Destroyed {
                key_id,
                certificate_stele_id,
            } => write!(
                f,
                "key {key_id} was crypto-shredded; deletion certificate is stele row {certificate_stele_id}"
            ),
            Self::NoGenesis => {
                f.write_str("the stele has no genesis row; refusing to shred without a ledger")
            }
            Self::Corrupt { key_id, column } => {
                write!(f, "keyring row {key_id}: column {column} is malformed")
            }
        }
    }
}

impl std::error::Error for ZeroTraceError {}

impl From<rusqlite::Error> for ZeroTraceError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sql(err)
    }
}

/// `BLAKE3(ZEROTRACE_DOMAIN || salt || dek_nonce || wrapped_dek || fek_nonce || wrapped_fek)`.
/// Every field is fixed-width, so the concatenation is unambiguous. It names which envelope a
/// certificate destroyed; it is a digest of salt and ciphertext and reveals no key.
pub fn envelope_fingerprint(envelope: &StratumEnvelope) -> Digest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ZEROTRACE_DOMAIN);
    hasher.update(&envelope.salt);
    hasher.update(&envelope.wrapped_dek.nonce);
    hasher.update(&envelope.wrapped_dek.ciphertext);
    hasher.update(&envelope.wrapped_fek.nonce);
    hasher.update(&envelope.wrapped_fek.ciphertext);
    *hasher.finalize().as_bytes()
}

/// Key id: the first 16 bytes of the fingerprint, as 32 lowercase hex characters.
pub fn key_id_for(fingerprint: &Digest) -> String {
    digest_hex(fingerprint).chars().take(32).collect()
}

fn label_ok(label: &str) -> bool {
    (1..=LABEL_MAX).contains(&label.len())
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Canonical certificate payload. Every value is fixed-format or from the label allow-list.
pub fn certificate_payload(key_id: &str, label: &str, fingerprint: &Digest) -> Vec<u8> {
    format!(
        "HCC-ZEROTRACE-v1\nkey_id={key_id}\nlabel={label}\nenvelope_blake3_hex={}\n\
         destroyed=salt(II) wrapped_dek(III) wrapped_fek(IV)\n\
         method=crypto-shred; sqlite secure_delete=ON; wal_checkpoint(TRUNCATE) after commit\n\
         scope=this vault file only; copies taken before this row still hold the envelope\n",
        digest_hex(fingerprint)
    )
    .into_bytes()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStatus {
    Live,
    Destroyed {
        destroyed_at_utc: String,
        certificate_stele_id: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionCertificate {
    pub key_id: String,
    pub label: String,
    pub envelope_blake3: Digest,
    pub stele_id: i64,
    pub stele_blake3: Digest,
    /// `PRAGMA wal_checkpoint(TRUNCATE)` completed with no busy reader. When false, another
    /// connection held the WAL open; the shredded page reaches the main file at its next
    /// checkpoint, and older WAL frames stay on disk until then.
    pub wal_truncated: bool,
}

type RawEnvelope = (
    String,
    Vec<u8>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<String>,
    Option<i64>,
);

const ENVELOPE_COLUMNS: &str = "label, envelope_blake3, salt, dek_nonce, wrapped_dek, fek_nonce, \
     wrapped_fek, destroyed_at_utc, certificate_stele_id";

fn raw_envelope(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawEnvelope> {
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

fn fixed<const N: usize>(
    key_id: &str,
    column: &'static str,
    bytes: Option<Vec<u8>>,
) -> Result<[u8; N], ZeroTraceError> {
    bytes
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| ZeroTraceError::Corrupt {
            key_id: key_id.to_owned(),
            column,
        })
}

fn wrapped(
    key_id: &str,
    column: &'static str,
    nonce: Option<Vec<u8>>,
    ciphertext: Option<Vec<u8>>,
) -> Result<WrappedKey, ZeroTraceError> {
    Ok(WrappedKey {
        nonce: fixed::<NONCE_LEN>(key_id, column, nonce)?,
        ciphertext: fixed::<WRAPPED_KEY_LEN>(key_id, column, ciphertext)?.to_vec(),
    })
}

/// A row read back: either the live envelope or the tombstone.
enum Entry {
    Live {
        label: String,
        fingerprint: Digest,
        envelope: StratumEnvelope,
    },
    Destroyed {
        destroyed_at_utc: String,
        certificate_stele_id: i64,
    },
}

fn decode(key_id: &str, raw: RawEnvelope) -> Result<Entry, ZeroTraceError> {
    let (label, fp, salt, dek_nonce, dek, fek_nonce, fek, destroyed_at, certificate) = raw;
    if let (Some(destroyed_at_utc), Some(certificate_stele_id)) = (destroyed_at, certificate) {
        return Ok(Entry::Destroyed {
            destroyed_at_utc,
            certificate_stele_id,
        });
    }
    let fingerprint = fixed::<32>(key_id, "envelope_blake3", Some(fp))?;
    let envelope = StratumEnvelope {
        salt: fixed::<SALT_LEN>(key_id, "salt", salt)?,
        wrapped_dek: wrapped(key_id, "wrapped_dek", dek_nonce, dek)?,
        wrapped_fek: wrapped(key_id, "wrapped_fek", fek_nonce, fek)?,
    };
    if envelope_fingerprint(&envelope) != fingerprint {
        return Err(ZeroTraceError::Corrupt {
            key_id: key_id.to_owned(),
            column: "envelope_blake3",
        });
    }
    Ok(Entry::Live {
        label,
        fingerprint,
        envelope,
    })
}

/// Refuse unless stele row 1 is the genesis row: keys are born and shredded on a ledger.
fn require_genesis(tx: &rusqlite::Transaction<'_>) -> Result<(), ZeroTraceError> {
    let genesis: Option<(Vec<u8>, String)> = tx
        .query_row(
            "SELECT prev_digest, module_namespace FROM hcc_precedent_stele
             ORDER BY stele_id ASC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match genesis {
        Some((prev, ns)) if prev == GENESIS_PREV_DIGEST && ns == GENESIS_NAMESPACE => Ok(()),
        _ => Err(ZeroTraceError::NoGenesis),
    }
}

/// Canonical `krypt.stratum.init` stele payload. Hashes and allow-listed text only.
pub fn stratum_payload(key_id: &str, label: &str, fingerprint: &Digest) -> Vec<u8> {
    format!(
        "HCC-KRYPT-STRATUM-v1\nkey_id={key_id}\nlabel={label}\nenvelope_blake3_hex={}\n",
        digest_hex(fingerprint)
    )
    .into_bytes()
}

/// A key created on the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedKey {
    pub key_id: String,
    pub label: String,
    pub envelope_blake3: Digest,
    pub stele_id: i64,
}

/// One keyring row as listed: never any key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEntry {
    pub key_id: String,
    pub label: String,
    pub envelope_blake3: Digest,
    pub created_at_utc: String,
    pub status: KeyStatus,
}

/// The keyring table in a vault file, on its own `secure_delete` connection.
pub struct Keyring {
    conn: Connection,
}

impl Keyring {
    /// Open the keyring inside the vault file at `path` (the same file as the stele).
    pub fn open(path: &Path) -> Result<Self, ZeroTraceError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(hcc_iedb_stele::BUSY_TIMEOUT)?;
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        let secure: i64 = conn.query_row("PRAGMA secure_delete = ON", [], |r| r.get(0))?;
        if secure != 1 {
            return Err(ZeroTraceError::Sql(rusqlite::Error::InvalidQuery));
        }
        conn.execute_batch("PRAGMA synchronous = FULL;")?;
        // The certificate is chained onto the stele, so its table must exist here too.
        drop(hcc_iedb_stele::Stele::open(path).map_err(ZeroTraceError::Stele)?);
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// `krypt.stratum.init` persisted: fresh salt, DEK and FEK wrapped down from `rmk`.
    /// Returns the key id.
    pub fn create(&mut self, rmk: &RootMasterKey, label: &str) -> Result<String, ZeroTraceError> {
        if !label_ok(label) {
            return Err(ZeroTraceError::BadLabel);
        }
        let envelope = init_stratum(rmk).map_err(ZeroTraceError::Krypt)?;
        let fingerprint = envelope_fingerprint(&envelope);
        let key_id = key_id_for(&fingerprint);
        self.conn.execute(
            "INSERT INTO hcc_krypt_keyring
               (key_id, label, envelope_blake3, salt, dek_nonce, wrapped_dek, fek_nonce, wrapped_fek)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                key_id,
                label,
                &fingerprint[..],
                &envelope.salt[..],
                &envelope.wrapped_dek.nonce[..],
                envelope.wrapped_dek.ciphertext,
                &envelope.wrapped_fek.nonce[..],
                envelope.wrapped_fek.ciphertext,
            ],
        )?;
        Ok(key_id)
    }

    /// `krypt.stratum.init` on the ledger: the keyring row and an `HCC-KRYPT-STRATUM-v1` stele
    /// row (key id, label, envelope fingerprint; never a key) commit in one IMMEDIATE transaction.
    /// Refused unless stele row 1 is genesis, like a certificate.
    pub fn create_recorded(
        &mut self,
        rmk: &RootMasterKey,
        label: &str,
        ed25519_pubkey: &[u8; 32],
    ) -> Result<CreatedKey, ZeroTraceError> {
        if !label_ok(label) {
            return Err(ZeroTraceError::BadLabel);
        }
        let envelope = init_stratum(rmk).map_err(ZeroTraceError::Krypt)?;
        let fingerprint = envelope_fingerprint(&envelope);
        let key_id = key_id_for(&fingerprint);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_genesis(&tx)?;
        tx.execute(
            "INSERT INTO hcc_krypt_keyring
               (key_id, label, envelope_blake3, salt, dek_nonce, wrapped_dek, fek_nonce, wrapped_fek)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                key_id,
                label,
                &fingerprint[..],
                &envelope.salt[..],
                &envelope.wrapped_dek.nonce[..],
                envelope.wrapped_dek.ciphertext,
                &envelope.wrapped_fek.nonce[..],
                envelope.wrapped_fek.ciphertext,
            ],
        )?;
        let payload = stratum_payload(&key_id, label, &fingerprint);
        let stele_id = append_in(&tx, STRATUM_NAMESPACE, &payload, ed25519_pubkey, None)
            .map_err(ZeroTraceError::Stele)?;
        tx.commit()?;
        Ok(CreatedKey {
            key_id,
            label: label.to_owned(),
            envelope_blake3: fingerprint,
            stele_id,
        })
    }

    /// Every keyring row, oldest first: id, label, fingerprint and whether it was shredded.
    pub fn list(&self) -> Result<Vec<KeyEntry>, ZeroTraceError> {
        let mut stmt = self.conn.prepare(
            "SELECT key_id, label, envelope_blake3, created_at_utc, destroyed_at_utc,
                    certificate_stele_id
               FROM hcc_krypt_keyring ORDER BY created_at_utc ASC, key_id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (key_id, label, fp, created_at_utc, destroyed_at_utc, certificate) = row?;
            let envelope_blake3 = fixed::<32>(&key_id, "envelope_blake3", Some(fp))?;
            let status = match (destroyed_at_utc, certificate) {
                (Some(destroyed_at_utc), Some(certificate_stele_id)) => KeyStatus::Destroyed {
                    destroyed_at_utc,
                    certificate_stele_id,
                },
                _ => KeyStatus::Live,
            };
            entries.push(KeyEntry {
                key_id,
                label,
                envelope_blake3,
                created_at_utc,
                status,
            });
        }
        Ok(entries)
    }

    fn entry(conn: &Connection, key_id: &str) -> Result<Entry, ZeroTraceError> {
        let sql = format!("SELECT {ENVELOPE_COLUMNS} FROM hcc_krypt_keyring WHERE key_id = ?1");
        let raw = conn
            .query_row(&sql, params![key_id], raw_envelope)
            .optional()?
            .ok_or_else(|| ZeroTraceError::NotFound(key_id.to_owned()))?;
        decode(key_id, raw)
    }

    pub fn status(&self, key_id: &str) -> Result<KeyStatus, ZeroTraceError> {
        Ok(match Self::entry(&self.conn, key_id)? {
            Entry::Live { .. } => KeyStatus::Live,
            Entry::Destroyed {
                destroyed_at_utc,
                certificate_stele_id,
            } => KeyStatus::Destroyed {
                destroyed_at_utc,
                certificate_stele_id,
            },
        })
    }

    /// The persisted envelope (salt and wrapped keys; no plaintext key). Err once destroyed.
    pub fn envelope(&self, key_id: &str) -> Result<StratumEnvelope, ZeroTraceError> {
        match Self::entry(&self.conn, key_id)? {
            Entry::Live { envelope, .. } => Ok(envelope),
            Entry::Destroyed {
                certificate_stele_id,
                ..
            } => Err(ZeroTraceError::Destroyed {
                key_id: key_id.to_owned(),
                certificate_stele_id,
            }),
        }
    }

    /// Unwrap RMK → KEK → DEK → FEK for a live key.
    pub fn open_key(
        &self,
        rmk: &RootMasterKey,
        key_id: &str,
    ) -> Result<FileEncryptionKey, ZeroTraceError> {
        let envelope = self.envelope(key_id)?;
        open_stratum(rmk, &envelope).map_err(ZeroTraceError::Krypt)
    }

    /// `zerotrace.key.destroy` / `krypt.death.keyDestroy`: crypto-shred one key and append its
    /// deletion certificate to the stele, atomically. `ed25519_pubkey` is the operator key the
    /// stele row is recorded under.
    pub fn destroy(
        &mut self,
        key_id: &str,
        ed25519_pubkey: &[u8; 32],
    ) -> Result<DeletionCertificate, ZeroTraceError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (label, fingerprint) = match Self::entry(&tx, key_id)? {
            Entry::Live {
                label, fingerprint, ..
            } => (label, fingerprint),
            Entry::Destroyed {
                certificate_stele_id,
                ..
            } => {
                return Err(ZeroTraceError::Destroyed {
                    key_id: key_id.to_owned(),
                    certificate_stele_id,
                })
            }
        };
        require_genesis(&tx)?;
        let payload = certificate_payload(key_id, &label, &fingerprint);
        let stele_id = append_in(&tx, CERTIFICATE_NAMESPACE, &payload, ed25519_pubkey, None)
            .map_err(ZeroTraceError::Stele)?;
        let shredded = tx.execute(
            "UPDATE hcc_krypt_keyring
                SET salt = NULL, dek_nonce = NULL, wrapped_dek = NULL,
                    fek_nonce = NULL, wrapped_fek = NULL,
                    destroyed_at_utc = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                    certificate_stele_id = ?2
              WHERE key_id = ?1 AND destroyed_at_utc IS NULL",
            params![key_id, stele_id],
        )?;
        if shredded != 1 {
            return Err(ZeroTraceError::NotFound(key_id.to_owned()));
        }
        let stele_blake3: Vec<u8> = tx.query_row(
            "SELECT blake3_digest FROM hcc_precedent_stele WHERE stele_id = ?1",
            params![stele_id],
            |r| r.get(0),
        )?;
        let stele_blake3 = fixed::<32>(key_id, "certificate blake3_digest", Some(stele_blake3))?;
        tx.commit()?;
        let (busy, _log, _done): (i64, i64, i64) =
            self.conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?;
        Ok(DeletionCertificate {
            key_id: key_id.to_owned(),
            label,
            envelope_blake3: fingerprint,
            stele_id,
            stele_blake3,
            wal_truncated: busy == 0,
        })
    }

    #[cfg(test)]
    fn connection(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests;
