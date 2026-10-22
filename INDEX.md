# ZIGGURAT v1.0.0 — COMPLETE INDEX

**Document:** `INDEX.md`  
**System:** Ziggurat v1.0.0 — HCC-CCS Concept to Creation Station  
**Org:** Hammurabi Coding Company LLC / R@Made (Eau Claire, WI)  
**Architect:** R.K. Girdhari (Hammurabi / OMG-RAMG)  
**Axiom:** See A Need, Code A Seed. Models are weather; the ledger is the aquifer. AI proposes. Rules decide. Code executes. There is no try. There is only `Result<T, E>`.

Use this file as the master lookup. If it is not in this index, it is not in v1.0.0.

---

## 0. How to read this index

| Mark | Meaning |
|---|---|
| **LIVE** | Must compile, persist, or verify on the operator machine for 1.0.0 |
| **TYPED** | Named in the constitution; returns `Err(Unimplemented)` until built |
| **LAW** | Non-negotiable; violation means it is not Ziggurat |
| **WEATHER** | Optional at runtime (local model weights) |

---

## 1. Identity and governing documents

| ID | Path | Role |
|---|---|---|
| G0 | `CHARTER_GOAL.md` | ≤8k any-LLM north star. Full intent, laws, done-definition |
| G1 | `charters/ZIGGURAT_v1.0.0.txt` | Exact SYSTEM bytes (LF). Only prompt source. Hashed by xtask |
| G2 | `INDEX.md` | This file |
| G3 | `README.md` | Operator boot: seal, run daemon, gates, weather optional |
| G4 | `LICENSE` | `LicenseRef-HCC-Sovereign-Perpetual` |
| G5 | `Cargo.toml` | Workspace; `members` = crates that compile |
| G6 | `rust-toolchain.toml` | Pinned stable |
| G7 | `.gitattributes` | LF on `*.rs`, `charters/*`, `*.Modelfile` |
| G8 | `keys/` | Operator pubkey/sig. **Not in git** |
| G9 | `SPEC/ERRORS.md` | Fixed `seal_error` / `status` strings served by `/status` |
| G10 | `SPEC/DEVIATIONS.md` | Operator-accepted departures from the build order |

**Seal law (LAW):** never invent BLAKE3, Ed25519, or byte length. `hcc-xtask seal-charter` writes `crates/hcc-seal/src/baked.rs`.

---

## 2. Four objects (framing)

| Object | What | Missing? |
|---|---|---|
| Constitution | G0 + G1 + seal | Never |
| Ledger | Eduba hash chain + BATON | Never |
| Factory | daemon, seed, healer, tests, cockpit | Never |
| Weather | Ollama/Candle on `127.0.0.1:11434` | Allowed; UI must not fake success |

---

## 3. Terraces (routing)

| Terrace | Name | Job | Typical modules |
|---|---|---|---|
| T1 | Foundation | Keys, stele, shred | M03, M07, M08 |
| T2 | Substrate | Daemon, loopback, wasm, compile | M02, M04, M06 |
| T3 | Workforce | Model, retrieve, seed, jobs | M01, M05, M09 |
| T4 | Crucible | Tests, replay, gates | M10, M11 |
| T5 | Pinnacle | Cockpit, operator seal | M12 |

Every dispatch carries `terrace_level: 1..=5`. Outside that range → reject.

---

## 4. Twelve modules (BSM-C registry)

| ID | HTTP `module_id` | Name | Namespace | Status | Crate(s) |
|---|---|---|---|---|---|
| M01 | `M01-NeuralHighway` | Neural Highway | `neural.*` | TYPED | (later `hcc-neural`) |
| M02 | `M02-Accordion` | Accordion.be | `accordion.*` | **LIVE** | `hcc-daemon` |
| M03 | `M03-Krypt` | Krypt.DB / Eduba Vault | `krypt.*` `vault.*` | **LIVE** | `hcc-krypt` `hcc-iedb-stele` |
| M04 | `M04-WasiHealer` | WASI Healer | `wasi.*` `healer.*` | **LIVE** (fixture) | `hcc-wasi-healer` |
| M05 | `M05-CodeASeed` | Code A Seed | `seed.*` `ziggurat.*` | **LIVE** | `hcc-seed` |
| M06 | `M06-Polymorph` | Polymorphic Engine | `polymorph.*` | TYPED | xtask target-cpu only |
| M07 | `M07-Genesis` | Genesis / Chare | `genesis.*` `chare.*` | TYPED | metadata on stele rows |
| M08 | `M08-ZeroTrace` | Zero-Trace | `zerotrace.*` `trace.*` | TYPED | crypto-shred only when built |
| M09 | `M09-RamGenie` | RamGenie | `genie.*` | **LIVE** | `hcc-daemon` |
| M10 | `M10-Baton` | BATON / Konjure / Curie-Monet | `baton.*` `konjure.*` `curie_monet.*` | **LIVE** (BATON) / TYPED (rest) | `hcc-baton` |
| M11 | `M11-Lense` | Lense / Inspect-A-File | `lense.*` `inspect.*` | **LIVE** | `scripts/nine_gates_audit.sh` |
| M12 | `M12-Origami` | Origami.fe / SUE / Tailor | `origami.*` `sue.*` `tailor.*` | **LIVE** (cockpit) | `origami/` `src-tauri/` |

### 4.1 Namespace verbs (v1.0.0)

| Marker | Module | Status |
|---|---|---|
| `accordion.fold.expand` | M02 | LIVE — IPC unfold |
| `accordion.persist.sqlite` | M02 | LIVE — WAL write |
| `krypt.stratum.init` | M03 | LIVE |
| `krypt.ke_key.derive` | M03 | LIVE |
| `krypt.de_key.wrap` | M03 | LIVE |
| `krypt.fe_key.encrypt` | M03 | LIVE — FEK wrapped under DEK |
| `krypt.stele.verify` | M03 | LIVE via `hcc-seal` |
| `vault.ast.parse` | M03 | TYPED |
| `vault.store.lesson` | M03 | TYPED |
| `healer.diagnose.heap` | M04 | LIVE — fixture wasm |
| `healer.patch.apply` | M04 | TYPED |
| `wasi.sandbox.lock` | M04 | LIVE |
| `seed.manifest.init` | M05 | LIVE |
| `seed.scaffold.emit` | M05 | LIVE — writes files |
| `ziggurat.daemon.boot` | M05/M02 | LIVE |
| `polymorph.workspace.init` | M06 | TYPED |
| `polymorph.emit.native` | M06 | TYPED |
| `genesis.charter.version` | M07 | TYPED |
| `chare.provenance.tag` | M07 | TYPED — 67/30/3 metadata |
| `zerotrace.key.destroy` | M08 | TYPED |
| `krypt.death.keyDestroy` | M08 | TYPED |
| `genie.prompt.seal` | M09 | LIVE |
| `genie.dispatch.local` | M09 | LIVE |
| `baton.trace.record` | M10 | LIVE |
| `baton.state.replay` | M10 | LIVE |
| `konjure.pipeline.init` | M10 | TYPED |
| `curie_monet.verify` | M10 | TYPED |
| `lense.pillar.status` | M11 | LIVE |
| `inspect.gate.validate` | M11 | LIVE |
| `origami.fold.assemble` | M12 | LIVE |
| `sue.cockpit.mount` | M12 | LIVE |
| `tailor.burst.balance` | M12 | TYPED |
| `neural.retrieve.lesson` | M01 | TYPED |
| `neural.index.upsert` | M01 | TYPED |

Unknown namespace without a dot → HTTP 400. Known but TYPED → `Err(Unimplemented)`.

The HTTP `module_id` column is the wire: `hcc_daemon::MODULE_IDS` serves exactly these twelve strings; any other id → HTTP 404.

---

## 5. Crate index (workspace)

| Crate | Tier | Terrace | LIVE duty |
|---|---|---|---|
| `hcc-types` | T1 no_std | — | `TerraceLevel`, `BsmcNamespace`, shared errors |
| `hcc-seal` | T1 no_std | T1 | BLAKE3 domain hash, Ed25519 verify, typestate `Unsealed`/`RoyalSealed` |
| `hcc-iedb-stele` | T2 | T1 | Hash-chained stele, WAL, immutable triggers |
| `hcc-krypt` | T2 | T1 | 4-stratum wrap/unwrap, `Zeroize` |
| `hcc-daemon` | T2 lib+bin | T2–T3 | Bind `127.0.0.1:7749`, status, dispatch |
| `hcc-baton` | T2 | T4 | Record/replay envelopes |
| `hcc-seed` | T2 | T3 | Write real crate files + tests |
| `hcc-wasi-healer` | T2 | T2/T4 | Fuel sandbox, empty wasm fixture |
| `hcc-vanguard` | T2 | T3 | Job table; 3 live jobs |
| `hcc-xtask` | T2 | T4 | `seal-charter`, `freeze-vendor`, `gates` |
| `ziggurat-desktop` | host | T5 | Tauri 2 shell; spawns daemon |

Not members until they compile. No ghost crates in `Cargo.toml`.

---

## 6. Directory index

```text
ziggurat/
├── CHARTER_GOAL.md
├── INDEX.md                          ← this file
├── README.md
├── LICENSE
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── .gitattributes
├── .gitignore
├── charters/ZIGGURAT_v1.0.0.txt
├── crates/hcc-types/
├── crates/hcc-seal/src/baked.rs      ← xtask output only
├── crates/hcc-iedb-stele/
├── crates/hcc-krypt/
├── crates/hcc-daemon/src/
│   ├── lib.rs
│   ├── main.rs
│   └── accordion_switchboard.rs
├── crates/hcc-baton/
├── crates/hcc-seed/
├── crates/hcc-wasi-healer/
├── crates/hcc-vanguard/
├── crates/hcc-xtask/
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   ├── build.rs
│   └── src/{lib.rs,main.rs}
├── origami/
│   ├── dist/                         ← what Tauri loads
│   └── src/OrigamiCockpit.*
├── scripts/nine_gates_audit.sh
├── tests/ziggurat_integration.rs
├── fixtures/healer_empty.wasm
├── keys/.gitkeep                     ← secrets stay off git
└── vendor/                           ← cargo vendor
```

---

## 7. Network and Faraday index (LAW)

| Endpoint | Bind | Role |
|---|---|---|
| Accordion.be | `127.0.0.1:7749` | Only public IPC |
| Local runner | `127.0.0.1:11434` | Weather (Ollama/Candle) |
| Origami serve (dev) | `127.0.0.1` only | Optional; no Vite-as-law |

**Banned:** Node, npm, pnpm, yarn, Vite, Webpack, React, Next, SvelteKit SSR, Electron, Supabase, Firebase, Docker-as-architecture, `localStorage` as source of truth, DNS on daemon hot path, telemetry, non-loopback bind/connect.

**CSP:** `connect-src 'self' http://127.0.0.1:7749 http://127.0.0.1:11434`

**Svelte 5:** allowed only if compiler vendored or `origami/dist` checked in. Else Leptos/static webview.

---

## 8. HTTP API index

| Method | Path | Marker | Auth/guard |
|---|---|---|---|
| GET | `/v1/ziggurat/pillar/{module_id}/status` | `lense.pillar.status` | Loopback peer |
| POST | `/v1/ramgenie/dispatch` | `genie.dispatch.local` | Loopback + sealed charter |

### 8.1 Status body

`module_id`, `status`, `ziggurat_version`, `charter_sealed`, `charter_byte_len`, `charter_blake3_hex`, `seal_error` (omitted when sealed; values in `SPEC/ERRORS.md`), `stele_genesis`, `loopback_peer`, `runner_up`

Hex/length **computed**. Never stub.

### 8.2 Dispatch request

`model`, `bsmc_namespace`, `terrace_level`, `operator_prompt`

Reject: non-loopback, terrace ∉ 1..=5, namespace without `.`, unsealed charter.

### 8.3 Dispatch response

`status`, `bsmc_namespace`, `terrace_level`, `charter_blake3_hex`, `baton_envelope_blake3_hex`, `stele_dispatch_row_id`, `model_output`

Runner down → 502, UI red, no forged BATON.

---

## 9. Ledger index

### 9.1 Domain separators (frozen)

| Name | Bytes | Used by |
|---|---|---|
| Stele | `b"HCC-STELLE-v2\x00"` (14) | Charter + stele rows. Spelling frozen |
| BATON | `b"HCC-BATON-v1\x00"` (13) | Dispatch/trace envelopes |
| Krypt HKDF | `"HCC-KRYPT-STRATUM-v2"` | KEK derive context string |

### 9.2 Tables

**`hcc_precedent_stele`**  
`stele_id`, `prev_digest`, `module_namespace`, `version_tag`, `domain_separator`, `payload`, `payload_byte_len`, `blake3_digest` UNIQUE, `ed25519_pubkey`, `royal_seal_sig`, `committed_at_utc`

`blake3_digest = BLAKE3(domain || prev_digest || payload)`  
Genesis `prev_digest` = 32×`0x00`.

**`hcc_baton_trace_ledger`**  
`trace_id`, `prev_digest`, `module_namespace`, `sequence_number`, `terrace_level` CHECK 1–5, `input_state_hash`, `output_state_hash`, `baton_envelope_sig` UNIQUE, `recorded_at_utc`

**`hcc_ramgenie_dispatch_log`**  
Optional denormalized view of dispatches; still chained; still append-only.

**Triggers:** `BEFORE UPDATE` / `BEFORE DELETE` → `RAISE(ABORT)`. Seatbelt, not the chain. The chain is `prev_digest`.

**Pragmas:** `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`.

**DB file:** `./eduba_vault.db` (app data dir in desktop build). Tests use `Connection::open_in_memory()`, never a file named `:memory:`.

---

## 10. Crypto index (M03)

| Stratum | Name | Algorithm | Rule |
|---|---|---|---|
| I | RMK | 32-byte OS entropy | `Zeroize` |
| II | KEK | BLAKE3 `derive_key` | From RMK + salt |
| III | DEK | ChaCha20-Poly1305 wrap | Under KEK |
| IV | FEK | AES-256-GCM | **Wrapped under DEK**; random 12-byte nonces in prod |

`hcc-krypt` reading of IV: the FEK is wrapped under the DEK with ChaCha20-Poly1305 (the DEK's cipher); the FEK seals file bytes with AES-256-GCM. Wrapped keys carry their stratum name as AEAD associated data.

Wipe (M08 when live) = crypto-shred keys + deletion certificate on stele. No SSD Gutmann claims.

Typestate: `SystemCharter<Unsealed>` has no `verified_prompt()`.

---

## 11. Vanguard job index (M05/M09)

“30 Vanguard” = capability table, not 30 processes.

| Job | Status |
|---|---|
| `scribe.stele.append` | **LIVE** |
| `seed.scaffold.emit` | **LIVE** |
| `inspect.gate.validate` | **LIVE** |
| All other named jobs | TYPED → `Err(Unimplemented { job })` |

Handoff JSON: `{ "job", "bsmc_namespace", "terrace_level", "payload" }`.

`hcc-vanguard::VANGUARD_TABLE` holds 35 named jobs: the three above plus every §4.1 marker. As a
*dispatchable job* each §4.1 marker is TYPED (the underlying function may already be LIVE in its
crate). In this build all three live jobs execute.

`inspect.gate.validate` payload: `{ "gates": [1..=11, ...] }` (default: all of §14). It runs the same checks as
`scripts/nine_gates_audit.sh` (a thin wrapper over `cargo run -p hcc-xtask -- gates`, so Windows runs the
same code), returns every verdict with its detail, and appends one stele row `HCC-GATES-v1\ngate=NN PASS|FAIL\n…passed=P/T\n`.
Gates 8 and 9 FAIL until `src-tauri/` and `origami/dist` exist; a failing gate is never reported as a pass.

`seed.scaffold.emit` payload: `{ "blueprint": { "name", "summary", "tables": [{ "name", "columns": [{ "name", "type" }] }] } }`
(`type` ∈ INTEGER | TEXT | BLOB | REAL; names from allow-lists, so nothing in a blueprint reaches Rust or SQL
unescaped). It writes `crates/<name>/{Cargo.toml, src/lib.rs, tests/schema.rs, rustfmt.toml}`, adds the
workspace member, runs `cargo check --offline -p <name>` (rolled back on failure, never overwrites), and appends
one stele row `HCC-SEED-v1\nname=…\nblueprint_blake3_hex=…\nfile=<path> <blake3>\n…`. The operator path is
`cargo run -p hcc-xtask -- seed blueprints/<name>.json`; `blueprints/capsnap.json` → `crates/capsnap` is the
committed demo, and a test fails if those files drift from the blueprint.

`hcc-wasi-healer` (M04) implements `wasi.sandbox.lock` as `SandboxPolicy::LOCKED` (fuel 10,000,000; linear memory
16 MiB; module ≤ 4 MiB; one instance; `declared_paths` empty, so no host imports are linked and any import is refused)
and `healer.diagnose.heap` as `diagnose(wasm, &policy)`. Both are crate functions; as Vanguard jobs they remain TYPED.

M12 build notes: `src-tauri/` is the `ziggurat-desktop` crate (Tauri 2). It calls `hcc_daemon::boot_and_serve` on
Tauri's async runtime, so the app and `hcc-daemon` boot identically. `origami/dist` is hand-written HTML/CSS/JS
checked in as-is (no `origami/src`, no npm). `capabilities/default.json` grants no permissions. Gate 5 also scans
Cargo.lock: tauri lists `reqwest` only under `cfg(android | ios)`, which no desktop build compiles; any other path
to a forbidden crate fails the gate.

`xtask freeze-vendor` runs `cargo vendor --locked --versioned-dirs vendor`, checks that every registry package in
Cargo.lock is present, and writes a local `.cargo/config.toml` replacing crates-io with `vendor/`. Both paths are
gitignored (about 650 MB); `--undo` removes only a config it wrote. Dispatch commits the BATON trace and its
`genie.dispatch.local` stele row in one transaction (`BatonLedger::record_then` + `hcc_iedb_stele::append_in`).

BATON envelope inputs: `prompt_hash = BLAKE3(prompt)`, `response_hash = BLAKE3(response)`,
`terrace_u8` is one byte. Ledger `prev_digest` = previous row's envelope; replay recomputes each
envelope under the sealed charter digest.

---

## 12. Ritual index (operator / LLM)

| Ritual | When | Output |
|---|---|---|
| TRIAGE | Every inbound | `KEEP` `MERGE` `COMPOST` + next mechanical action |
| BUILD | Implementation | Compiling code; no TODO where logic belongs |
| LEVEL | Before “done” | test, clippy, loopback tree, stele chain |
| REPAIR | Defects | Smallest reversible shim + proof |
| DIVERGE | Only if operator asks full-spec prose | Build / ripple / chuckle / puddle |
| VERIFY | Claims | Badge only if a command ran this session |

Badges: `CERTAIN` `VERIFIED` `EVIDENCED` `INFERRED` `ASSUMED` `SPECULATIVE` `UNCOMPILED` `UNIMPLEMENTED`.

---

## 13. Command index

```text
# Seal (writes baked.rs from the charter file)
cargo run -p hcc-xtask -- seal-charter charters/ZIGGURAT_v1.0.0.txt \
  --pubkey keys/operator.pub --sig keys/charter.sig

cargo run -p hcc-xtask -- freeze-vendor
cargo run -p hcc-xtask -- gates

cargo check --workspace --all-targets
cargo test --workspace -- --nocapture
cargo clippy --workspace --all-targets -- -D warnings \
  -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic
cargo tree --workspace

cargo run -p hcc-daemon
curl -s http://127.0.0.1:7749/v1/ziggurat/pillar/M09-RamGenie/status

curl -s -X POST http://127.0.0.1:7749/v1/ramgenie/dispatch \
  -H "Content-Type: application/json" \
  -d '{"model":"ziggurat:1.0.0","bsmc_namespace":"seed.scaffold.emit","terrace_level":3,"operator_prompt":"Scaffold crate capsnap with sqlite wal schema."}'

bash scripts/nine_gates_audit.sh
cargo tauri build --bundles app
```

Optional weather: local Ollama/Candle already listening on `127.0.0.1:11434`. Absence must not crash the daemon.

---

## 14. Nine Gates index (M11)

| Gate | Check |
|---|---|
| 1 | `cargo check --workspace --all-targets` |
| 2 | `#![forbid(unsafe_code)]` compiles (not naive `git grep unsafe`) |
| 3 | clippy deny warnings, unwrap, expect, panic |
| 4 | `cargo test --workspace` |
| 5 | no openssl / reqwest / native-tls / trust-dns in `cargo tree` |
| 6 | WAL on test/app db |
| 7 | seal tests with **in-test generated** keys, not fantasy hex |
| 8 | Tauri CSP contains both loopback hosts |
| 9 | `origami/dist` exists; no non-loopback URLs |
| 10 | stele/baton `prev_digest` + immutable triggers |
| 11 | krypt round-trip + healer empty wasm under fuel |

---

## 15. Cockpit index (M12)

| Surface | Behavior |
|---|---|
| Top bar | version, `127.0.0.1:7749`, runner `:11434`, seal badge from GET |
| Left rail | T1–T5 |
| Center | active terrace, module chips, pillar telemetry |
| Right | namespace, prompt, dispatch, last BATON hex, last output |
| Poll | ~5s |
| Failure | red + error text. **No invented hex** |

UI compile story: checked-in `origami/dist` **or** Leptos. No runtime npm.

---

## 16. Type index (`hcc-types`)

| Type | Notes |
|---|---|
| `TerraceLevel` | Foundation=1 … Pinnacle=5; serde when used in JSON |
| `BsmcNamespace` | newtype; must contain `.` |
| `CharterState` | sealed trait; `Unsealed`, `RoyalSealed` |
| `BsmcModule<S>` | consume-only transitions if used; no fake `const fn` + `String` |
| `VanguardJob` | enum/table id |
| `ZigguratError` | Display + no unwrap |

---

## 17. Boot sequence index

1. Tauri starts → spawn `hcc-daemon`  
2. Read `charters/ZIGGURAT_v1.0.0.txt`  
3. Length + `BLAKE3(stele_domain || bytes)` vs `baked.rs`  
4. Ed25519 verify  
5. Fail → `/status` `charter_sealed: false`; `/dispatch` forbidden  
6. Open WAL; create tables; genesis stele if empty  
7. Bind `127.0.0.1:7749` only  
8. Probe `:11434` (warn, do not exit)  
9. Webview loads `origami/dist`; GET status; badge follows daemon  

---

## 18. Expected output of finished 1.0.0

- Workspace members = crates that `cargo test --workspace`  
- Charter sealed by xtask, not hand-typed arrays  
- `curl` status on loopback  
- Dispatch works if weather is up; degrades if not  
- M05 has written at least one demo crate to disk  
- README: daemon portable; weights optional; how to seal; how to run gates  
- No shipping claims of 5ms HNSW, AVX-512 magic, or GitHub-as-airgap  

---

## 19. Compost index (not Ziggurat)

- Invented `EXPECTED_CHARTER_BLAKE3` / royal signatures  
- `[VERIFIED]` without a command  
- Fake UI BATON/seal hex  
- `PathBuf::from(":memory:")` as in-memory SQLite  
- `import` in Rust, hyphenated crate idents (`hcc_iedb-stele`)  
- Vite `:5173` with empty `beforeBuildCommand`  
- GitHub Actions as sovereignty  
- Marker counts (“36 Markers”) with no table  
- 27 silent-success Vanguard agents  

---

## 20. Implementer lookup (first vertical slice)

| Order | Build until green |
|---|---|
| 1 | `CHARTER_GOAL.md` + `charters/ZIGGURAT_v1.0.0.txt` + this `INDEX.md` |
| 2 | `hcc-xtask` `seal-charter` |
| 3 | `hcc-seal` fail-closed |
| 4 | `hcc-iedb-stele` chain + triggers |
| 5 | `hcc-daemon` GET `/status` |
| 6 | cockpit badge from live GET |
| 7 | POST `/dispatch` + BATON |
| 8 | `hcc-seed` file emit |
| 9 | healer fixture |
| 10 | `nine_gates_audit.sh` on this machine |

**Next mechanical action:** items 1–5. Do not add M01/M06/M08 engines until `curl http://127.0.0.1:7749/v1/ziggurat/pillar/M09-RamGenie/status` returns a `charter_blake3_hex` that matches the `charter_blake3_hex` printed by `hcc-xtask seal-charter` (the domain-separated digest, not plain `b3sum`), and stele row 1 embeds that hex.

---

*End of Index. If you add a crate, namespace, port, table, or job, add a row here in the same commit or COMPOST the change.*
