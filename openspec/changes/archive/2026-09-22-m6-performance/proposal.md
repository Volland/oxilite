## Why

Query speed is a primary goal. It has to be measured against Oxigraph on a recognized benchmark, and the planner has to be tuned against the results. Text search is also a frequent need, and SQLite's FTS5 (available on D1) can provide it cheaply.

## What Changes

- BSBM (the Berlin SPARQL Benchmark, which Oxigraph publishes numbers for):
  - explore and business-intelligence mixes;
  - run on oxilite (rusqlite, dylib, D1) and on Oxigraph with RocksDB;
  - repeatable scripts and a results table in the README.
- Planner tuning from benchmark findings (cardinality model, `(p, o)` statistics, index choice checks).
- An optional FTS5 full-text index over string literals, with a query extension (a `text:match`-style function) compiled to FTS5 `MATCH`.
- A write-cost report for D1: rows written per inserted triple, including index entries.

## Capabilities

### New Capabilities
- `benchmarks`: reproducible performance measurement against Oxigraph.
- `text-search`: optional full-text search over literals.

### Modified Capabilities
<!-- none -->

## Impact

- New `bench/` directory with BSBM generator scripts; an optional FTS5 table created when the text index is enabled.
