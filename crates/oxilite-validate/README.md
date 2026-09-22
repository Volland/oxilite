<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-validate

[![crates.io](https://img.shields.io/crates/v/oxilite-validate.svg)](https://crates.io/crates/oxilite-validate) [![docs.rs](https://img.shields.io/docsrs/oxilite-validate)](https://docs.rs/oxilite-validate) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**SHACL and ShEx validation of oxilite stores with [rudof](https://rudof-project.github.io/).** rudof's validators run unchanged over your SQLite or D1 data.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-validate) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

`StoreGraph` implements rudof's RDF traits over a blocking oxilite `Store`, so neighbourhood lookups become SQL index scans and rudof's SPARQL-mode validation runs through the oxilite compiler. On the W3C SHACL core suite and the shexTest suite, results are identical to rudof's in-memory graph.

## Usage

```rust
use oxilite_validate::{validate_shacl, validate_shex, ShaclValidationMode};

let shapes = r#"
    @prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.com/> .
    ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
        sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:datatype <http://www.w3.org/2001/XMLSchema#string> ] .
"#;
let report = validate_shacl(&store, shapes, &ShaclValidationMode::Native)?;
if !report.conforms() {
    println!("{report}");
}

let results = validate_shex(&store, shexc, "http://example.com/",
    "<http://example.com/alice>@<http://example.com/Person>")?;
```

### Cloudflare D1

rudof's validators do not build for wasm32, so D1 is validated from native code (a CLI, a server, CI) over the D1 HTTP API. `prefetch` loads the subgraph the shapes need in a few batched requests, with a size limit that fails loudly, then validates it in memory:

```rust
let report = oxilite_validate::prefetch::validate_shacl_async(
    &d1_store, shapes, &ShaclValidationMode::Native, &Default::default(),
).await?;
```

Shapes stored in the dataset are also the property-graph schema for [`oxilite-cypher`](https://crates.io/crates/oxilite-cypher), which checks simple constraints before every Cypher write. See [Reasoning and validation](https://github.com/Volland/oxilite#reasoning-and-validation).

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
