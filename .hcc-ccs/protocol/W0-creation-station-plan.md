# W0 — Concept-to-Creation Station: baseline and plan

Status: **Specified** (plan) · baseline evidence below is **Built** only where a command ran.
Date: 2026-10-22
Requested by: R.K. Girdhari (operator)

## Goal

Turn Ziggurat v1.0.0 (already titled "HCC-CCS Concept to Creation Station" in `INDEX.md`) into the
Windows 11 desktop app the operator asked for:

> Concept-to-Creation Station via Paperclip, DesktopCommanderMCP, BMAD, REA, Diffusion for image gen,
> Elven Labs for audio gen, all local, all built in, a HuggingFace quantized GGUF LLM baked in, a semantic
> vector CoW WAL SQLite database built in, Origami frontend with Svelte 5 runes, SvelteKit, Tauri 2,
> S.T.R.A.W. + IEDB, Accordion middleware and Eduba database backend, full-stack with no Node.js.

The plan extends the existing charter rather than replacing it. `CHARTER_GOAL.md` is sealed, so nothing
here renames a module, adds a thirteenth module, or changes a frozen domain separator. Every new
capability lands on a namespace the charter already reserves as TYPED.

## Mapping the request onto the twelve modules

| Asked for | Lands on | Namespace (already reserved) | How |
|---|---|---|---|
| Baked-in GGUF LLM | M09 RamGenie | `genie.dispatch.local` | New `Runner` trait: in-process Candle quantized GGUF first, `127.0.0.1:11434` stays as fallback weather |
| Semantic vector DB | M01 Neural Highway | `neural.index.upsert`, `neural.retrieve.lesson` | Embeddings as f32 BLOBs in the Eduba WAL; exact cosine scan first, ANN only if measured slow |
| "CoW" | M02 / Eduba | `accordion.persist.sqlite` | WAL already gives copy-on-write style snapshot reads; no new engine |
| Diffusion image gen | M10 Curie-Monet | `curie_monet.verify` + new verb via DEVIATION | Candle Stable Diffusion (SD-Turbo / SDXL-Turbo class), weights loaded from a local folder |
| Audio gen ("Elven Labs") | M10 Konjure | `konjure.pipeline.init` | ElevenLabs is a cloud API, which breaks zero egress. Local replacement, see decision 3 |
| BMAD | M05 / Vanguard | Vanguard job table | BMAD's roles (Analyst, PM, Architect, Scrum Master, Dev, QA) as checked-in persona packs; each is a Vanguard job, so unimplemented roles return `Err`, never fake success |
| Paperclip | Vanguard + Eduba | `scribe.stele.append` | Paperclip's org chart, tickets and budgets become Eduba tables; every hand-off is a BATON trace |
| DesktopCommanderMCP | M04 / M05 | `wasi.sandbox.lock`, `seed.scaffold.emit` | Native Rust file and process tools scoped by Tauri capabilities. Optional: talk to an external Desktop Commander over MCP stdio, never bundled |
| REA | ? | ? | **Needs operator definition** (decision 4) |
| Origami + Svelte 5 runes | M12 | `origami.fold.assemble` | Svelte 5 components compiled once to static files and checked into `origami/dist` (charter rule) |
| S.T.R.A.W. + IEDB + Accordion + Eduba | existing | — | Already live: Axum on 7749, hash-chained stele, WAL |

## Conflicts with the charter, stated plainly

1. **SvelteKit and "No Node".** SvelteKit's compiler runs on Node. The charter already resolves this: Svelte 5
   is allowed only if `origami/dist` is checked in. So Node can appear on the operator's build bench once per
   UI change, never in the shipped app or on the customer's machine. If even that is unacceptable, the
   alternative is Leptos (Rust to WASM), which drops Svelte entirely.
2. **Paperclip, BMAD and Desktop Commander are Node projects.** Bundling them would break Zero-Node. The plan
   reimplements their ideas in Rust and keeps attribution in the docs. Their code is not vendored.
3. **ElevenLabs is cloud-only.** It cannot be "all local". Candidates, all permissive and offline:
   Kokoro-82M (Apache-2.0, best quality per size), Piper (MIT, fastest, many voices).
   License qualifiers must be re-checked on the vendor pages before shipping (scaffold rule).
4. **CSP is frozen to ports 7749 and 11434.** In-process inference needs no new port, which is why it is the
   recommended path. A llama-server sidecar would need a DEVIATION row and a CSP change.
5. **SQLx vs rusqlite.** The workspace uses `rusqlite` (bundled). Adding `sqlx-sqlite` links a second
   `libsqlite3-sys` and Cargo refuses two `links = "sqlite3"` crates. Recommendation: stay on rusqlite.
   If SQLx is required, it is a full migration gate of its own.
6. **Model weights are not "baked into the binary".** A Q4 7B GGUF is roughly 4 GB; it ships beside the
   `.exe` in the app data folder (or the installer), is hashed into the stele on first load, and the app
   boots honestly without it (charter law).

## Gate map (each stops for operator approval)

| Gate | Title | Deliverable | Proof |
|---|---|---|---|
| W0 | Baseline + plan | This file | `cargo test --workspace --offline` on current main |
| W1 | Neural Highway | `hcc-neural`: embeddings table in Eduba WAL, upsert, cosine top-k, stele row per index change | unit tests, gate 6 + 10 still pass |
| W2 | In-process GGUF | `Runner` trait; Candle quantized GGUF runner; 11434 fallback; model file hashed to stele | test with a tiny GGUF fixture; dispatch still 502s honestly with no model |
| W3 | Origami Studio | Svelte 5 runes UI compiled to `origami/dist`: Concept → Plan → Assets board | gate 9, CSP unchanged, no Node at runtime (gate 5 scan) |
| W4 | Curie-Monet images | Candle SD pipeline, local weights folder, PNG + stele row with prompt and seed | one draft render on CPU, real file checked with an image decoder |
| W5 | Konjure audio | Local TTS (choice from decision 3), WAV out, stele row | one rendered WAV, duration and sample rate checked |
| W6 | Workforce | BMAD persona packs + Paperclip-style org/tickets as Vanguard jobs and Eduba tables | job table tests; unknown roles return `Err` |
| W7 | Desk tools | Scoped file/process tools (Desktop Commander equivalent), REA once defined | capability JSON grants only named paths |
| W8 | Windows 11 ship | `cargo tauri build` on the operator's Windows machine, NSIS/MSI | build log from that machine, not from this sandbox |

## Evidence (W0)

Run in a Linux cloud sandbox on 2026-10-22 against `main` @ `8397f6c1`:

```
cargo test --workspace --offline --exclude ziggurat-desktop
# 31 test binaries, every one "test result: ok"
# 115 passed; 0 failed; 0 ignored
EXIT=0
```

`ziggurat-desktop` (the Tauri crate) was excluded because the sandbox has no `webkit2gtk-4.1`. It is
**not verified** by this gate; W8 verifies it on the operator's Windows machine.

## Tenth Man

The strongest case against this plan: it is ten gates of work and the operator asked for one app. Mapping
every new capability onto reserved namespaces keeps the charter sealed, but it may read as cramming image
and audio generation into modules named for other ideas (Curie-Monet was "verify", Konjure was "pipeline").
If the operator would rather amend the charter (v1.1.0) and add honest module names, that is cleaner long
term and costs one re-seal. Second objection: CPU-only Candle diffusion on a typical Windows laptop is slow
(minutes per image). W4 must report real timings, and GPU (CUDA or DirectML) may need its own gate.

## Decision needed from owner

1. Build in Ziggurat (recommended) or a separate repo?
2. Svelte 5 with a checked-in `dist` (Node on the build bench only), or Leptos (no Node anywhere)?
3. Local voice engine: Kokoro-82M (recommended) or Piper?
4. What does **REA** stand for in this request?
5. Approve W0 and start W1 (Neural Highway)?
