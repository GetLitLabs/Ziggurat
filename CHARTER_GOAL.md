# ZIGGURAT v1.0.0 — UNIFIED GOAL (ANY-LLM NORTH STAR)

Designation: Master architectural shell — 5-terrace desktop container and 12-module local workforce.
Org: Hammurabi Coding Company LLC / R@Made, Eau Claire, WI.
Architect: R.K. Girdhari (Hammurabi / OMG-RAMG).
Axiom: See A Need, Code A Seed. | Models are weather; the ledger is the aquifer. AI proposes. Rules decide. Code executes. There is no try. There is only Result<T, E>. | Keep the faith, the good exists.

## INTENT
Ziggurat exists so one operator can run a complete software factory on one air-gapped machine: retrieve, generate, compile, verify, seal, and ship without renting compute or leaking telemetry. Cloud is optional weather. The on-disk stele is the aquifer you cannot fake. Finished Ziggurat is not a chat wrapper. It is a sovereign execution environment whose UI, daemon, ledger, sandbox, and optional local model share one constitution.

## FINISHED SYSTEM
One Tauri 2 desktop binary hosting: Rust daemon Accordion.be on 127.0.0.1:7749; cockpit Origami.fe in the webview; SQLite WAL plus hash-chained Eduba stele; 4-stratum Krypt.DB; fuel-capped WASI healer; Code A Seed scaffolder that writes real crates; BATON trace recorder; Inspect-A-File gates as a local script; RamGenie talking only to 127.0.0.1:11434. If the model is absent, the app still boots, shows SEALED or UNSEALED honestly, serves status, and records operator actions. Inference never starts if charter BLAKE3 or Ed25519 royal seal fails.

## HARD LAWS (ELSE IT IS NOT ZIGGURAT)
- Zero-Node at runtime: never require Node, npm, pnpm, yarn, Vite, Webpack, React, Next, SvelteKit SSR, Electron, Supabase, Firebase, Docker-as-architecture, or browser localStorage as source of truth. Svelte 5 only if the compiler is vendored or precompiled assets are checked in; otherwise Leptos or static webview. cargo-installed Tauri CLI is allowed.
- Zero egress: bind and connect only loopback. No DNS on the daemon hot path. No telemetry. CSP connect-src 'self' http://127.0.0.1:7749 http://127.0.0.1:11434.
- Rust: #![forbid(unsafe_code)], no unwrap/expect/panic on production paths, explicit Result<T, E>.
- Never invent digests, signatures, or byte lengths. Seal via xtask from charters/ZIGGURAT_v1.0.0.txt (LF only). Private key never in git.
- Never tag [VERIFIED] unless a command ran. Never fake green seals or BATON hex in the UI.
- GitHub Actions is optional public mirror, not air-gap proof.

## METHODOLOGY
BSM-C: every module has a frozen namespace, a mechanical next action, and joinery to neighbors. No framework soup.
NBS: if it does not compile, persist, or verify on this machine, it is not done. Placeholders are COMPOST.

## SUBSTRATE (S.T.R.A.W. + EDUBA)
S cockpit; T Tauri 2 capability-scoped; R Rust 2021 three-tier workspace; A Axum 0.8 loopback; W wasmi/Wasmtime with fuel and memory caps.
Eduba: WAL for mutable state; append-only stele where each row hashes domain || prev_digest || payload. Domain frozen as b"HCC-STELLE-v2\x00".
Krypt: RMK → KEK (BLAKE3 derive) → DEK (ChaCha20-Poly1305 wrap) → FEK (AES-256-GCM; FEK wrapped under DEK). Wipe means crypto-shred keys.
Tiers: T1 no_std hcc-types, hcc-seal; T2 host stele, krypt, daemon, baton, seed, wasi-healer, xtask; T3 wasm and/or checked-in cockpit dist.

## TERRACES
1 Foundation: keys, stele, shred.
2 Substrate: daemon, loopback, wasm, compile.
3 Workforce: model, retrieve, seed, jobs.
4 Crucible: tests, BATON replay, gates.
5 Pinnacle: cockpit, operator seal.
Route every task to a terrace. Do not couple UI frame rate to heavy loops.

## TWELVE MODULES
M01 neural.* local embeddings and lesson retrieve.
M02 accordion.* loopback HTTP/JSON/SSE and sqlite persist.
M03 krypt.* vault.* 4-stratum crypto and lesson/AST store.
M04 wasi.* healer.* sandboxed diagnose; patches are reversible shims with tests.
M05 seed.* ziggurat.* blueprint to real files (Cargo.toml, src, tests).
M06 polymorph.* compile-time target-cpu, offline wasm.
M07 genesis.* chare.* charter versions; artifact metadata 67 creator / 30 validator / 3 protocol.
M08 zerotrace.* key destroy plus deletion certificate on stele.
M09 genie.* sealed prompt inject; local generate/chat wire.
M10 baton.* konjure.* curie_monet.* hash-chained traces; optional logic vs UI tags.
M11 lense.* inspect.* status JSON; nine_gates_audit.sh is 11/11.
M12 origami.* sue.* tailor.* cockpit, sidebar, burst vs consolidate.

Workforce: 30 Vanguard is a capability table of named jobs dispatched through M09 with JSON handoff. v1.0.0 ships the table plus three live jobs: scribe, scaffold, verifier. Unimplemented jobs return Err, never pretend success.

## RITUALS
TRIAGE KEEP|MERGE|COMPOST plus next mechanical action.
BUILD compiling code; no TODO where logic belongs.
LEVEL cargo test, clippy -D warnings, loopback-only deps, stele chain.
REPAIR smallest reversible shim plus proof.
Full DIVERGE (build, ripple, chuckle, puddle) only when the operator asks for full-spec prose. Default output is paths, schemas, code, commands.

## RUNTIME
Boot: read charter file → length and BLAKE3(domain||bytes) match baked constants → Ed25519 verify → SystemCharter<RoyalSealed> → open WAL → append genesis stele if missing → bind 127.0.0.1:7749 → probe 11434 (warn, do not crash) → open window.
GET /v1/ziggurat/pillar/{module_id}/status → sealed, byte_len, blake3_hex, peer.
POST /v1/ramgenie/dispatch {model, bsmc_namespace, terrace_level, operator_prompt} → inject sealed prompt → local generate → BATON envelope = BLAKE3(b"HCC-BATON-v1\x00"||charter||ns||terrace||prompt_hash||response_hash) → append log → return output and hex.
Reject non-loopback peers, terrace outside 1..=5, namespace without a dot.
Seed writes crates/<name>/ and a workspace member, then cargo check.
Healer loads fixture wasm, enforces fuel, no host FS except declared paths.

## COCKPIT
Five-terrace rail, module chips, live seal badge from GET status, dispatch form, last output, last BATON hex, health dots. Poll ~5s. Failure is red with error text. No invented hashes.

## EXPECTED OUTPUT OF FINISHED v1.0.0
Workspace members equal crates that compile.
charters/ZIGGURAT_v1.0.0.txt is the only prompt source; xtask writes seal constants.
cargo test --workspace and scripts/nine_gates_audit.sh pass offline after vendor.
cargo run -p hcc-daemon serves loopback; curl status works.
Desktop shows live charter state; dispatch works when runner is up and degrades when not.
M05 has written at least one demo crate to disk with schema and tests.
README states: daemon is portable; model weights are optional weather; how to seal; how to run gates.
No shipping claims of 5ms HNSW, AVX-512 magic, or GitHub-as-airgap.

## IMPLEMENTER RULES
Pass tests before adding modules. Prefer seal → daemon → sqlite row → window over twelve empty crates. Namespace comments must match real functions. .gitattributes forces LF on charters, *.rs, *.Modelfile. Do not invent crypto material. Do not mark work operations-ready until cargo test has been run on the operator's machine.
