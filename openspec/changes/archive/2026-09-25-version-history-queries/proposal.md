## Status

Built (2026-09-25). Follows the archived phase 1 (`2026-09-25-versioned-store`), which built the change
log, as-of queries in SPARQL and Datalog, and history through `history` / `changes` / `diff`.

## Why

Phase 1 exposes history through store methods, and past versions to SPARQL and Datalog. Three gaps remain:
- **History as data.** History cannot be queried like the rest of the dataset, for example "who removed this
  triple, and when".
- **Datalog over time.** Datalog cannot mix versions per atom or reason over commits.
- **Cypher.** Cypher, which reads the quad table directly, cannot read the past at all.

## What Changes

- A virtual graph `<oxilite:history>` in SPARQL: commits (their ticks) as PROV-O activities, and their
  changes as `oxl:added` / `oxl:removed` triple terms, compiled to joins over `ticks`, `commits` and `quad_log`.
  A write tick records its time, author and message as terms (one more statement per batch), so the history
  reads as RDF.
- Datalog:
  - an `at "REF"` / `at ?c` suffix on atoms, with a safety rule for `at ?c`;
  - built-in relations `commit`, `added`, `removed`, `branch`.
- A version option for Cypher: its direct quad reads go through the as-of source.

## Capabilities

### Modified Capabilities
- `versioned-store`: history as data.
- `datalog-query`: versioned atoms and history relations.
- `cypher-query`: a version option.

## Impact

- **Code:** `oxilite-core` (compiler: virtual graph), `oxilite-datalog` (lexer, parser, safety, SQL) and
  `oxilite-cypher` (`exec.rs` quad reads).
- **Cost:** no schema change and no write cost.
