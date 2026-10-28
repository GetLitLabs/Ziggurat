# Ziggurat v1.0.0 — recorded deviations

Departures from the `INDEX.md` §20 build order. Each row records who accepted it and what it obliges.

| ID | Date | Deviation | Status | Obligation |
|---|---|---|---|---|
| D1 | 2026-10-22 | Built `hcc-daemon` GET `/status` (item 5) before `hcc-iedb-stele` (item 4), so the first daemon booted with no WAL and no genesis stele. | **Accepted** by the operator; **closed** by P3 (`hcc-iedb-stele` + daemon genesis boot): the status endpoint is read-only, touches no ledger, and the fail-closed path was proven by a one-byte tamper test. | P3 (`hcc-iedb-stele`) is the missing half of boot and comes before krypt, BATON, Vanguard, dispatch, and Tauri. Closed when the daemon opens the WAL, appends genesis on a royal-sealed boot, and reports `stele_genesis` in `/status`. |
| D2 | 2026-10-28 | The Vanguard live set grows past the charter's three (`scribe.stele.append`, `seed.scaffold.emit`, `inspect.gate.validate`): every job whose purpose `CHARTER_GOAL.md`/`INDEX.md` define is live (INDEX §11). The frozen charter text (the model's system prompt) still lists three live jobs and M01/M06/M08 as TYPED. | **Requested** by the operator on 2026-10-28 ("keep going until those are built"). The charter is not edited: an edit is a v1.0.1 bump with a fresh seal, which only the operator can sign. | Open until the operator re-seals: write `charters/ZIGGURAT_v1.0.1.txt` with the current live set, sign and seal it, then `xtask charter-version`. Until then the code and INDEX §11 are the source of truth for what runs. |
