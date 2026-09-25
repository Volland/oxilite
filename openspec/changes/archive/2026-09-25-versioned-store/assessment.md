# Versioned store — assessment

Overhead, pros and cons, use cases and a scorecard for the features in [design.md](design.md).

Write costs per level are measured (§0). Query latencies and engineering figures are still estimates: the BEAR-B measurement of as-of latency (task 3.4) is the phase 2 gate. The baseline is `bench` `write-cost`: D1 writes about 4.8 rows per quad by default and 3.8 without the graph index.

## 0. Optional by design: levels and the cost of optionality

Versioning is the user's choice, per store. The default must stay the D1-optimized store it is today. The design therefore has four nested levels (see [design.md § Versioning levels](design.md#versioning-levels)):

| Level | For whom | Rows written per new quad (D1) | Per batch | Storage | Head queries |
|---|---|---|---|---|---|
| `off` (default) | everyone who doesn't ask | **4.8** (unchanged) | 0 | unchanged | unchanged, same SQL |
| `stamped` | "when was this added", "what's new since", incremental export, cache invalidation | **4.81 measured** (+0) | +1 row (the tick) | +≈4–5 bytes per `quads` row (≈10 % of the base table, not of the indexes) | unchanged, unless `t` is used |
| `log` | audit, time travel, change feed with deletes, undo | **6.82 measured** (+42 %: log row + `tx` index) | +2 rows (tick, commit) | log ≈ `quads` for insert-only data | unchanged; as-of 2–5× slower |
| `log` + as-of index | fast as-of on any pattern | **8.82 measured** (+83 %) | +2 rows (tick, commit) | log + 2 indexes | unchanged; faster as-of |
| `history` (phase 2) | branches, merge, sync | not built | — | — | — | log + 2 indexes | unchanged; faster as-of |

The rows-per-quad figures were measured on a local D1 (Miniflare) with `bench` `write-cost`, 5,000 triples in batches of 500 (2026-09-25). The first estimates were +21 % for `log` and +62 % for the full log. The measurement is higher because the log keeps a `tx` index for listings and diffs.

**What optionality costs us:**

| Cost | Size | Why it is small |
|---|---|---|
| Runtime cost to `off` stores | **none** | No table, column or trigger unless chosen. `base()` branches only when `as_of` is set. A golden test pins the default SQL. |
| D1 statement budget | 0 for `off`; 2 (`stamped`) or 3 (`log`+) of the 50 per batch | Commit statements only in versioned batches. |
| Schema variants | 4 levels, nested | Same pattern as `graph_index` and `text_index`, which are already optional and measured per option. |
| Write path | one variant: quad inserts with a `t` column | Log capture is triggers, so no writer changes. |
| Test matrix | the largest cost: 4 levels × 4 backends | Full suite on `off` and `history` only. Targeted suites on `stamped` and `log`. `write-cost` per level. |
| Migration and upgrade code | small | `off → stamped` is a metadata-only `ALTER TABLE` (verified). Higher levels create tables and a genesis commit, with no copy unless a snapshot is asked for. |
| Documentation and API | moderate | One option and one error ("store keeps no history") shared by every frontend. |

**Monotonic timestamp without versioning (`stamped`):** this is the cheap middle ground asked for.
- It costs no extra rows per quad. On D1, which bills rows rather than bytes, that is the cost that matters.
- It gives a strictly monotonic store clock (one tick per batch) mapped to wall time.
- It records when each present quad was added.
- It cannot see deletes, so it gives no time travel.
- It is also the foundation `log` builds on, so choosing it first doesn't paint a store into a corner.

**Verdict on optionality:**
- Keep `off` as the default.
- Ship `stamped` together with `log` in phase 1. It's nearly free, and useful on its own for incremental export and change detection.
- Treat a regression in `off` stores' SQL or `write-cost` as a release blocker.

## 1. Overhead

### Write cost per new quad on D1 (rows written, index entries included)

| Configuration | Rows/quad | vs today |
|---|---|---|
| Today (default, with `gspo`) | ~4.8 | — |
| + log, PK only `(s,p,o,g,tx)` | ~5.8 | +21 % |
| + log, PK + `posg` | ~6.8 | +42 % |
| + log, PK + `posg` + `ospg` (mirrors D3) | ~7.8 | +62 % |
| Per commit, fixed (clock, `ticks`, `commits`, `refs`) | ~4 per commit | amortized over the batch |

A delete costs its current price plus one log row per index, because the log records the removal.

**Recommendation:** make the log indexes part of the level: `log` is lean, `history` is full (see §0).
- **Lean (+21 %):** as-of queries with a bound subject are fast; the rest scan the log.
- **Full (+62 %):** every as-of pattern uses an index.

### Storage

- **Insert-only data:** the log is about the same size as `quads`, so total data is about 2×.
- **High churn** (the same facts added and removed repeatedly): the log grows without bound while `quads` stays flat. At that point checkpoints or compaction become necessary. That is deferred work, and the main long-term cost.
- **Terms:** no extra storage. `terms` is already never garbage-collected, so history never points to a missing term.

### Query latency (measured)

Measured with `bench` `as-of-latency` on bundled SQLite in memory: 20,000 tickets (80,000 triples), 500 commits, 95,724 log rows, median of 15 runs, in ms (2026-09-25).

| Query | `off`, current | `log`, current | `log`, 250 commits ago | + as-of index, 250 commits ago |
|---|---|---|---|---|
| subject lookup | 0.02 | 0.02 | 0.06 (3.2×) | 0.06 (3.0×) |
| value lookup (`?t :status "blocked"`) | 1.87 | 1.93 | 18.92 (9.8×) | 2.02 (1.0×) |
| star join (4 patterns, COUNT) | 0.18 | 0.17 | 18.00 (104×) | 0.64 (3.6×) |
| group by status | 0.91 | 0.94 | 25.17 (27×) | 15.31 (17×) |

- **The present is untouched:** current-state queries cost the same at every level.
- **The lean log answers subject-bound queries at ~3×.** Patterns bound only by predicate or object scan the whole log (10–100×).
- **The as-of index brings selective patterns to 1–3.6×.** It costs 2 more rows per change on D1.
- **Aggregating a whole predicate's history stays around 17×**, about 15 ms here. Every historical row pays a `NOT EXISTS` probe. Checkpoints (materialized snapshots every N commits) would bound it.
- **Gate 5.3 ("within 5× of the present"):** met with the as-of index for selective queries, and not met for full-history aggregates. Branches would read every non-checked-out branch through the log, so `version-branches` should require the as-of index and design checkpoints before building merge. A BEAR-B run moves there as its formal gate.

### Engineering cost (rough)

| Phase | Scope | Size |
|---|---|---|
| P1 | Log, triggers, commits, `as_of`, `@version`, SERVICE version IRIs, history graph, D1 migration, tests | ~1.5–3k LOC, 2–3 weeks |
| P2 | Branches, checkout, three-way merge, SHACL gate, CLI and studio UI | ~2–3k LOC, 2–3 weeks |
| P3 | Push/pull, patch format, remotes (file, D1) | ~1.5–2k LOC, ~2 weeks |

### Ongoing maintenance

- **Stays small:** triggers mean existing and future writers need no changes.
- **New burden:**
  - Every new query feature (paths, Cypher, Datalog iteration) must also be tested in as-of mode.
  - Schema migrations now cover history tables.
  - The version-reference grammar is public API.

## 2. Pros and cons by feature

| Feature | Pros | Cons |
|---|---|---|
| **Immutable log** | Full audit trail. Can't be bypassed (triggers). Enables every other feature. Log rows can be copied between stores unchanged (hash IDs, D2). | +21–62 % D1 write cost. The log grows forever without compaction. **Conflicts with the GDPR right to erasure**, so it needs an explicit, audited `purge` that rewrites history and breaks commit ids after that point. |
| **Time travel (as-of)** | Zero cost to head queries. Works in SPARQL, Cypher and Datalog at once through `base()`. Reproducible results: a query can be pinned to a commit. | As-of queries are 2–5× slower. Inferences (`quads_inf`) aren't versioned, so as-of with `include_inferred` needs re-materialization or is refused. Stats are head-only, so as-of plans may be worse. |
| **History as data** (PROV-O graph, `added`/`removed`) | Cheap once the log exists. Answers "who changed this and when". Fits RDF 1.2 triple terms and the studio's justifications view. | A reserved graph IRI and vocabulary (`ov:`) to maintain. Large histories need pagination in the UI. |
| **Branches** | Safe what-if editing, review before publish, parallel work by agents. Branch creation is O(1). | `checkout` rewrites `quads`, which is billed on D1. Only one branch is materialized at a time; the others are read through as-of, which is slower. The concept is new to explain to users. |
| **Merge** | Set semantics give very few conflicts (only add-vs-remove). SHACL as a merge gate is a unique selling point, and `oxilite-validate` already exists. | Blank nodes: the same content on two branches can have different labels. D21's deterministic labels help for documents but not for general data. Semantic conflicts that SHACL doesn't catch (for example, two values for a functional property) aren't detected. |
| **Push/pull** | Offline-first and multi-device use. Local SQLite ↔ D1 sync. Content-addressed commits make it idempotent. | Most network/protocol surface. Auth and remotes. Overlaps with plain dump/load. Needs a real user to justify it. |

## 3. Use cases

| Use case | Needs | Strength |
|---|---|---|
| **AI-agent memory**: what the agent believed at time T, rollback of a bad ingestion, agents proposing edits on a branch that a human merges | log, as-of, branches, merge | ★★★★★ |
| **Ontology and KG curation in Studio:** edit on a branch, then SHACL-gated merge, kgtests run per branch, "PR for knowledge" | branches, merge, history | ★★★★★ |
| **Audit and compliance:** who changed which fact and when, and the state at the time of a decision | log, history-as-data | ★★★★☆ (GDPR purge is required) |
| **Verifiable credentials (`oxilite-vc`):** was the credential or its status valid at presentation time | as-of | ★★★★☆ |
| **Reproducible analytics / ML datasets:** pin a query to a commit | as-of | ★★★☆☆ |
| **Debugging reasoning:** diff of inferences between versions | as-of, history (+ re-materialize) | ★★★☆☆ |
| **Edge/offline sync:** laptop ↔ D1, several devices | push/pull | ★★☆☆☆ (speculative until a user asks) |

## 4. Competitive position

- **Oxigraph and GraphDB:** no built-in versioning.
- **TerminusDB:** Git-for-data on graphs, but it has its own storage and query language (not SQLite, D1 or SPARQL-first).
- **Dolt:** Git for SQL; not RDF.
- **Quit Store and R43ples:** research prototypes.
- **Stardog:** had a versioning module and reportedly dropped it. That's a warning sign about demand for a *generic* feature, and a reason to lead with concrete use cases (agent memory, curation), not "versioning" as such.

**The gap:** versioned RDF with SPARQL, Cypher and Datalog, running on the edge (D1) and embedded (SQLite). Nobody occupies it.

## 5. Scorecard (1 = poor, 5 = excellent; cost rows: 5 = cheap)

| Criterion | Log + time travel (P1) | History as data (P1) | Branches + merge (P2) | Push/pull (P3) |
|---|---|---|---|---|
| User value | 5 | 4 | 4 | 3 |
| Differentiation | 4 | 4 | 5 | 4 |
| Fit with the architecture (base() hook, hash IDs, D5 batches) | 5 | 5 | 4 | 4 |
| Implementation cost | 4 | 5 | 3 | 3 |
| Runtime overhead (head path, D1 bill) | 4 (opt-in; head path 0) | 5 | 3 (checkout writes) | 4 |
| Maintenance and complexity | 4 | 4 | 3 | 2 |
| Risk (correctness, GDPR, adoption) | 3 | 4 | 3 | 2 |
| Standards alignment (SPARQL SERVICE, PROV-O, RDF 1.2, RDF Patch) | 4 | 5 | 3 | 4 |
| **Total / 40** | **33** | **36** | **28** | **26** |

## 6. Verdict

- **Build P1 now** (immutable log, time travel, history as data). It's worth it:
  - It's opt-in, and head queries cost nothing.
  - Triggers make it almost free to integrate.
  - It serves the two strongest use cases.
  - The D1 cost is bounded (+21 % lean).
- **Build P2 (branches + merge) after P1 is measured.**
  - It's the real differentiator for Studio and agent workflows.
  - Gate: as-of query latency on BEAR-B is within 5× of head, and the log overhead matches the estimates.
- **Defer P3 (push/pull)** until a concrete sync user or the studio needs it. Dump/load of a commit range covers the interim.
- **Packaging:** not a separate product. Ship it as the `versioning` store option (levels `off`, `stamped`, `log`, `history`; default `off`) and an `oxilite-version` crate, and market it as "Git for knowledge graphs, on the edge." It is also a Studio feature (a branch switcher, history panel and merge review).
- **Required before P1 ships:** an audited `purge` operation (GDPR), and documented behavior of as-of with inferences.
