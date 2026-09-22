<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-reason

[![crates.io](https://img.shields.io/crates/v/oxilite-reason.svg)](https://crates.io/crates/oxilite-reason) [![docs.rs](https://img.shields.io/docsrs/oxilite-reason)](https://docs.rs/oxilite-reason) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**OWL 2 RL materialization for oxilite with the [`reasonable`](https://github.com/gtfierro/reasonable) reasoner**, for native backends.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-reason) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

oxilite already reasons in two ways without this crate: per-query RDFS / OWL QL rewriting, and OWL 2 RL materialization as SQL rules that also run on D1. `oxilite-reason` is the fast path for large native stores: it reads every asserted triple, runs `reasonable`'s Datalog engine in memory and writes the new conclusions into `quads_inf`, exactly where the SQL rules put theirs. Both give identical results.

## Usage

Through `oxilite` (feature `reasonable`):

```rust
// oxilite = { version = "0.2", features = ["reasonable"] }
use oxilite::sparql::QueryOptions;

let added = store.materialize_with_reasonable()?;     // store.materialize() uses the SQL rules
let opts = QueryOptions { include_inferred: true, ..Default::default() };
let out = store.query_output("SELECT ?x WHERE { ?x a <http://example.com/Animal> }", &opts)?;
```

Or over any `SyncBackend`: `oxilite_reason::materialize(&backend)` stores the closure, `oxilite_reason::infer(&backend)` only returns it.

Materialized inferences are not maintained: re-run it after changing data, or clear them with `clear_inferences()`. The whole dataset must fit in memory, so D1 stores use the SQL rules instead. See [Reasoning and validation](https://github.com/Volland/oxilite#reasoning-and-validation).

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
