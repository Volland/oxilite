## Why

Most D1 users write Workers in TypeScript, and many RDF applications run on Node.js. oxilite needs typed JavaScript packages whose API matches Oxigraph's JS package, so Oxigraph users can switch by changing an import.

## What Changes

- `@oxilite/node`: napi-rs native addon over `blocking::Store` (bundled SQLite, or a user-supplied `libsqlite3` path), with a TypeScript API mirroring Oxigraph's JS `Store`:
  - `query`, `update`, `load`, `dump`, `add`, `delete`, `has`, `match`, `size`
  - plus oxilite extras: `explain`, `optimize`
  - terms follow RDF/JS (`termType`, `value`, `language`, `datatype`), with a `DataFactory` included.
- `@oxilite/d1`: a TypeScript driver running the wasm core (`oxilite-wasm`) against a `D1Database` binding, with the same API returning Promises, plus a helper to apply the schema migration.
- Both packages are built with `tsc`, ship `.d.ts` files, and are tested with vitest (Oxigraph's own JS tests use it). The D1 driver is tested against a local D1 (Miniflare, the engine behind `wrangler dev`), locally and in CI.

## Capabilities

### New Capabilities
- `node-bindings`: oxilite for Node.js with a typed, Oxigraph-compatible API.
- `d1-typescript-driver`: oxilite for TypeScript Workers on D1.

### Modified Capabilities
<!-- none -->

## Impact

- New `bindings/node` (Rust napi crate + TS package) and `packages/d1` (TS package + wasm artifacts).
- The Node part depends on M1; the D1 driver depends on the `oxilite-wasm` crate from M3.
- The compatibility harness runs Oxigraph's `js/test/store.test.ts` against `@oxilite/node`.
