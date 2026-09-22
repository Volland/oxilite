<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-jsonld

[![crates.io](https://img.shields.io/crates/v/oxilite-jsonld.svg)](https://crates.io/crates/oxilite-jsonld) [![docs.rs](https://img.shields.io/docsrs/oxilite-jsonld)](https://docs.rs/oxilite-jsonld) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**JSON-LD documents in oxilite.** Store a JSON-LD document and get two things:
- the exact bytes back whenever you ask for them;
- its RDF in a named graph of its own, which you query with SPARQL like any other data.

It runs natively and on Cloudflare D1, and every write is one atomic request.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-jsonld) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

## Usage

Enable the `jsonld` feature of [`oxilite`](https://crates.io/crates/oxilite):

```toml
oxilite = { version = "0.2", features = ["jsonld"] }
```

```rust
use oxilite::sparql::QueryResults;
use oxilite::store::Store;

let store = Store::new()?;
let docs = store.jsonld()?;

let key = docs.put_document(r#"{
  "@context": {"name": "http://schema.org/name", "knows": {"@id": "http://schema.org/knows", "@type": "@id"}},
  "@id": "https://example.org/people/ada",
  "name": "Ada Lovelace",
  "knows": "https://example.org/people/charles"
}"#)?;

// The RDF is in the graph named after the key…
let r = store.query("SELECT ?who WHERE { GRAPH <https://example.org/people/ada> { ?s <http://schema.org/knows> ?who } }")?;
// …and the JSON comes back byte for byte.
let doc = docs.get_document(&key)?.unwrap();
println!("{} ({} bytes, sha256 {})", doc.key, doc.json.len(), doc.sha256);

// Replace (same key) and remove are atomic and clear exactly the document's triples.
docs.remove_document(&key)?;
# Result::<_, Box<dyn std::error::Error>>::Ok(())
```

`AsyncStore::jsonld()` has the same methods for D1. From JavaScript, call `store.jsonld()` in [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node) or [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1).

## Keys and graphs

| Option | Values | Default |
|---|---|---|
| `key` | `Id` (top-level `@id`/`id`), `Pointer("/credentialSubject/id")`, `ContentHash`, `Explicit` | `Id` |
| `on_missing_key` | `Reject`, `ContentHash` (`urn:oxilite:doc:sha256:<hex>`) | `Reject` |
| `graph` | `Key` (the key is the graph IRI), `Template("https://ex.org/g/{key}")`, `Fixed(iri)`, `DefaultGraph` | `Key` |

```rust
use oxilite::jsonld::{GraphStrategy, JsonLdOptions, KeyStrategy};

let docs = store.jsonld_with(JsonLdOptions {
    key: KeyStrategy::Pointer("/credentialSubject/id".into()),
    graph: GraphStrategy::Template("https://example.org/holders/{key}".into()),
    ..Default::default()
})?;
```

With `Key` or `Template`, each document owns its graph: replace and remove delete the graph without reading it first. With `Fixed` or `DefaultGraph`, documents share a graph. The previous version is then read back and exactly its triples are deleted.

## Contexts: offline by default

Remote `@context` IRIs are resolved **without network access**, in this order:

1. contexts registered on the handle: `JsonLdOptions::with_context(iri, json)`;
2. contexts persisted in the store with `put_context(iri, json)`. These work from every process and on D1.

An unknown context fails with `JsonLdError::ContextNotFound(iri)`, and nothing is written. To download unknown contexts, enable the `network` feature (native only). Then set `fetcher: Some(oxilite_jsonld::http_fetcher())`, and set `cache_fetched: true` to persist what was downloaded.

## What you get

- **Verbatim storage:** the raw JSON, its SHA-256, the target graph and the time it was stored, in the `jsonld_documents` table.
- **Standard conversion:** the [`json-ld`](https://crates.io/crates/json-ld) crate implements the JSON-LD 1.1 processing algorithms. 450 tests of the W3C `toRdf` suite pass; the 4 allow-listed exceptions are limitations of `json-ld` 0.21.
- **Graphs the document defines:** the graphs a document defines, such as `@graph` containers and proofs, belong to it. Replacing or removing the document removes them too.
- **Isolated blank nodes:** blank-node labels are derived from the document's key. Two documents never share a blank node, and re-storing a document changes nothing.
- **Atomic writes:** one request, which is one D1 batch. Long documents are written in chunks inside that batch. A document that cannot fit in one batch fails with `DocumentTooLarge` rather than being split.
- **Metadata lookups:** `find_documents(&DocumentFilter { issuer, subject, type_, valid_at, profile, after, limit })` reads indexed columns, not SPARQL. [`oxilite-vc`](https://crates.io/crates/oxilite-vc) fills them for credentials.
- **Drift repair:** SPARQL UPDATE can edit document graphs. `check_documents()` reports documents whose graphs no longer match their JSON, and `rebuild_graph(key)` regenerates them from the stored JSON, which is the source of truth.
- **Opt-in tables:** the three tables are created when you first open a handle. For D1 migrations, use `oxilite_jsonld::schema::schema_sql`.

## Features

- `network`: download unknown contexts over HTTP (native only; off by default).
- `json`: JSON forms of options, filters and documents, as used by the JavaScript bindings.

## The oxilite family

oxilite is an Oxigraph-compatible RDF database and SPARQL 1.1 engine that stores its data in SQLite, so it runs anywhere SQLite runs: in-process, on a system or vendor `libsqlite3`, on Cloudflare D1 and in Durable Objects. The same data can be queried with SPARQL and openCypher, reasoned over with RDFS / OWL, validated with SHACL and ShEx, and loaded from JSON-LD documents and Verifiable Credentials. Read the overview on **[oxilitedb.com](https://oxilitedb.com)** and the full guide in the [main README](https://github.com/Volland/oxilite#readme).

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
