## 1. Node package

- [x] 1.1 `bindings/node` napi-rs crate over `blocking::Store` (bundled SQLite and dylib)
- [x] 1.2 Term conversion to and from RDF/JS objects; `DataFactory`
- [x] 1.3 `query` / `update` / `load` / `dump` / `add` / `delete` / `has` / `match` / `size` / `explain` / `optimize`
- [x] 1.4 TypeScript wrapper and `.d.ts`, strict compile
- [x] 1.5 vitest suite; port Oxigraph `js/test/store.test.ts` (with the compatibility harness)

## 2. D1 package

- [x] 2.1 `packages/d1` TS driver over `oxilite-wasm` step machines and `D1Database`
- [x] 2.2 Result conversion identical to the Node package (shared types module)
- [x] 2.3 `init()` and the exported migration SQL
- [x] 2.4 Tests against a local D1 (Miniflare, the engine behind `wrangler dev`), in CI
- [x] 2.5 Example Worker (TypeScript) with a SPARQL endpoint

## 3. Docs

- [x] 3.1 README sections for Node and D1 with typed examples
- [x] 3.2 Update `lat.md/` and run `lat check`
