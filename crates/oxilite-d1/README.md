<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-d1

[![crates.io](https://img.shields.io/crates/v/oxilite-d1.svg)](https://crates.io/crates/oxilite-d1) [![docs.rs](https://img.shields.io/docsrs/oxilite-d1)](https://docs.rs/oxilite-d1) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**Cloudflare D1 backend for oxilite in Rust Workers** ([worker-rs](https://github.com/cloudflare/workers-rs)). Run an Oxigraph-compatible SPARQL store on a D1 database: every query is one D1 call, every update one atomic `batch()`.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-d1) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

## Usage

```toml
[dependencies]
oxilite = { version = "0.2", default-features = false, features = ["d1"] }
worker = { version = "0.8", features = ["d1"] }
```

```rust
use oxilite::{d1::D1Backend, AsyncStore};
use worker::*;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let store = AsyncStore::open_existing(D1Backend::new(env.d1("DB")?))
        .await.map_err(|e| e.to_string())?;
    let q = req.url()?.query_pairs().find(|(k, _)| k == "query").map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| "ASK { ?s ?p ?o }".into());
    let out = store.query_output(q.as_str(), &Default::default()).await.map_err(|e| e.to_string())?;
    Response::ok(oxilite_core::json::output_to_sparql_json(&out).map_err(|e| e.to_string())?)
}
```

Create the schema as a D1 migration, from `oxilite_d1::migration_sql(graph_index)` or with `npx oxilite-d1 schema > migrations/0001_oxilite.sql`, then open the store with `AsyncStore::open_existing` so no DDL runs per request.

## How it fits D1

- Atomic requests run as one `batch()` (D1's only transaction); single reads use `raw()`.
- 64-bit ids travel as TEXT, because D1 returns JavaScript numbers.
- SQL stays within D1's statement, parameter and batch limits (`Capabilities::d1()`, tunable with `D1Backend::with_capabilities`), and writes never read back, which keeps billed rows low.

The details, and tips such as dropping the graph index, are in the [D1 guide](https://github.com/Volland/oxilite#using-oxilite-with-cloudflare-d1). A complete Worker with `/sparql`, `/update`, `/load` and `/explain` is in [`examples/d1-worker`](https://github.com/Volland/oxilite/tree/main/examples/d1-worker). For TypeScript Workers and Durable Objects, use [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1).

## The oxilite family

oxilite is an Oxigraph-compatible RDF database and SPARQL 1.1 engine that stores its data in SQLite, so it runs anywhere SQLite runs: in-process, on a system or vendor `libsqlite3`, on Cloudflare D1 and in Durable Objects. The same data can be queried with SPARQL and openCypher, reasoned over with RDFS / OWL, and validated with SHACL and ShEx. Read the overview on **[oxilitedb.com](https://oxilitedb.com)** and the full guide in the [main README](https://github.com/Volland/oxilite#readme).

| Package | What it is for |
|---|---|
| [`oxilite`](https://crates.io/crates/oxilite) | The store: a drop-in for `oxigraph::store::Store`, plus `AsyncStore` for D1 |
| [`oxilite-core`](https://crates.io/crates/oxilite-core) | The sans-IO core: term encoding, schema, SPARQL → SQL compiler and planner |
| [`oxilite-rusqlite`](https://crates.io/crates/oxilite-rusqlite) | In-process backend with a bundled SQLite (the default) |
| [`oxilite-dylib`](https://crates.io/crates/oxilite-dylib) | Backend that loads your own `libsqlite3` at runtime |
| [`oxilite-d1`](https://crates.io/crates/oxilite-d1) | Cloudflare D1 backend for Rust Workers |
| [`oxilite-cypher`](https://crates.io/crates/oxilite-cypher) | openCypher over the same data, OWL- and SHACL-aware |
| [`oxilite-reason`](https://crates.io/crates/oxilite-reason) | OWL 2 RL materialization with `reasonable` |
| [`oxilite-validate`](https://crates.io/crates/oxilite-validate) | SHACL and ShEx validation with rudof |
| [`oxilite-cli`](https://crates.io/crates/oxilite-cli) | The `oxilite` command and a SPARQL endpoint like `oxigraph serve` |
| [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node) | Node.js bindings, API of Oxigraph's JS package |
| [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1) | Cloudflare D1 and Durable Objects from TypeScript (WebAssembly core) |
| [`@oxilite/common`](https://www.npmjs.com/package/@oxilite/common) | RDF/JS terms and shared TypeScript types |

## License

Dual-licensed under [MIT](https://github.com/Volland/oxilite/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/Volland/oxilite/blob/main/LICENSE-APACHE), at your option, like Oxigraph.
