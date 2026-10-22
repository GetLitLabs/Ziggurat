# Ziggurat

Constitution: [CHARTER_GOAL.md](CHARTER_GOAL.md). This is the only long-form briefing for any model.
Master lookup: [INDEX.md](INDEX.md). If it is not in the index, it is not in v1.0.0.

## What exists today

| Piece | State |
|---|---|
| `charters/ZIGGURAT_v1.0.0.txt` | Frozen: 4660 bytes, LF only, no BOM |
| `hcc-types` | `TerraceLevel` (1..=5), `BsmcNamespace` (must contain a dot) |
| `hcc-seal` | `no_std`; domain-separated BLAKE3, Ed25519 verify, `SystemCharter<Unsealed>` / `<RoyalSealed>` |
| `hcc-xtask` | `keygen`, `sign-charter`, `seal-charter`, `hygiene` |
| `crates/hcc-seal/src/baked.rs` | Length + digest baked; **no royal seal yet**, so the charter stays Unsealed |

No daemon, stele, Tauri, or cockpit yet. Nothing here is operations-ready until
`cargo test --workspace` passes on the operator machine.

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

`hcc-daemon` GET `/v1/ziggurat/pillar/{module_id}/status` on 127.0.0.1:7749, after the
operator confirms the step 4 gate. Its `charter_blake3_hex` must equal `seal-charter`'s.
