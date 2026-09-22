## Why

Two things are needed next. SPARQL UPDATE has to work where interactive transactions don't exist. And D1, the platform that motivated the project, needs first-class backends for both Rust Workers and TypeScript Workers. D1's only atomic unit is `db.batch()`, so every update must compile to self-reading SQL that fits in one batch.

## What Changes

- Compile SPARQL UPDATE: INSERT DATA, DELETE DATA, DELETE/INSERT … WHERE (including DELETE WHERE and USING), CLEAR, DROP, CREATE, and the ADD / MOVE / COPY desugarings.
  - The whole update request goes to one atomic request.
  - DELETE/INSERT stages through `update_buffer`, so WHERE is evaluated before any modification.
- Fresh blank nodes in INSERT templates, generated per solution in SQL.
- New crate `oxilite-d1`: an async backend over `worker::D1Database`.
  - Atomic requests go through `batch()`.
  - 64-bit ids travel as TEXT.
  - Statement size and per-invocation query limits are respected.
- New crate `oxilite-wasm`: wasm-bindgen export of the sans-IO core (request/response step machines as JSON) for JavaScript drivers.
- Example Rust Worker and a `wrangler.toml`; the schema can be applied as a D1 migration.
- Documented non-atomic bulk loading across batches for large imports.

## Capabilities

### New Capabilities
- `sparql-update`: atomic SPARQL 1.1 Update on every backend.
- `d1-backend`: running oxilite on Cloudflare D1.

### Modified Capabilities
<!-- none -->

## Impact

- New crates `oxilite-d1` (target wasm32) and `oxilite-wasm`.
- The compatibility harness gains a D1 engine (`wrangler dev --local`).
- The TypeScript D1 driver (change `typescript-bindings`) depends on `oxilite-wasm`.
