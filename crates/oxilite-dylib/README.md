<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-dylib

[![crates.io](https://img.shields.io/crates/v/oxilite-dylib.svg)](https://crates.io/crates/oxilite-dylib) [![docs.rs](https://img.shields.io/docsrs/oxilite-dylib)](https://docs.rs/oxilite-dylib) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**An oxilite backend that loads a SQLite shared library from a path at runtime.** No SQLite is linked at build time: use the system `libsqlite3`, a vendor-provided build, or SQLCipher.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-dylib) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

Only the stable SQLite C API is resolved (`open_v2`, `prepare_v2`, `step`, `column_*`, `finalize`, `errmsg`, `changes`, `exec`, and optionally `create_function_v2`), so **any SQLite ≥ 3.37** works. When the library allows user-defined functions, oxilite registers its own; otherwise it compiles to plain SQL (Shallow SQL), as on D1.

## Usage

Through `oxilite` (feature `dylib`):

```rust
let store = oxilite::store::Store::open_with_library("/usr/lib/x86_64-linux-gnu/libsqlite3.so.0", "data.sqlite")?;
```

Or directly:

```rust
use oxilite::store::Store;
use oxilite_dylib::{find_system_library, DylibBackend};

let library = find_system_library().expect("no system libsqlite3");
let store = Store::with_backend(DylibBackend::open(library, "data.sqlite")?)?;
```

`SqliteLibrary::load(path)` loads a library once and reports its `version_number()`. The same database file can be opened with the bundled SQLite ([`oxilite-rusqlite`](https://crates.io/crates/oxilite-rusqlite)) and with a runtime library, for example by `oxilite serve --library`.

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
