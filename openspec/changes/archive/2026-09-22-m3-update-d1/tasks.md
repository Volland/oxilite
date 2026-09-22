## 1. Update compilation

- [x] 1.1 INSERT DATA / DELETE DATA through the write path
- [x] 1.2 DELETE/INSERT … WHERE through `update_buffer` (stage, delete, insert, clear) in one atomic request, including USING
- [x] 1.3 Template validity (subjects IRI/bnode, predicates IRI, bound variables) and graph variables in templates
- [x] 1.4 Fresh blank nodes per solution in INSERT templates
- [x] 1.5 CLEAR / DROP / CREATE / ADD / MOVE / COPY; LOAD reports unsupported
- [x] 1.6 Native-only fallback: spareval `prepare_delete_insert` inside an interactive transaction

## 2. D1 backend

- [x] 2.1 `oxilite-d1` crate: `AsyncBackend` over `worker::D1Database` (`batch`, `raw`/`results`, TEXT ids)
- [x] 2.2 D1 capabilities (statement size, statements per request, no UDFs, no interactive transactions)
- [x] 2.3 Schema migration file generated from `create_schema`
- [x] 2.4 Example Rust Worker with a SPARQL endpoint (GET/POST query, POST update) and `wrangler.toml`

## 3. wasm core

- [x] 3.1 `oxilite-wasm` crate exposing step machines (JSON requests/responses) through wasm-bindgen
- [x] 3.2 Build for bundler/web (Workers) and nodejs targets

## 4. Verification

- [x] 4.1 W3C update suite on rusqlite
- [x] 4.2 D1 engine in the compatibility harness via `wrangler dev --local`; corpus plus query/update suites
- [x] 4.3 Large-id and large-batch D1 tests
- [x] 4.4 Update `lat.md/` and run `lat check`
