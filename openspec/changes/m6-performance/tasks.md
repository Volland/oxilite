## 1. Benchmark

- [ ] 1.1 BSBM generator scripts and fixed scales (e.g. 100k, 1M, 10M triples)
- [ ] 1.2 Runner for oxilite (rusqlite, dylib, D1) and Oxigraph (RocksDB)
- [ ] 1.3 Results format and README table generation
- [ ] 1.4 D1 write-cost report per schema configuration

## 2. Tuning

- [ ] 2.1 Cardinality model review against BSBM plans
- [ ] 2.2 `(p, o)` statistics beyond `rdf:type` where they help
- [ ] 2.3 `EXPLAIN QUERY PLAN` checks in CI for index-only scans

## 3. Text search

- [ ] 3.1 FTS5 table and triggers (opt-in)
- [ ] 3.2 SPARQL text function compiled to `MATCH`
- [ ] 3.3 Tests, `lat.md/` update, `lat check`
