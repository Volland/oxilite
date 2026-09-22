## Context

See proposal.md for the motivation. Oxigraph's repository already contains the assets we need:
- `testsuite/` has a W3C manifest runner (`manifest.rs`, `sparql_evaluator.rs`, `parser_evaluator.rs`, `evaluator.rs`, `report.rs`), plus `rdf-tests` and Oxigraph-specific `oxigraph-tests` manifests. `testsuite/tests/sparql.rs` lists the ignored tests.
- `lib/oxigraph/tests/store.rs` holds API tests, and `optimizer_regression.rs` holds query regressions.
- `js/test/store.test.ts` holds the JS store tests.

The runner is tied to `oxigraph::store::Store`.

## Goals / Non-Goals

**Goals:**
- Reuse Oxigraph's own tests as much as possible, adapted rather than rewritten.
- Compare with Oxigraph itself, not just with expected results, so behaviour outside the W3C suites is also covered.
- Make every divergence visible and justified.

**Non-Goals:**
- Performance comparison (M6 benchmarks).
- Testing Oxigraph-only features: RocksDB internals, `backup` layout, and the HTTP server.

## Decisions

### Harness crate layout
```
testsuite/
  Cargo.toml            # oxilite-compat, publish = false
  rdf-tests/            # git submodule (w3c/rdf-tests), same commit Oxigraph pins
  oxigraph-tests/       # copied from oxigraph/testsuite/oxigraph-tests (Apache-2.0/MIT)
  allowlist.toml        # accepted divergences: test id / corpus id → reason + decision link
  src/
    engine.rs           # Engine trait + impls: OxigraphEngine, OxiliteEngine<B>
    manifest.rs         # ported from Oxigraph
    sparql_evaluator.rs # ported, generic over Engine
    parser_evaluator.rs # ported (syntax suites: load → dump → isomorphic, per engine)
    differential.rs     # corpus runner: same data + query on two engines, compare
    corpus/             # generated datasets + query families (*.rq, *.ru)
    report.rs           # ported; also emits COMPATIBILITY.md
  tests/
    w3c.rs              # manifests × engines
    oxigraph_store_api.rs   # ported lib/oxigraph/tests/store.rs, `use oxilite::blocking as oxigraph_store`
    optimizer_regression.rs # ported queries → differential
    differential.rs
```

### Engine trait
This is the smallest surface the runner needs:
- `load(format, data, base, graph)`
- `query(q, options) -> QueryResults`
- `update(u)`
- `quads() -> Dataset`
- `named_graphs`, `clear`

Both engines return `spareval::QueryResults` / `oxrdf` types, since oxilite reuses them (decision D9). The ported runner's comparison code (canonical blank-node mapping, ordered and unordered solution comparison) therefore works as-is.

### Three verdicts per test
Each test gets three comparisons:
1. oxilite against the expected result
2. Oxigraph against the expected result (a sanity check)
3. oxilite against Oxigraph

A test counts as an oxilite failure when comparison 1 fails while comparison 2 passes, or when comparison 3 fails. When both engines fail against the expected result, the test is reported as "upstream".

### Differential corpus
- **Datasets** are generated deterministically from a seed. They include typed literals of every kind, language tags, blank nodes, multiple named graphs, duplicate triples across graphs, deep class hierarchies and cycles (for paths).
- **Query families** are templated by milestone:
  - M1: BGP shapes (star, chain, cycle, cartesian) × filters
  - M2: OPTIONAL, UNION, MINUS, aggregates, paths, subqueries, ORDER BY
  - M3: updates
- Every query also runs with `stats = {absent, present}` × `planner = {oxilite, sqlite}`.
- On a mismatch, the report includes `explain()` output (the generated SQL).

### Store API tests port
Build with `use oxilite::blocking::Store` in place of `use oxigraph::store::Store`. The only exclusions are tests of RocksDB-specific methods (`backup` binary layout, `flush`, `compact`, `rocksdb_bc_*` data), and they are listed in the allow-list.

### JS tests port
Copy `js/test/store.test.ts` and swap `import … from "../pkg/oxigraph.js"` for the `@oxilite/node` entry point, then run it with `node --test` after `tsc`. Differences go in `allowlist.toml` under `[js]`.

### D1 runs
From M3, `OxiliteEngine<D1Backend>` talks to a `wrangler dev --local` Worker that exposes a minimal request/response endpoint for the sans-IO driver. The same corpus and suites run through it.

## Risks / Trade-offs

- **Upstream runner code changes with Oxigraph versions.** Pin it to the Oxigraph 0.5 tag we depend on and re-port when upgrading.
- **Blank-node labels differ between engines.** Always compare with isomorphism / canonical bnode mapping, never by label.
- **ORDER BY ties return in a different order.** Compare only the ordered prefix defined by the sort keys (Oxigraph's runner already does this for W3C tests).
- **Licenses.** Oxigraph is MIT/Apache-2.0 and the W3C tests use the W3C test suite license; keep the notices in `testsuite/`.
