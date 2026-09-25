<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite

[![crates.io](https://img.shields.io/crates/v/oxilite.svg)](https://crates.io/crates/oxilite) [![docs.rs](https://img.shields.io/docsrs/oxilite)](https://docs.rs/oxilite) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**An Oxigraph-compatible RDF database and SPARQL 1.1 engine that uses SQLite as its storage engine.** Same data model, same SPARQL semantics and the same Rust API as Oxigraph, on plain SQL. It runs in-process, on a SQLite library you load at runtime, and on Cloudflare D1.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

## Why oxilite

[Oxigraph](https://github.com/oxigraph/oxigraph) stores its data in RocksDB, which needs native code and a local filesystem. oxilite keeps Oxigraph's API and swaps the storage for SQLite, so a knowledge graph can live where SQLite already lives:

- **Edge databases** such as Cloudflare D1, where you can send SQL but cannot install extensions or native libraries.
- **Hosts that ship their own SQLite**: mobile apps, embedded devices, sandboxes, SQLCipher builds.
- **Apps that already use SQLite** and want their graph in the same file, backed up with the same tools.

It is fast because it is built around how SQLite runs queries: terms are hashed to 64-bit ids in Rust (writes never read back), small integers live inside the id, every triple pattern is an index-only scan, a whole SPARQL query compiles to **one SQL statement**, and oxilite's own statistics-driven planner fixes the join order. On the BSBM business-intelligence mix it is faster than Oxigraph. See [Performance](https://github.com/Volland/oxilite#performance).

## Install

```toml
[dependencies]
oxilite = "0.2"                     # bundled SQLite (feature `rusqlite`, on by default)
```

| Feature | Adds |
|---|---|
| `rusqlite` (default) | `Store` on a bundled, in-process SQLite |
| `dylib` | `Store::open_with_library`: any SQLite ≥ 3.37 loaded from a path at runtime |
| `d1` | `oxilite::d1::D1Backend` for `AsyncStore` in Rust Cloudflare Workers |
| `cypher` | `store.cypher(…)`: openCypher over the same data |
| `reasonable` | `materialize_with_reasonable()`: in-memory OWL 2 RL with `reasonable` |

## Quick start

```rust
use oxilite::io::RdfFormat;
use oxilite::sparql::QueryResults;
use oxilite::store::Store;          // was: use oxigraph::store::Store;

let store = Store::open("data.sqlite")?;              // or Store::new() in memory
store.load_from_reader(RdfFormat::Turtle, r#"
    @prefix ex: <http://example.com/> .
    ex:ada a ex:Person ; ex:name "Ada" ; ex:age 36 .
"#.as_bytes())?;

if let QueryResults::Solutions(solutions) = store.query(
    "SELECT ?name WHERE { ?p a <http://example.com/Person> ; <http://example.com/name> ?name }",
)? {
    for s in solutions {
        println!("{}", s?.get("name").unwrap());
    }
}

store.update("INSERT DATA { <http://example.com/ada> <http://example.com/knows> <http://example.com/alan> }")?;
store.optimize()?;                                     // refresh planner statistics after big loads
println!("{}", store.explain("SELECT * WHERE { ?s ?p ?o } LIMIT 1")?);   // the SQL and join order
```

The modules mirror Oxigraph (`model`, `io`, `sparql`, `store`), so porting is usually a change of crate name. Oxigraph's own store tests and W3C manifest runner pass against oxilite; the few intentional differences are listed under [Compatibility](https://github.com/Volland/oxilite#compatibility-with-oxigraph).

## Your own SQLite library

```rust
// features = ["dylib"]
let store = oxilite::store::Store::open_with_library("/usr/lib/libsqlite3.so", "data.sqlite")?;
```

Only the stable C API is bound, so the system SQLite, a vendor build or SQLCipher all work.

## Cloudflare D1 (Rust Workers)

```rust
// oxilite = { version = "0.2", default-features = false, features = ["d1"] }
use oxilite::{d1::D1Backend, AsyncStore};

let store = AsyncStore::open_existing(D1Backend::new(env.d1("DB")?)).await?;
let out = store.query_output("SELECT * WHERE { ?s ?p ?o } LIMIT 10", &Default::default()).await?;
```

Every query is one D1 call and every update one atomic `batch()`. How oxilite fits D1's limits (no UDFs, statement and batch sizes, per-row billing) is in the [D1 guide](https://github.com/Volland/oxilite#using-oxilite-with-cloudflare-d1). From TypeScript, use [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1).

## Reasoning

```rust
use oxilite::sparql::{QueryOptions, Reasoning};

// ex:Dog rdfs:subClassOf ex:Animal . ex:rex a ex:Dog .
let opts = QueryOptions { reasoning: Reasoning::Rdfs, ..Default::default() };
let out = store.query_output("SELECT ?x WHERE { ?x a <http://example.com/Animal> }", &opts)?;   // ex:rex

store.materialize()?;       // OWL 2 RL closure, SQL rules (D1 too); read with `include_inferred: true`
```

RDFS and OWL QL entailment are query rewrites against a small schema closure, so queries stay single statements and nothing extra is written.

## Cypher

```rust
// features = ["cypher"]
use oxilite::cypher::{CypherOptions, Params, Vocabulary};

let opts = CypherOptions { vocabulary: Vocabulary::new("http://example.com/"), ..Default::default() };
store.cypher_with("CREATE (:Person {name: 'Ada'})-[:KNOWS {since: 2020}]->(:Person {name: 'Alan'})", &Params::new(), &opts)?;
let r = store.cypher_with("MATCH (a:Person)-[k:KNOWS]->(b) RETURN b.name, k.since", &Params::new(), &opts)?;
```

Property graphs are a view of the RDF data, so SPARQL and Cypher see the same graph. See [`oxilite-cypher`](https://crates.io/crates/oxilite-cypher).

## JSON-LD documents and Verifiable Credentials

```rust
// features = ["vc"]  (or ["jsonld"] for JSON-LD without the credentials profile)
let vcs = store.credentials()?;
let id = vcs.put_credential(credential_json)?;          // checked, stored verbatim, RDF in graph <id>
let raw = vcs.get_credential(&id)?.unwrap().json;       // the exact bytes
let docs = store.jsonld()?;                              // any JSON-LD document
docs.put_document(r#"{"@context": {"name": "http://schema.org/name"}, "@id": "urn:x", "name": "x"}"#)?;
```

Each document is stored byte for byte, and its RDF goes into a named graph of its own (by default its `id`), which SPARQL queries. Keys, graphs, contexts and metadata indexes are configurable. See [`oxilite-jsonld`](https://crates.io/crates/oxilite-jsonld) and [`oxilite-vc`](https://crates.io/crates/oxilite-vc).

## Versioning and time travel

Create a store with `StoreOptions { versioning: Versioning::Log, .. }` and every write becomes a commit in an immutable change log. `QueryOptions::as_of` (`"HEAD~1"`, `"#42"`, `"@2026-09-01T12:00:00Z"`) queries any past version, and `SERVICE <oxilite:version/HEAD~1> { … }` compares two versions in one query. `GRAPH <oxilite:history>` reads commits (PROV-O) and changes (`oxl:added` / `oxl:removed`) as RDF. Datalog adds `at "HEAD~1"` / `at ?c` per atom and `commit`/`added`/`removed`/`branch` relations, and Cypher takes the same `as_of`. `history`, `changes`, `diff`, `with_commit` and `purge` read and manage the log, and `set_versioning` raises or lowers an existing store's level. `Versioning::Stamped` keeps only a store clock and the tick that added each quad, and writes no extra row per quad. See the [README](https://github.com/Volland/oxilite#versioning-history-and-time-travel).

## Full-text search

Create the store with `StoreOptions { text_index: true, .. }` and match literals with FTS5:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
SELECT ?doc WHERE { ?doc rdfs:label ?label FILTER(oxl:textMatch(?label, "graph data*")) }
```

## Learn more

- [How it works](https://github.com/Volland/oxilite#how-it-works): the sans-IO core, the storage schema and what a query compiles to
- [Performance](https://github.com/Volland/oxilite#performance): BSBM results against Oxigraph and D1 write costs
- [Reasoning and validation](https://github.com/Volland/oxilite#reasoning-and-validation), [Cypher and property graphs](https://github.com/Volland/oxilite#cypher-and-property-graphs), [JSON-LD and Verifiable Credentials](https://github.com/Volland/oxilite#json-ld-and-verifiable-credentials)
- [Design decisions](https://github.com/Volland/oxilite/blob/main/lat.md/decisions.md) and [architecture](https://github.com/Volland/oxilite/blob/main/lat.md/architecture.md)

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
