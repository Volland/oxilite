# Milestones

Delivery plan. Each milestone ships something usable, has an OpenSpec change under `openspec/changes/`, and has an objective done-criterion.

## Oxigraph compatibility harness

Cross-cutting: the harness that runs Oxigraph's own tests and a differential corpus on oxilite and Oxigraph. Change: `oxigraph-compat-harness`; built alongside M1 and extended by every milestone.

Details in [[test-plan#Oxigraph compatibility harness]]. Every milestone's done-criterion is measured with it.

## M1 Storage core

Schema, term encoding, backends, load/dump, pattern scans and BGP+FILTER queries compiled to single SQL statements with the statistics planner. Change: `m1-storage-core`.

Scope: [[architecture#Term encoding]], [[architecture#Storage schema]], [[architecture#Write path]], [[architecture#Query planner]], the rusqlite and dlopen backends, `blocking::Store` and async `Store`. Done when W3C Turtle/N-Triples/N-Quads syntax suites round-trip through the store and the planner beats SQLite-planned SQL on a join-heavy benchmark.

## M2 Full SPARQL 1.1 query

OPTIONAL, UNION, MINUS, subqueries, aggregates, property paths, VALUES, all function tiers, the spareval fallback and `explain()`. Change: `m2-sparql-query`.

Done when at least 95% of the W3C SPARQL 1.1 query test suite passes on rusqlite, with the remainder listed and justified.

## M3 SPARQL Update and D1

Atomic SPARQL UPDATE compiled to batch SQL, the `oxilite-d1` Rust backend, the wasm core and the `@oxilite/d1` TypeScript driver, with an example Worker. Change: `m3-update-d1`.

Done when the W3C SPARQL 1.1 update suite passes on rusqlite and the same query/update tests pass against local D1 (`wrangler dev`).

Status: done. The W3C update suites pass on every engine variant, including D1 through the Miniflare sidecar; `@oxilite/d1` and the example Rust Worker (`examples/d1-worker`) pass end-to-end tests on local D1.

## M4 Reasoning

TBox closure, RDFS / OWL-QL query rewriting and opt-in OWL 2 RL materialization. Change: `m4-reasoning`.

Done when hand-written entailment tests pass and materialization agrees with `reasonable` on sample ontologies.

## M5 Validation

rudof `srdf` traits on the native store and the D1 prefetch adapter for SHACL and ShEx. Change: `m5-validation`.

Done when rudof's own SHACL/ShEx test suites pass against an oxilite-backed store.

## M6 Performance

BSBM benchmark against Oxigraph/RocksDB, planner tuning and an optional FTS5 text index. Change: `m6-performance`.

Done when a comparison table is published in the README.

## TypeScript bindings

`@oxilite/node` (napi-rs) for Node.js and `@oxilite/d1` for Workers, both fully typed. Change: `typescript-bindings`; the Node part depends on M1, the D1 part on M3.

Done when both packages have passing test suites and typed examples in the README.

Status: done. Both packages are tested with vitest (Oxigraph's own JS store tests included for `@oxilite/node`, Miniflare D1 for `@oxilite/d1`), and both example Workers pass end-to-end tests.
