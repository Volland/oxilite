## 1. Update compilation

- [ ] 1.1 INSERT DATA / DELETE DATA through the write path
- [ ] 1.2 DELETE/INSERT … WHERE through `update_buffer` (stage, delete, insert, clear) in one atomic request, including USING
- [ ] 1.3 Template validity (subjects IRI/bnode, predicates IRI, bound variables) and graph variables in templates
- [ ] 1.4 Fresh blank nodes per solution in INSERT templates
- [ ] 1.5 CLEAR / DROP / CREATE / ADD / MOVE / COPY; LOAD reports unsupported
- [ ] 1.6 Native-only fallback: spareval `prepare_delete_insert` inside an interactive transaction

## 2. D1 backend

- [ ] 2.1 `oxilite-d1` crate: `AsyncBackend` over `worker::D1Database` (`batch`, `raw`/`results`, TEXT ids)
- [ ] 2.2 D1 capabilities (statement size, statements per request, no UDFs, no interactive transactions)
- [ ] 2.3 Schema migration file generated from `create_schema`
- [ ] 2.4 Example Rust Worker with a SPARQL endpoint (GET/POST query, POST update) and `wrangler.toml`

## 3. wasm core

- [ ] 3.1 `oxilite-wasm` crate exposing step machines (JSON requests/responses) through wasm-bindgen
- [ ] 3.2 Build for bundler/web (Workers) and nodejs targets

## 4. Verification

- [ ] 4.1 W3C update suite on rusqlite
- [ ] 4.2 D1 engine in the compatibility harness via `wrangler dev --local`; corpus plus query/update suites
- [ ] 4.3 Large-id and large-batch D1 tests
- [ ] 4.4 Update `lat.md/` and run `lat check`
