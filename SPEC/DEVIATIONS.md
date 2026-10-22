# Ziggurat v1.0.0 — recorded deviations

Departures from the `INDEX.md` §20 build order. Each row records who accepted it and what it obliges.

| ID | Date | Deviation | Status | Obligation |
|---|---|---|---|---|
| D1 | 2026-10-22 | Built `hcc-daemon` GET `/status` (item 5) before `hcc-iedb-stele` (item 4), so the first daemon booted with no WAL and no genesis stele. | **Accepted** by the operator; **closed** by P3 (`hcc-iedb-stele` + daemon genesis boot): the status endpoint is read-only, touches no ledger, and the fail-closed path was proven by a one-byte tamper test. | P3 (`hcc-iedb-stele`) is the missing half of boot and comes before krypt, BATON, Vanguard, dispatch, and Tauri. Closed when the daemon opens the WAL, appends genesis on a royal-sealed boot, and reports `stele_genesis` in `/status`. |
