# Ziggurat v1.0.0 — status error strings

`GET /v1/ziggurat/pillar/{module_id}/status` carries `seal_error` only when
`charter_sealed` is `false`. On the sealed path the field is **omitted**.

Values are fixed strings. They never contain lengths, file paths, key paths, or key
material: details (expected vs actual length, io errors, the charter path) go to the
daemon's boot log on stderr only. The daemon never reads private keys at all.

Do not invent new strings. Add a row here in the same commit as the code, or COMPOST the change.
`crates/hcc-seal/tests/errors_doc.rs` fails if this table and the code drift apart.

## `seal_error`

| Value | Source | Meaning |
|---|---|---|
| `baked seal constants absent` | `SealError::NotBaked` | `hcc-xtask seal-charter` has not written `baked.rs` |
| `charter byte length does not match baked seal` | `SealError::ByteLenMismatch` | file length differs from the seal (often CRLF or BOM) |
| `charter digest does not match baked seal` | `SealError::DigestMismatch` | same length, different bytes |
| `no operator Ed25519 royal seal baked` | `SealError::NoRoyalSeal` | digest matches; operator has not signed |
| `baked operator public key is invalid` | `SealError::BadPublicKey` | baked pubkey is not a valid Ed25519 point |
| `operator Ed25519 signature does not verify` | `SealError::BadSignature` | signature is not over this charter's digest by this key |
| `charter is not valid UTF-8` | `SealError::NotUtf8` | sealed bytes cannot be a prompt |
| `charter file unreadable` | `hcc_daemon::CHARTER_UNREADABLE` | daemon could not read the charter file; length and hex are `null` |

## `status`

| Value | When |
|---|---|
| `ROYAL_SEAL_LOCKED` | `charter_sealed: true` |
| `UNSEALED` | any `seal_error` above |
