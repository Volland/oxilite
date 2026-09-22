<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-rusqlite

[![crates.io](https://img.shields.io/crates/v/oxilite-rusqlite.svg)](https://crates.io/crates/oxilite-rusqlite) [![docs.rs](https://img.shields.io/docsrs/oxilite-rusqlite)](https://docs.rs/oxilite-rusqlite) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**In-process SQLite backend for oxilite**, built on rusqlite with a bundled SQLite. It is the default backend of the [`oxilite`](https://crates.io/crates/oxilite) store.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-rusqlite) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

It also registers oxilite's SQL functions (SPARQL regular expressions, `REPLACE`, hashes, `ENCODE_FOR_URI`) and Unicode-aware `upper` / `lower`, so **every SPARQL function compiles to SQL** on this backend.

## Usage

You normally get it through `oxilite` (feature `rusqlite`, on by default):

```rust
let store = oxilite::store::Store::open("data.sqlite")?;   // RusqliteBackend underneath
```

To build the backend yourself, for example over an existing `rusqlite::Connection`:

```rust
use oxilite::store::Store;
use oxilite_rusqlite::RusqliteBackend;

let backend = RusqliteBackend::open("data.sqlite")?;        // or ::memory(), ::open_read_only(path)
// let backend = RusqliteBackend::from_connection(conn)?;   // share your own connection
let store = Store::with_backend(backend)?;
```

Atomic requests run in one transaction. The connection uses WAL mode for file databases.

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
