## 1. Harness skeleton

- [x] 1.1 Create `testsuite/` crate `oxilite-compat` (publish = false) with dev-deps `oxigraph` (0.5, `default-features = false`) and `oxilite`
- [x] 1.2 Add `rdf-tests` submodule at the commit Oxigraph 0.5 pins; copy `oxigraph-tests` with license notices
- [x] 1.3 `Engine` trait with `OxigraphEngine` and `OxiliteEngine<B>` implementations

## 2. Ported Oxigraph runner

- [x] 2.1 Port `manifest.rs`, `evaluator.rs`, `report.rs` unchanged where possible
- [x] 2.2 Port `parser_evaluator.rs`: syntax suites load → dump → isomorphism on each engine
- [x] 2.3 Port `sparql_evaluator.rs` generic over `Engine`, with the three verdicts (expected, upstream sanity, cross-engine)
- [x] 2.4 Import Oxigraph's ignore lists from `testsuite/tests/sparql.rs` as the "upstream" baseline

## 3. API parity suites

- [x] 3.1 Port `lib/oxigraph/tests/store.rs` against `oxilite::blocking::Store` (import change only); list excluded RocksDB-specific tests
- [x] 3.2 Port `optimizer_regression.rs` queries into the differential corpus
- [x] 3.3 Port `js/test/store.test.ts` against `@oxilite/node` (after change `typescript-bindings`)

## 4. Differential corpus

- [x] 4.1 Seeded dataset generator (all literal kinds, lang tags, bnodes, named graphs, cross-graph duplicates, hierarchies, cycles)
- [x] 4.2 M1 query families: BGP shapes × filters × GRAPH/FROM/union-default-graph × modifiers
- [x] 4.3 Run matrix: stats {absent, present} × planner {oxilite, sqlite} × backend {rusqlite, dylib}
- [x] 4.4 Mismatch report containing query, data seed, both results, and oxilite's `explain()` SQL

## 5. Reporting and CI

- [x] 5.1 `allowlist.toml` format (id, reason, decision link) with "stale entry" detection
- [x] 5.2 Generate `COMPATIBILITY.md` (pass rates per suite, divergence table)
- [x] 5.3 CI job running the harness on every PR
- [x] 5.4 (M3) D1 engine via `wrangler dev --local`; run the corpus and the query/update suites on it
