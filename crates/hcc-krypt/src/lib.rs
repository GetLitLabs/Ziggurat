//! Krypt.DB four strata (M03 `krypt.*`).
//!
//! I   RMK: 32 bytes of OS entropy.
//! II  KEK: `blake3::derive_key(KRYPT_CONTEXT, RMK || salt)` (`krypt.ke_key.derive`).
//! III DEK: random 32 bytes, wrapped under KEK with ChaCha20-Poly1305 (`krypt.de_key.wrap`).
//! IV  FEK: random 32 bytes, wrapped under DEK with ChaCha20-Poly1305; file bytes are
//!     sealed under FEK with AES-256-GCM (`krypt.fe_key.encrypt`).
//!
//! Every nonce is 12 fresh bytes from the OS. Every key type zeroizes on drop.
//! Wrapped keys bind their stratum name as AEAD associated data, so a wrapped DEK
//! cannot be replayed as a wrapped FEK.
#![forbid(unsafe_code)]

use std::fmt;

use aes_gcm::Aes256Gcm;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::ChaCha20Poly1305;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Frozen KEK derivation context.
pub const KRYPT_CONTEXT: &str = "HCC-KRYPT-STRATUM-v2";
pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const SALT_LEN: usize = 32;
/// Wrapped key = 32-byte key + 16-byte Poly1305 tag.
pub const WRAPPED_KEY_LEN: usize = KEY_LEN + 16;

const DEK_AAD: &[u8] = b"HCC-KRYPT-STRATUM-v2/III/DEK";
const FEK_AAD: &[u8] = b"HCC-KRYPT-STRATUM-v2/IV/FEK";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KryptError {
    Entropy,
    /// Wrong key, wrong associated data, or tampered bytes. Deliberately not more specific.
    Unwrap,
    Encrypt,
    MalformedWrappedKey,
}

impl fmt::Display for KryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Entropy => "os entropy unavailable",
            Self::Unwrap => "authentication failed: wrong key or tampered data",
            Self::Encrypt => "encryption failed",
            Self::MalformedWrappedKey => "wrapped key has the wrong length",
        })
    }
}

impl std::error::Error for KryptError {}

fn random<const N: usize>() -> Result<[u8; N], KryptError> {
    let mut out = [0u8; N];
    getrandom::getrandom(&mut out).map_err(|_| KryptError::Entropy)?;
    Ok(out)
}

macro_rules! secret_key {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Zeroize, ZeroizeOnDrop)]
        pub struct $name([u8; KEY_LEN]);

        impl $name {
            pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
                Self(bytes)
            }

            /// Key bytes, for wrapping or persisting under a higher stratum only.
            pub fn expose(&self) -> &[u8; KEY_LEN] {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "(<redacted>)"))
            }
        }
    };
}

secret_key!(RootMasterKey, "Stratum I: root master key.");
secret_key!(
    KeyEncryptionKey,
    "Stratum II: key-encryption key derived from the RMK."
);
secret_key!(
    DataEncryptionKey,
    "Stratum III: data-encryption key, stored only wrapped under the KEK."
);
secret_key!(
    FileEncryptionKey,
    "Stratum IV: file-encryption key, stored only wrapped under the DEK."
);

impl RootMasterKey {
    pub fn generate() -> Result<Self, KryptError> {
        Ok(Self(random()?))
    }
}

impl DataEncryptionKey {
    pub fn generate() -> Result<Self, KryptError> {
        Ok(Self(random()?))
    }
}

impl FileEncryptionKey {
    pub fn generate() -> Result<Self, KryptError> {
        Ok(Self(random()?))
    }
}

/// A key sealed under the stratum above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedKey {
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

/// File bytes sealed under a FEK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedBlob {
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

/// `krypt.ke_key.derive`
pub fn derive_kek(rmk: &RootMasterKey, salt: &[u8; SALT_LEN]) -> KeyEncryptionKey {
    let mut material = Zeroizing::new([0u8; KEY_LEN + SALT_LEN]);
    material[..KEY_LEN].copy_from_slice(rmk.expose());
    material[KEY_LEN..].copy_from_slice(salt);
    KeyEncryptionKey(blake3::derive_key(KRYPT_CONTEXT, material.as_ref()))
}

fn wrap(
    wrapping: &[u8; KEY_LEN],
    key: &[u8; KEY_LEN],
    aad: &[u8],
) -> Result<WrappedKey, KryptError> {
    let nonce = random::<NONCE_LEN>()?;
    let cipher = ChaCha20Poly1305::new(wrapping.into());
    let ciphertext = cipher
        .encrypt(&nonce.into(), Payload { msg: key, aad })
        .map_err(|_| KryptError::Encrypt)?;
    Ok(WrappedKey { nonce, ciphertext })
}

fn unwrap_key(
    wrapping: &[u8; KEY_LEN],
    wrapped: &WrappedKey,
    aad: &[u8],
) -> Result<[u8; KEY_LEN], KryptError> {
    if wrapped.ciphertext.len() != WRAPPED_KEY_LEN {
        return Err(KryptError::MalformedWrappedKey);
    }
    let cipher = ChaCha20Poly1305::new(wrapping.into());
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                &wrapped.nonce.into(),
                Payload {
                    msg: &wrapped.ciphertext,
                    aad,
                },
            )
            .map_err(|_| KryptError::Unwrap)?,
    );
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&plain);
    Ok(key)
}

/// `krypt.de_key.wrap`
pub fn wrap_dek(kek: &KeyEncryptionKey, dek: &DataEncryptionKey) -> Result<WrappedKey, KryptError> {
    wrap(kek.expose(), dek.expose(), DEK_AAD)
}

pub fn unwrap_dek(
    kek: &KeyEncryptionKey,
    wrapped: &WrappedKey,
) -> Result<DataEncryptionKey, KryptError> {
    unwrap_key(kek.expose(), wrapped, DEK_AAD).map(DataEncryptionKey)
}

/// FEK is never a loose key: it is stored only wrapped under the DEK.
pub fn wrap_fek(
    dek: &DataEncryptionKey,
    fek: &FileEncryptionKey,
) -> Result<WrappedKey, KryptError> {
    wrap(dek.expose(), fek.expose(), FEK_AAD)
}

pub fn unwrap_fek(
    dek: &DataEncryptionKey,
    wrapped: &WrappedKey,
) -> Result<FileEncryptionKey, KryptError> {
    unwrap_key(dek.expose(), wrapped, FEK_AAD).map(FileEncryptionKey)
}

/// `krypt.fe_key.encrypt`: AES-256-GCM under the FEK; `aad` binds caller context (e.g. a file id).
pub fn encrypt_file(
    fek: &FileEncryptionKey,
    aad: &[u8],
    plaintext: &[u8],
) -> Result<SealedBlob, KryptError> {
    let nonce = random::<NONCE_LEN>()?;
    let cipher = Aes256Gcm::new(fek.expose().into());
    let ciphertext = cipher
        .encrypt(
            &nonce.into(),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| KryptError::Encrypt)?;
    Ok(SealedBlob { nonce, ciphertext })
}

pub fn decrypt_file(
    fek: &FileEncryptionKey,
    aad: &[u8],
    blob: &SealedBlob,
) -> Result<Zeroizing<Vec<u8>>, KryptError> {
    let cipher = Aes256Gcm::new(fek.expose().into());
    cipher
        .decrypt(
            &blob.nonce.into(),
            Payload {
                msg: &blob.ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| KryptError::Unwrap)
}

/// What persists for one stratum chain. Holds no plaintext key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StratumEnvelope {
    pub salt: [u8; SALT_LEN],
    pub wrapped_dek: WrappedKey,
    pub wrapped_fek: WrappedKey,
}

/// `krypt.stratum.init`: fresh salt, DEK and FEK, wrapped down from the RMK.
pub fn init_stratum(rmk: &RootMasterKey) -> Result<StratumEnvelope, KryptError> {
    let salt = random::<SALT_LEN>()?;
    let kek = derive_kek(rmk, &salt);
    let dek = DataEncryptionKey::generate()?;
    let fek = FileEncryptionKey::generate()?;
    Ok(StratumEnvelope {
        salt,
        wrapped_dek: wrap_dek(&kek, &dek)?,
        wrapped_fek: wrap_fek(&dek, &fek)?,
    })
}

/// Unwrap RMK → KEK → DEK → FEK.
pub fn open_stratum(
    rmk: &RootMasterKey,
    envelope: &StratumEnvelope,
) -> Result<FileEncryptionKey, KryptError> {
    let kek = derive_kek(rmk, &envelope.salt);
    let dek = unwrap_dek(&kek, &envelope.wrapped_dek)?;
    unwrap_fek(&dek, &envelope.wrapped_fek)
}

#[cfg(test)]
mod tests;
