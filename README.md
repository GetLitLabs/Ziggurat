# Ziggurat

Constitution: [CHARTER_GOAL.md](CHARTER_GOAL.md). This is the only long-form briefing for any model.
Master lookup: [INDEX.md](INDEX.md). If it is not in the index, it is not in v1.0.0.

## What exists today

The repo skeleton only: CHARTER_GOAL.md, INDEX.md, .gitattributes, .gitignore, keys/.gitkeep, and an empty
workspace Cargo.toml. No crates are members yet, so nothing is sealed and nothing is operations-ready.
The charter file charters/ZIGGURAT_v1.0.0.txt has not been written; xtask cannot seal until it exists.

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
│   └── hcc-xtask/                  # T2 seal-charter, freeze-vendor, gates
├── src-tauri/                      # Faraday frame; hosts daemon task
├── origami/{dist,src}/             # precompiled cockpit; no runtime npm
├── scripts/nine_gates_audit.sh
├── tests/ziggurat_integration.rs
├── fixtures/healer_empty.wasm
├── keys/.gitkeep                   # operator keys live here, never in git
└── vendor/                         # cargo vendor for offline
```

## Plumb line (next)

1. `cargo run -p hcc-xtask -- seal-charter charters/ZIGGURAT_v1.0.0.txt --pubkey keys/operator.pub --sig keys/charter.sig`
2. hcc-seal verifies length, BLAKE3(b"HCC-STELLE-v2\x00" || bytes), and Ed25519.
3. `cargo run -p hcc-daemon` then `curl http://127.0.0.1:7749/v1/ziggurat/pillar/M09-RamGenie/status`
   must report the same digest as hashing the charter file with the same domain prefix.
