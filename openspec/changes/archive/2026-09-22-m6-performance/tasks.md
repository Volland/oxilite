## 1. Benchmark

- [x] 1.1 BSBM generator scripts and fixed scales (e.g. 100k, 1M, 10M triples): `bench/bsbm.sh [products]` with the official BSBM tools (about 350 triples per product)
- [x] 1.2 Runner for oxilite (rusqlite, dylib, D1) and Oxigraph (RocksDB): `oxilite serve` driven by the BSBM test driver
- [x] 1.3 Results format and README table generation
- [x] 1.4 D1 write-cost report per schema configuration

## 2. Tuning

- [x] 2.1 Cardinality model review against BSBM plans
- [x] 2.2 `(p, o)` statistics beyond `rdf:type` where they help
- [x] 2.3 `EXPLAIN QUERY PLAN` checks in CI for index-only scans

## 3. Text search

- [x] 3.1 FTS5 table and triggers (opt-in)
- [x] 3.2 SPARQL text function compiled to `MATCH`
- [x] 3.3 Tests, `lat.md/` update, `lat check`
