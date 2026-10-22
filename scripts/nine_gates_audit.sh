#!/usr/bin/env bash
# M11 nine-gates audit (INDEX.md §14). The checks live in `hcc-xtask gates` so they run the
# same way on Windows (`cargo run -p hcc-xtask -- gates`). Exits non-zero unless every gate passes.
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --offline --quiet -p hcc-xtask -- gates "$@"
