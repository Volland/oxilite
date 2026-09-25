## Status

Phase 1 of versioning, built and measured (2026-09-25). The later phases are separate planned changes:
`version-history-queries` (history as an RDF graph, Datalog history relations, a Cypher version option),
`version-branches` (phase 2) and `version-sync` (phase 3). [assessment.md](assessment.md) holds the evaluation
and the measurements; [design.md](design.md) the approach for all phases, with the phase-1 differences at its top.

## Why

oxilite updates `quads` in place, so a store cannot say what it contained yesterday, who changed a fact, or
undo a bad ingestion. Agent memory and ontology curation in the studio both need that history. No other RDF
store offers it on SQLite and D1, where every row written is billed.

## What Changes

- **Versioning is optional, in nested levels, recorded in the store:**
  - `off` (default): the store as before, with identical SQL (pinned by a golden test) and write cost.
  - `stamped`: a store clock. Every write is a tick with time, author and message, and every quad records the
    tick that added it. No extra rows per triple.
  - `log`: an immutable, trigger-written change log. Genesis records the whole store. An optional as-of index
    speeds up past-version queries.
- **Explicit level changes** through the API, the CLI and D1 migrations. Opening never changes a level.
  Downgrades freeze the history; `allow_loss` deletes it. Resuming records the gap.
- **Time travel:**
  - SPARQL takes `as_of`, and `SERVICE <oxilite:version/REF>` compares versions in one query.
  - Datalog takes `@version` and `as_of`.
  - `oxilite serve` answers `/query?version=`.
- **History:** `history`, `changes` and `diff` (RDF Patch lines in the CLI), commit author and message, and an
  audited `purge`.
- **Every surface:** the Rust stores, `@oxilite/d1`, `@oxilite/node`, the CLI (`oxilite versioning …`,
  `oxilite schema`), and `npx oxilite-d1 schema --versioning` / `versioning-migration`.

## Capabilities

### New Capabilities
- `versioned-store`: levels, the store clock, the change log, as-of queries, history, level changes, purge.

### Modified Capabilities
- `datalog-query`: the `@version` directive.

## Impact

- **Code:**
  - `oxilite-core`: the `version` module, schema, writer, update, compiler, fallback and stats.
  - `oxilite`: the versioned backend and the store API.
  - `oxilite-datalog`, `oxilite-wasm`, `oxilite-cli`, `oxilite-node`, and `@oxilite/d1` / `@oxilite/common`.
- **Cost on D1:** measured with `write-cost`:

  | Level | Rows per triple |
  |---|---|
  | `off` | 4.81 |
  | `stamped` | 4.81 |
  | `log` | 6.82 |
  | `log` + as-of index | 8.82 |

- **Read latency:** measured with `as-of-latency`:
  - Current-state queries are unchanged at every level.
  - Past versions of subject-bound queries take about 3× as long.
  - Past versions of other patterns take 10–100× as long without the as-of index, and 1–3.6× with it.
  - Aggregations over a whole predicate's history take about 17× as long even with the index.
- **Docs:** `lat.md` gains the Versioning section and D29–D32. The website gains two articles.
