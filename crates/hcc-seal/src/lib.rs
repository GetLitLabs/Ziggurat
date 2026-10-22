//! T1 charter seal (`krypt.stele.verify`).
//!
//! The charter digest is `BLAKE3(STELE_DOMAIN || charter_bytes)`. A charter
//! reaches [`RoyalSealed`] only when its length and digest match the constants
//! that `hcc-xtask seal-charter` baked into `baked.rs` and the operator's
//! Ed25519 signature over that digest verifies. Only a royal-sealed charter
//! yields [`SystemCharter::verified_prompt`]:
//!
//! ```compile_fail
//! let charter = hcc_seal::SystemCharter::load(b"unsealed".to_vec());
//! let _ = charter.verified_prompt();
//! ```
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod baked;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;

use ed25519_dalek::{Signature, VerifyingKey};

pub use baked::BAKED_SEAL;

/// Frozen stele/charter domain separator (14 bytes, spelling frozen).
pub const STELE_DOMAIN: &[u8; 14] = b"HCC-STELLE-v2\x00";
/// Frozen BATON envelope domain separator (13 bytes).
pub const BATON_DOMAIN: &[u8; 13] = b"HCC-BATON-v1\x00";

pub type Digest = [u8; 32];

/// `BLAKE3(STELE_DOMAIN || bytes)`.
pub fn charter_digest(bytes: &[u8]) -> Digest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(STELE_DOMAIN);
    hasher.update(bytes);
    *hasher.finalize().as_bytes()
}

/// Lowercase hex of a 32-byte digest.
pub fn digest_hex(digest: &Digest) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

/// Operator's Ed25519 public key and signature over the charter digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoyalSeal {
    pub operator_pubkey: [u8; 32],
    pub signature: [u8; 64],
}

/// What `hcc-xtask seal-charter` measured from the charter file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealConstants {
    pub byte_len: usize,
    pub blake3: Digest,
    pub royal: Option<RoyalSeal>,
}

mod private {
    pub trait Sealed {}
}

/// Typestate marker. Only [`Unsealed`] and [`RoyalSealed`] implement it.
pub trait CharterState: private::Sealed {
    #[doc(hidden)]
    type Body: AsRef<[u8]>;
}

/// Charter bytes read from disk, not yet verified.
pub enum Unsealed {}
/// Length, digest, and royal seal all verified; bytes are UTF-8.
pub enum RoyalSealed {}

impl private::Sealed for Unsealed {}
impl private::Sealed for RoyalSealed {}
impl CharterState for Unsealed {
    type Body = Vec<u8>;
}
impl CharterState for RoyalSealed {
    type Body = String;
}

pub struct SystemCharter<S: CharterState> {
    body: S::Body,
    digest: Digest,
    _state: PhantomData<S>,
}

impl<S: CharterState> SystemCharter<S> {
    pub fn byte_len(&self) -> usize {
        self.body.as_ref().len()
    }

    /// The domain-separated charter digest, computed from the bytes held.
    pub fn digest(&self) -> &Digest {
        &self.digest
    }

    pub fn digest_hex(&self) -> String {
        digest_hex(&self.digest)
    }
}

impl<S: CharterState> fmt::Debug for SystemCharter<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemCharter")
            .field("byte_len", &self.byte_len())
            .field("blake3_hex", &self.digest_hex())
            .finish()
    }
}

impl SystemCharter<Unsealed> {
    pub fn load(bytes: Vec<u8>) -> Self {
        let digest = charter_digest(&bytes);
        Self {
            body: bytes,
            digest,
            _state: PhantomData,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    /// Fail-closed seal check. On refusal the unsealed charter is handed back
    /// so status can still report its real length and digest.
    pub fn verify(
        self,
        expected: Option<&SealConstants>,
    ) -> Result<SystemCharter<RoyalSealed>, Refusal> {
        match check(&self, expected) {
            Err(error) => Err(Refusal {
                charter: self,
                error,
            }),
            Ok(()) => match String::from_utf8(self.body) {
                Ok(text) => Ok(SystemCharter {
                    body: text,
                    digest: self.digest,
                    _state: PhantomData,
                }),
                Err(not_utf8) => Err(Refusal {
                    charter: SystemCharter::load(not_utf8.into_bytes()),
                    error: SealError::NotUtf8,
                }),
            },
        }
    }
}

impl SystemCharter<RoyalSealed> {
    /// The charter text, injectable as a system prompt.
    pub fn verified_prompt(&self) -> &str {
        &self.body
    }
}

fn check(
    charter: &SystemCharter<Unsealed>,
    expected: Option<&SealConstants>,
) -> Result<(), SealError> {
    let expected = expected.ok_or(SealError::NotBaked)?;
    if charter.byte_len() != expected.byte_len {
        return Err(SealError::ByteLenMismatch {
            expected: expected.byte_len,
            actual: charter.byte_len(),
        });
    }
    if charter.digest != expected.blake3 {
        return Err(SealError::DigestMismatch);
    }
    let royal = expected.royal.ok_or(SealError::NoRoyalSeal)?;
    verify_royal_seal(&royal, &charter.digest)
}

/// Ed25519 strict verification of the operator's signature over `digest`.
pub fn verify_royal_seal(royal: &RoyalSeal, digest: &Digest) -> Result<(), SealError> {
    let key =
        VerifyingKey::from_bytes(&royal.operator_pubkey).map_err(|_| SealError::BadPublicKey)?;
    let signature = Signature::from_bytes(&royal.signature);
    key.verify_strict(digest, &signature)
        .map_err(|_| SealError::BadSignature)
}

/// A charter that failed the seal, plus the reason.
#[derive(Debug)]
pub struct Refusal {
    pub charter: SystemCharter<Unsealed>,
    pub error: SealError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealError {
    /// `baked.rs` holds no constants: xtask seal-charter has not run.
    NotBaked,
    ByteLenMismatch {
        expected: usize,
        actual: usize,
    },
    DigestMismatch,
    /// Length and digest match, but no operator signature was baked.
    NoRoyalSeal,
    BadPublicKey,
    BadSignature,
    NotUtf8,
}

impl fmt::Display for SealError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBaked => f.write_str("no baked seal constants; run hcc-xtask seal-charter"),
            Self::ByteLenMismatch { expected, actual } => {
                write!(f, "charter is {actual} bytes, seal expects {expected}")
            }
            Self::DigestMismatch => {
                f.write_str("charter BLAKE3 digest does not match the baked seal")
            }
            Self::NoRoyalSeal => f.write_str("no operator Ed25519 royal seal baked"),
            Self::BadPublicKey => {
                f.write_str("baked operator public key is not a valid Ed25519 point")
            }
            Self::BadSignature => f.write_str("operator Ed25519 signature does not verify"),
            Self::NotUtf8 => f.write_str("charter bytes are not valid UTF-8"),
        }
    }
}
