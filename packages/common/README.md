<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# @oxilite/common

[![npm](https://img.shields.io/npm/v/@oxilite/common.svg)](https://www.npmjs.com/package/@oxilite/common) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**RDF/JS terms and the shared TypeScript types of oxilite**, used by [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node) and [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1).

**[Website](https://oxilitedb.com)** · [npm](https://www.npmjs.com/package/@oxilite/common) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

You rarely install it directly: both packages re-export everything here.

```ts
import { namedNode, literal, quad, DataFactory, type Term } from "@oxilite/node";   // or "@oxilite/d1"

const q = quad(namedNode("http://example.com/ada"), namedNode("http://example.com/name"), literal("Ada", "en"));
```

## What is inside

- **RDF/JS terms:** `NamedNode`, `BlankNode`, `Literal` (with language and direction), `DefaultGraph`, `Variable` and `Quad` (also usable as an RDF 1.2 triple term), plus `DataFactory` and its shortcuts `namedNode`, `blankNode`, `literal`, `defaultGraph`, `variable`, `quad`, `triple`. They follow the [RDF/JS data model](https://rdf.js.org/data-model-spec/), so they mix with other RDF/JS libraries.
- **SPARQL types:** `QueryOptions` (with oxilite's `reasoning` and `include_inferred`), `LoadOptions`, `DumpOptions`, `QueryResult`.
- **Cypher types:** `CypherValue`, `CypherNode`, `CypherRelationship`, `CypherPath`, `CypherTemporal`, `CypherResult`, `CypherStats` and `CypherOptions` (`base`, `prefixes`, `names`, `multiValue`, `reasoning`, `shapes`…).
- **JSON-LD and credential types:** `JsonLdOptions`, `CredentialOptions`, `KeyStrategy`, `GraphStrategy`, `StoredDocument`, `DocumentFilter`, `Drift`, `PresentationKeys` and the `JsonLdError` class (with its JSON-LD error `code`).
- **JSON helpers:** `toJson` / `fromJson` convert terms to and from the JSON form the oxilite core exchanges.

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
