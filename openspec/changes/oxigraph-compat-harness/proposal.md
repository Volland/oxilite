## Why

oxilite promises to behave "as close as possible to Oxigraph". That promise only means something if it's checked mechanically on every change. Oxigraph already has the test assets needed: a W3C manifest runner, its own extra SPARQL tests, store API tests, optimizer regression queries, and a TypeScript store test suite. This change reuses all of them, and adds a differential harness that runs the same operations on both engines and compares the outputs.

## What Changes

- New dev-only crate `oxilite-compat` (not published).
  - Depends on `oxigraph` 0.5 with `default-features = false` (in-memory store, no RocksDB) and on `oxilite`.
  - Defines a small `Engine` trait implemented by both stores (load, query, update, dump, pattern scan, named-graph ops).
- Ported Oxigraph `testsuite` manifest runner (`manifest.rs`, `sparql_evaluator.rs`, `parser_evaluator.rs`).
  - Made generic over `Engine`.
  - Each W3C test is checked against its expected result, and the two engines are checked against each other.
- `rdf-tests` and Oxigraph's `testsuite/oxigraph-tests` vendored as git submodules / pinned copies.
- Ported `lib/oxigraph/tests/store.rs` against `oxilite::blocking::Store` with only the import changed. This is the executable proof of API drop-in compatibility.
- Ported `lib/oxigraph/tests/optimizer_regression.rs` queries into the differential harness.
- A differential corpus: generated datasets plus query families (BGP shapes, filters, optional, union, aggregates, paths, updates) run on both engines, comparing results as multisets (or isomorphic graphs).
- Ported `js/test/store.test.ts` against `@oxilite/node` (with change `typescript-bindings`).
- A compatibility report (`COMPATIBILITY.md`, generated) listing every divergence with its reason, and an allow-list file for known, justified differences.

## Capabilities

### New Capabilities
- `oxigraph-compatibility`: the requirements for checking oxilite's behaviour against Oxigraph and the W3C suites, and for reporting divergences.

### Modified Capabilities
<!-- none -->

## Impact

- New dev-dependency on the `oxigraph` crate (in-memory only), and submodules `testsuite/rdf-tests` and `testsuite/oxigraph-tests`.
- CI runs the harness on native backends for every milestone, and also against local D1 (`wrangler dev`) from M3.
- Each milestone's done-criterion in `lat.md/milestones.md` is measured with this harness.
