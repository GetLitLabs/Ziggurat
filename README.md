# Ziggurat

Constitution: [CHARTER_GOAL.md](CHARTER_GOAL.md). This is the only long-form briefing for any model.
Master lookup: [INDEX.md](INDEX.md). If it is not in the index, it is not in v1.0.0.

## What exists today

| Piece | State |
|---|---|
| `charters/ZIGGURAT_v1.0.0.txt` | Frozen: 4660 bytes, LF only, no BOM |
| `hcc-types` | `TerraceLevel` (1..=5), `BsmcNamespace` (must contain a dot) |
| `hcc-seal` | `no_std`; domain-separated BLAKE3, Ed25519 verify, `SystemCharter<Unsealed>` / `<RoyalSealed>` |
| `hcc-xtask` | `keygen`, `sign-charter`, `seal-charter`, `hygiene`, `seed <blueprint.json>`, `gates [--only N,N]` (M11 audit, INDEX §14), `freeze-vendor [--undo]` |
| `crates/hcc-seal/src/baked.rs` | Length + digest baked; **no royal seal yet**, so the charter stays Unsealed |
| `hcc-iedb-stele` | Append-only hash-chained stele in SQLite WAL; abort triggers; genesis embeds the charter |
| `hcc-krypt` | RMK → KEK (BLAKE3 `derive_key`, context `HCC-KRYPT-STRATUM-v2`) → DEK (ChaCha20-Poly1305 wrap) → FEK (wrapped under DEK; AES-256-GCM data); random nonces; zeroize on drop. Not wired into the daemon yet |
| `hcc-neural` | M01 Neural Highway: `vault.store.lesson` appends a lesson plus an `HCC-LESSON-v1` stele row (hashes only); `neural.index.upsert` stores per-model f32 vectors from the runner's `/api/embed` on 127.0.0.1:11434; `neural.retrieve.lesson` is exact brute-force cosine top-k. Runner down is a typed error. Not wired into the daemon or Vanguard yet; lesson text is not encrypted yet |
| `hcc-baton` | `hcc_baton_trace_ledger`: frozen envelope, hash-chained traces, record + replay, abort triggers |
| `hcc-vanguard` | 35-entry job table with JSON handoff; `scribe.stele.append`, `seed.scaffold.emit` and `inspect.gate.validate` live; the rest `Err(Unimplemented)` |
| `hcc-seed` | M05 Code A Seed: validated blueprint JSON → `crates/<name>` (Cargo.toml, lib, schema tests, WAL); deterministic; adds the workspace member; `cargo check --offline`, rolled back on failure; never overwrites |
| `hcc-wasi-healer` | M04: wasmi 2 (pure Rust interpreter) sandbox with a 10M fuel cap, 16 MiB memory cap, one instance and zero host imports (any import, WASI `path_open` included, is refused before instantiation). `diagnose` runs the start section and `_start`, then reports exported memories and fuel used. `fixtures/healer_empty.wasm` is the 8-byte empty module. Not wired into Vanguard or the daemon; `healer.patch.apply` stays TYPED |
| `src-tauri` (`ziggurat-desktop`) | Tauri 2 shell: starts Accordion.be in-process, then shows the cockpit. CSP allows only `'self'` plus `127.0.0.1:7749` and `127.0.0.1:11434`; the window has no Tauri permissions |
| `origami/dist` | M12 cockpit as plain checked-in HTML, CSS and JS (no npm, no build step): seal badge, T1–T5 rail, module chips, pillar telemetry, dispatch form, last BATON hex and output, 5 s poll, red errors |
| `capsnap` | Demo crate written by `xtask seed blueprints/capsnap.json`; pinned to its blueprint by a drift test |
| `hcc-daemon` | Loopback-only on 127.0.0.1:7749: `GET /v1/ziggurat/pillar/{module_id}/status` and `POST /v1/ramgenie/dispatch`; opens the stele and BATON ledger on a sealed boot |

Krypt and Vanguard are not wired into the daemon. Nothing here is operations-ready until
`cargo test --workspace` passes on the operator machine.

## Nine gates (M11)

```sh
bash scripts/nine_gates_audit.sh          # Linux/macOS/Git Bash
cargo run -p hcc-xtask -- gates           # same checks; works in PowerShell
cargo run -p hcc-xtask -- gates --only 2,6,10
```

Each line prints `GATE NN PASS|FAIL` with what was observed. The command exits non-zero unless every selected gate passes.

## Offline builds (vendor/ is committed)

`vendor/` holds every crate in Cargo.lock (464 crates, about 650 MB, mostly Windows API crates for Tauri) and
`.cargo/config.toml` points cargo at it, so a fresh clone builds, tests and runs the gates with networking off:

```sh
cargo test --workspace --offline
```

Cargo re-checks every vendored file against its checksum, and `.gitattributes` keeps git from converting their
line endings. After changing any dependency, refresh and commit both (the refresh needs the network once):

```sh
cargo run -p hcc-xtask -- freeze-vendor
git add vendor .cargo/config.toml Cargo.lock
```

`freeze-vendor --undo` points cargo back at the registry.

## Desktop app (Tauri 2 + cockpit)

Run from the repo root so the charter path resolves:

```sh
cargo run -p ziggurat-desktop                 # debug build, opens the cockpit window
cargo build --release -p ziggurat-desktop     # target/release/ziggurat-desktop(.exe)
```

The app boots the same daemon as `hcc-daemon` (same charter check, same stele, same flags `--charter` and `--db`).
If a standalone `hcc-daemon` already holds 127.0.0.1:7749, the app logs that and the cockpit talks to it.
Windows needs the WebView2 runtime (built into Windows 10 and 11) and the MSVC build tools; Linux needs
WebKitGTK 4.1 development packages. The cockpit reads only the daemon's replies: an unsealed charter shows a red
UNSEALED badge with its `seal_error` and dispatch stays disabled, and a missing daemon shows DAEMON UNREACHABLE.
The daemon sends CORS headers only to the Tauri webview origins (`tauri://localhost`, `http(s)://tauri.localhost`).

## Seal ceremony (operator machine)

```bash
cargo run -p hcc-xtask -- hygiene charters/ZIGGURAT_v1.0.0.txt
cargo run -p hcc-xtask -- keygen                      # writes keys/operator.key (0600) + keys/operator.pub; never commit
cargo run -p hcc-xtask -- sign-charter charters/ZIGGURAT_v1.0.0.txt --key keys/operator.key
cargo run -p hcc-xtask -- seal-charter charters/ZIGGURAT_v1.0.0.txt \
  --pubkey keys/operator.pub --sig keys/charter.sig   # rewrites crates/hcc-seal/src/baked.rs
cargo test -p hcc-seal
```

`seal-charter` prints two hashes. `charter_blake3_hex` is the seal:
`BLAKE3(b"HCC-STELLE-v2\x00" || bytes)`. `raw_b3sum_hex` is what plain `b3sum` prints,
shown only for comparison; the two never match. Re-running `seal-charter` on the same
file must produce a byte-identical `baked.rs`; if it does not, the file has a CRLF or BOM
fault, not a crypto fault.

## Target layout (crates appear here only once they compile)

```text
ziggurat/
├── CHARTER_GOAL.md
├── README.md
├── LICENSE                         # LicenseRef-HCC-Sovereign-Perpetual
├── Cargo.toml                      # workspace only; members = crates that compile
├── Cargo.lock
├── rust-toolchain.toml             # pinned stable
├── .gitattributes                  # LF on charters, *.rs, *.Modelfile
├── .gitignore
├── charters/ZIGGURAT_v1.0.0.txt    # exact SYSTEM bytes; LF; hashed by xtask
├── crates/
│   ├── hcc-types/                  # T1 TerraceLevel, BsmcNamespace, errors
│   ├── hcc-seal/                   # T1 BLAKE3 + Ed25519 verify
│   ├── hcc-iedb-stele/             # T2 hash-chained stele + WAL
│   ├── hcc-krypt/                  # T2 RMK→KEK→DEK→FEK
│   ├── hcc-daemon/                 # T2 Accordion + RamGenie wire
│   ├── hcc-baton/                  # T2 trace record + replay
│   ├── hcc-seed/                   # T2 writes real crate files
│   ├── hcc-wasi-healer/            # T2 fuel sandbox + fixture runner
│   ├── hcc-vanguard/               # T2 job table; 3 live, rest Err
│   ├── hcc-neural/                 # T2 M01 lessons + loopback embeddings + cosine retrieve
│   └── hcc-xtask/                  # T2 seal-charter, freeze-vendor, gates
├── src-tauri/                      # Faraday frame; hosts daemon task
├── origami/dist/                   # checked-in cockpit; no runtime npm
├── scripts/nine_gates_audit.sh
├── tests/ziggurat_integration.rs
├── fixtures/healer_empty.wasm
├── keys/.gitkeep                   # operator keys live here, never in git
└── vendor/                         # cargo vendor for offline
```

## Run the daemon

```bash
cargo run -p hcc-daemon            # from the workspace root; --charter <path>, --db <path> to override
curl -s http://127.0.0.1:7749/v1/ziggurat/pillar/M09-RamGenie/status
```

The daemon boots whether or not the seal holds. `charter_blake3_hex` is computed from the
file at boot and must equal `seal-charter`'s `charter_blake3_hex`. `charter_sealed` is `true`
and `status` is `ROYAL_SEAL_LOCKED` only after the operator's royal seal is baked; otherwise
`status` is `UNSEALED` and `seal_error` says why (values in `SPEC/ERRORS.md`).
On a royal-sealed boot the daemon opens `eduba_vault.db` (WAL), appends the genesis stele
if the vault is empty, verifies the whole chain, and reports `stele_genesis: true`. An
unsealed boot never opens or creates the vault. A missing model runner on 127.0.0.1:11434
is a warning and `runner_up: false`, never a crash. Module ids: `M01-NeuralHighway`,
`M02-Accordion`, `M03-Krypt`, `M04-WasiHealer`, `M05-CodeASeed`, `M06-Polymorph`,
`M07-Genesis`, `M08-ZeroTrace`, `M09-RamGenie`, `M10-Baton`, `M11-Lense`, `M12-Origami`.

## Genesis stele

Row 1 of `hcc_precedent_stele` has `prev_digest` = 32 zero bytes, namespace
`ziggurat.daemon.boot`, and this exact payload:

```text
HCC-GENESIS-v1
ziggurat_version=1.0.0
charter_byte_len=4660
charter_blake3_hex=cd380eae2bf4ef632316f7720d0aad77b2b916f86232c394ca2b6192004e3b96
```

Its digest does not depend on the operator key, so every honest seal of this charter
produces genesis `blake3_digest` `35401ED11F7C7CFC403E89F833C5C56115987C3EF0BBBB072F732F440FE1F040`:

```sql
SELECT stele_id, hex(prev_digest), hex(blake3_digest) FROM hcc_precedent_stele;
```

## Dispatch (model weights are optional weather)

With a local runner (Ollama-compatible `/api/chat`) on 127.0.0.1:11434:

```bash
curl -s -X POST http://127.0.0.1:7749/v1/ramgenie/dispatch \
  -H "Content-Type: application/json" \
  -d '{"model":"ziggurat:1.0.0","bsmc_namespace":"seed.scaffold.emit","terrace_level":3,"operator_prompt":"Scaffold crate capsnap with sqlite wal schema."}'
```

Only a royal-sealed charter with an open, verified vault can dispatch. The charter text is the
system message. On success the daemon records a BATON trace, appends a `genie.dispatch.local`
stele row that embeds the envelope hex, and returns `DISPATCH_SEALED` with
`baton_envelope_blake3_hex`, `baton_trace_id`, and `stele_dispatch_row_id`. If the runner is down
the reply is HTTP 502 and nothing is appended. Error strings: `SPEC/ERRORS.md`.
