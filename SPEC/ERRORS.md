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

## `POST /v1/ramgenie/dispatch` errors

Body is `{"error": "<value>"}`. Nothing is appended to BATON or the stele on any error.

| Value | HTTP | When |
|---|---|---|
| `non-loopback peer refused` | 403 | peer address is not loopback |
| `dispatch body must be JSON {model, bsmc_namespace, terrace_level, operator_prompt}` | 400 | malformed JSON, missing or unknown fields |
| `terrace_level must be 1..=5` | 400 | terrace outside 1..=5 |
| `bsmc_namespace must be dot-separated` | 400 | namespace without a dot or with an empty segment |
| `model must be 1-128 chars of [A-Za-z0-9_.:/-]` | 400 | model name fails the allow-list |
| `operator_prompt must not be empty` | 400 | empty prompt |
| `charter not royal-sealed; dispatch forbidden` | 403 | charter is not `RoyalSealed` |
| `stele or BATON ledger not open; dispatch held` | 503 | sealed, but the vault did not open or verify at boot |
| `runner unreachable on 127.0.0.1:11434` | 502 | nothing listening on the runner port |
| `runner timed out` | 502 | no full reply within 300 s |
| `runner returned a non-success HTTP status` | 502 | runner answered with a non-2xx status; the value continues `: HTTP <code>: <runner error>`, quoting the runner's `error` text (or body) with control characters dropped, cut at 200 characters |
| `runner HTTP exchange failed` | 502 | HTTP/1.1 exchange with the runner broke |
| `runner response has no message.content` | 502 | runner reply is not Ollama `/api/chat` JSON |
| `ledger lock poisoned` | 500 | an earlier panic poisoned the ledger lock |
| `ledger append failed` | 500 | SQLite refused the BATON or stele append (detail in the daemon log) |
