## 1. Node package

- [ ] 1.1 `bindings/node` napi-rs crate over `blocking::Store` (bundled SQLite and dylib)
- [ ] 1.2 Term conversion to and from RDF/JS objects; `DataFactory`
- [ ] 1.3 `query` / `update` / `load` / `dump` / `add` / `delete` / `has` / `match` / `size` / `explain` / `optimize`
- [ ] 1.4 TypeScript wrapper and `.d.ts`, strict compile
- [ ] 1.5 `node:test` suite; port Oxigraph `js/test/store.test.ts` (with the compatibility harness)

## 2. D1 package

- [ ] 2.1 `packages/d1` TS driver over `oxilite-wasm` step machines and `D1Database`
- [ ] 2.2 Result conversion identical to the Node package (shared types module)
- [ ] 2.3 `init()` and the exported migration SQL
- [ ] 2.4 Tests against a D1-compatible mock over `node:sqlite`, and `wrangler dev` in CI
- [ ] 2.5 Example Worker (TypeScript) with a SPARQL endpoint

## 3. Docs

- [ ] 3.1 README sections for Node and D1 with typed examples
- [ ] 3.2 Update `lat.md/` and run `lat check`
