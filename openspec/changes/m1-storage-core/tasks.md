## 1. Workspace

- [ ] 1.1 Create the Cargo workspace (`oxilite-core`, `oxilite`, `oxilite-rusqlite`, `oxilite-dylib`), pinned to the Oxigraph 0.5 crate family with `rdf-12`/`sparql-12`
- [ ] 1.2 Set up CI (fmt, clippy, tests on Linux/macOS, `cargo check --target wasm32-unknown-unknown -p oxilite-core`)

## 2. Core primitives

- [ ] 2.1 Error type, and the `SqlValue`/`Statement`/`Request`/`Response`/`Capabilities` sans-IO types
- [ ] 2.2 Term encoding: tags, xxh3 hashing, inline integers and booleans, typed side values, decoding
- [ ] 2.3 Schema DDL with the collision trigger and options (graph index)
- [ ] 2.4 Write path: encoded quads → chunked `INSERT OR IGNORE` / `DELETE` statements under `max_sql_len`
- [ ] 2.5 Statistics: load request, refresh request (`optimize()`)

## 3. Compiler (M1 subset)

- [ ] 3.1 Blocks: FROM items, WHERE, bindings, stages, sealing into subqueries
- [ ] 3.2 BGP compilation with graph scopes (default, fixed, variable, union/multi-FROM dedup)
- [ ] 3.3 Greedy planner with stats and heuristics, enforced with `CROSS JOIN`
- [ ] 3.4 Expressions tier A (comparisons, logic, arithmetic, string, numeric and date functions, casts) with SPARQL error semantics
- [ ] 3.5 FILTER, projection, DISTINCT/REDUCED, ORDER BY, LIMIT/OFFSET
- [ ] 3.6 Query forms: SELECT, ASK, CONSTRUCT (template instantiation), DESCRIBE
- [ ] 3.7 Result decoding with batched term resolution, and ids as TEXT when requested

## 4. Store API

- [ ] 4.1 Step-machine jobs and sync/async drivers
- [ ] 4.2 `blocking::Store` mirroring `oxigraph::store::Store` (new, open, insert, extend, remove, contains, len, is_empty, quads_for_pattern, iter, named graphs, clear, load_from_reader, dump_to_writer, dump_graph_to_writer, query, bulk_loader, optimize, backup)
- [ ] 4.3 Async `Store<B>` with the same method names
- [ ] 4.4 Fallback evaluator: spareval `QueryableDataset` over the sync backend
- [ ] 4.5 `explain()` returning the generated SQL, plus the fallback reason

## 5. Backends

- [ ] 5.1 rusqlite backend (bundled): savepoint-based atomic requests, WAL, UDFs (`oxilite_regex`, `oxilite_replace`, `oxilite_hash`, `oxilite_encode_for_uri`, Unicode `upper`/`lower`)
- [ ] 5.2 dylib backend: `libloading` binding of the needed C API, and error reporting for missing symbols
- [ ] 5.3 SQLite version check (≥ 3.37 for STRICT)

## 6. Verification

- [ ] 6.1 Unit tests (encoding, writer, compiler) referenced from `lat.md/tests.md`
- [ ] 6.2 Oxigraph compatibility harness M1 slice (see change `oxigraph-compat-harness`): store API parity tests, and W3C syntax suites round-tripped through both stores
- [ ] 6.3 Planner benchmark: oxilite planner against SQLite planning on a join-heavy generated dataset
- [ ] 6.4 Update `lat.md/` and run `lat check`
