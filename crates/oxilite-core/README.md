<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-core

[![crates.io](https://img.shields.io/crates/v/oxilite-core.svg)](https://crates.io/crates/oxilite-core) [![docs.rs](https://img.shields.io/docsrs/oxilite-core)](https://docs.rs/oxilite-core) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**The I/O-free core of oxilite:** RDF term encoding, the SQLite schema, and a SPARQL 1.1 → SQL compiler with its own join planner. It never touches a database, so the same code drives in-process SQLite, a runtime-loaded `libsqlite3`, Cloudflare D1 and a JavaScript host.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-core) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

Most applications should depend on [`oxilite`](https://crates.io/crates/oxilite), which wraps this crate in an Oxigraph-compatible `Store`. Use `oxilite-core` directly to **add a new SQLite backend** or to drive oxilite from another runtime.

## Sans-IO jobs

Every operation (open, load, query, update, optimize, materialize) is a [`Job`](https://docs.rs/oxilite-core/latest/oxilite_core/job/trait.Job.html): a step machine that yields SQL [`Request`](https://docs.rs/oxilite-core/latest/oxilite_core/sql/struct.Request.html)s and consumes [`Response`](https://docs.rs/oxilite-core/latest/oxilite_core/sql/struct.Response.html)s. A backend only has to "run these statements, atomically if asked":

```rust
use oxilite_core::{Capabilities, Request, Response, Result, SyncBackend};

struct MyBackend { /* a connection */ }

impl SyncBackend for MyBackend {
    fn execute(&self, request: &Request) -> Result<Response> {
        // run request.statements; in one transaction when request.mode is Mode::Atomic
        todo!()
    }
    fn capabilities(&self) -> &Capabilities {
        todo!()   // Capabilities::native(), Capabilities::d1(), or tuned limits
    }
}
```

`run_sync` and `run_async` drive any job over a `SyncBackend` or an `AsyncBackend`.

## What is inside

| Module | Role |
|---|---|
| `encoding` | Terms as tagged 64-bit ids (xxh3); canonical integers and booleans inline, ordered by value |
| `schema` | The `quads` / `terms` / `triple_terms` schema with covering `WITHOUT ROWID` indexes, and `StoreOptions` |
| `compiler`, `query` | spargebra algebra → one SQL `SELECT`, with static types, property paths as recursive CTEs, and `QueryOptions` |
| `stats` | Per-predicate and per-class statistics feeding a greedy planner, enforced with `CROSS JOIN` |
| `update`, `writer` | Atomic SPARQL Update: evaluate-then-apply inside one batch, chunked bulk loads |
| `reason` | RDFS / OWL QL rewriting and OWL 2 RL rules as SQL |
| `fallback` | Evaluation with spareval for the rare queries SQL cannot express (sync backends) |
| `text` | FTS5 full-text search (`oxl:textMatch`) |
| `json` (feature `serde`) | JSON forms of requests, responses and results for hosts such as the wasm engine |

Everything is designed around SQLite's planner and the costs of a remote SQLite; the reasons for each choice are in the [design decisions](https://github.com/Volland/oxilite/blob/main/lat.md/decisions.md) and [How it works](https://github.com/Volland/oxilite#how-it-works).

## Features

- `serde`: JSON (de)serialization of requests, responses, options and results.

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
| [`oxilite-jsonld`](https://crates.io/crates/oxilite-jsonld) | JSON-LD documents stored verbatim, one named graph each |
| [`oxilite-vc`](https://crates.io/crates/oxilite-vc) | Verifiable Credentials: stored under their id, indexed, queryable |
| [`oxilite-reason`](https://crates.io/crates/oxilite-reason) | OWL 2 RL materialization with `reasonable` |
| [`oxilite-validate`](https://crates.io/crates/oxilite-validate) | SHACL and ShEx validation with rudof |
| [`oxilite-cli`](https://crates.io/crates/oxilite-cli) | The `oxilite` command and a SPARQL endpoint like `oxigraph serve` |
| [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node) | Node.js bindings, API of Oxigraph's JS package |
| [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1) | Cloudflare D1 and Durable Objects from TypeScript (WebAssembly core) |
| [`@oxilite/common`](https://www.npmjs.com/package/@oxilite/common) | RDF/JS terms and shared TypeScript types |

## License

Dual-licensed under [MIT](https://github.com/Volland/oxilite/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/Volland/oxilite/blob/main/LICENSE-APACHE), at your option, like Oxigraph.
