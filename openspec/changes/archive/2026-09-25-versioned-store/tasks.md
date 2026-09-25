Status: done (2026-09-25). Later phases are the planned changes `version-history-queries`,
`version-branches` and `version-sync`.

## 1. Schema and log

- [x] 1.1 `StoreOptions::versioning` levels (`off`, `stamped`, `log`) recorded in `oxilite_meta`, read with the statistics
- [x] 1.2 `stamped`: `quads.t` by `ALTER TABLE`, `ticks`, one tick per atomic write, `t` in both quad inserts, optional `quads_t` index
- [x] 1.3 Level status; the stored level wins at open, a higher level applied only to an empty store
- [x] 1.4 Upgrades: `off→stamped` (ALTER), `→log` (genesis records the whole store), as-of index option
- [x] 1.5 Downgrades: soft by default (freeze history, keep ticks and column), `allow_loss` drops them; resume records the gap
- [x] 1.6 `quad_log`, `quad_log_tx`, `commits`; immutability triggers on `quad_log`, `ticks` and `commits`
- [x] 1.7 Capture triggers on `quads`, net effect per tick, guarded by the purge flag; verified on Miniflare D1 and the system SQLite
- [x] 1.8 Tick opened by the store for every request writing `quads` (`version::prepare`, `VersionedBackend`, the wasm job context); one tick per interactive transaction
- [x] 1.9 Commit author and message on the store API, the CLI and the JavaScript packages
- [x] 1.10 Golden test: every W3C query and update and the batch writer compile, for an `off` store, to the SQL and planner notes of 0.3.1

## 2. Reading history

- [x] 2.1 Version references (`HEAD`, `main`, `~n`, `#tick`, `@dateTime`), checked against genesis, freezes and gaps
- [x] 2.2 `QueryOptions::as_of`; `Entailment::base()` reads the log at a tick (compiler, fallback evaluator, DESCRIBE, paths)
- [x] 2.3 As-of refused with inferences or reasoning, on a store without history, and inside updates
- [x] 2.4 `SERVICE <oxilite:version/…>` compiled as a group at another version
- [x] 2.5 Datalog `@version` directive and `Options::as_of`; materialization with a version refused
- [x] 2.6 Audited `purge`
- [x] 2.7 `history`, `changes`, `diff` on every surface; RDF Patch output in the CLI; `/query?version=` on `oxilite serve`

## 3. Surfaces, tests, docs, measurement

- [x] 3.1 CLI `--versioning`, `--as-of`, `oxilite versioning status|set|log|changes|diff|purge|migration`, `oxilite schema`; `@oxilite/d1` and `@oxilite/node`; `npx oxilite-d1 schema --versioning` and `versioning-migration`
- [x] 3.2 Versioning suites on bundled SQLite, the system SQLite, the D1 code path, Miniflare D1, `@oxilite/d1`, `@oxilite/node` and Datalog
- [x] 3.3 `write-cost` per level on a local D1
- [x] 3.4 `as-of-latency`: current and past versions, with and without the as-of index (synthetic; BEAR-B moves to `version-branches`)
- [x] 3.5 `lat.md`: Versioning section, D29–D32, M9, test specs; READMEs; website section and two articles
