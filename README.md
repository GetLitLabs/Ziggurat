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
| `hcc-iedb-stele` | Append-only hash-chained stele in SQLite WAL; abort triggers; genesis embeds the charter |
| `hcc-krypt` | RMK → KEK (BLAKE3 `derive_key`, context `HCC-KRYPT-STRATUM-v2`) → DEK (ChaCha20-Poly1305 wrap) → FEK (wrapped under DEK; AES-256-GCM data); random nonces; zeroize on drop. Not wired into the daemon yet |
| `hcc-daemon` | Loopback-only `GET /v1/ziggurat/pillar/{module_id}/status` on 127.0.0.1:7749; opens the stele on a sealed boot |

No dispatch, BATON, Vanguard, Tauri, or cockpit yet. Nothing here is operations-ready until
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
