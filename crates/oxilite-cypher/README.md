<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-cypher

[![crates.io](https://img.shields.io/crates/v/oxilite-cypher.svg)](https://crates.io/crates/oxilite-cypher) [![docs.rs](https://img.shields.io/docsrs/oxilite-cypher)](https://docs.rs/oxilite-cypher) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**openCypher over oxilite.** Query and update your RDF data as a property graph: nodes, labels, properties and relationships, with the same store, planner, reasoning and validation as SPARQL. It runs natively and on Cloudflare D1.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-cypher) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

## The mapping

A property graph here is a **view of the RDF 1.2 dataset**, not a second copy:

| Property graph | RDF |
|---|---|
| node | IRI or blank node |
| label `:Person` | `?n rdf:type ex:Person` |
| property `n.name` | literal triple `?n ex:name "…"` (several values read as a list) |
| relationship `(a)-[:KNOWS]->(b)` | triple `?a ex:KNOWS ?b` |
| relationship properties, parallel relationships | an RDF 1.2 reifier: `?r rdf:reifies <<( ?a ex:KNOWS ?b )>> ; ex:since 2020`, created only when needed |

Data written with Cypher is plain RDF for SPARQL, and Turtle you load is a graph for Cypher. Names map to IRIs through a `Vocabulary`: a base namespace, prefixes (`` :`schema:Person` ``), explicit overrides, or absolute IRIs in backticks.

## Usage

Enable the `cypher` feature of [`oxilite`](https://crates.io/crates/oxilite):

```toml
oxilite = { version = "0.2", features = ["cypher"] }
```

```rust
use oxilite::cypher::{CypherOptions, Params, Value, Vocabulary};
use oxilite::store::Store;

let store = Store::new()?;
let opts = CypherOptions { vocabulary: Vocabulary::new("http://example.com/"), ..Default::default() };

store.cypher_with(
    "CREATE (:Person {name: 'Ada'})-[:KNOWS {since: 2020}]->(:Person {name: 'Alan'})",
    &Params::new(), &opts,
)?;

let mut params = Params::new();
params.insert("name".into(), Value::String("Ada".into()));
let r = store.cypher_with(
    "MATCH (a:Person {name: $name})-[k:KNOWS]->(b) RETURN b.name AS friend, k.since AS since",
    &params, &opts,
)?;
for row in r.records() {
    println!("{} since {}", row["friend"], row["since"]);
}
println!("{}", store.explain_cypher("MATCH (n:Person) RETURN n", &Params::new(), &opts)?);
```

`AsyncStore` has the same `cypher` / `cypher_with` / `explain_cypher` methods for D1. From JavaScript, call `store.cypher(query, params)` in [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node) and [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1).

## What is supported

- **Reading:** `MATCH`, `OPTIONAL MATCH`, `WHERE`, `WITH`, `RETURN`, `UNWIND`, `UNION`, aggregates, `ORDER BY` / `SKIP` / `LIMIT`, `EXISTS { }`, pattern predicates and pattern comprehensions, list comprehensions, `reduce`, `CASE`, lists and maps.
- **Paths:** variable-length relationships with Cypher's relationship uniqueness, path values, `shortestPath` and `allShortestPaths`.
- **Writing:** `CREATE`, `MERGE` (with `ON CREATE` / `ON MATCH`), `SET`, `REMOVE`, `DELETE`, `DETACH DELETE`, applied as **one atomic request** (one D1 batch).
- **Temporal types:** `date`, `time`, `localtime`, `datetime`, `localdatetime`, `duration` and their functions, stored as XSD literals.
- **Coverage:** 3733 of the 3880 [openCypher TCK](https://github.com/opencypher/openCypher/tree/main/tck) scenarios pass (read-only 96.4%). The rest are listed with reasons in [`tck-allowlist.txt`](https://github.com/Volland/oxilite/blob/main/crates/oxilite-cypher/tck-allowlist.txt).

## OWL- and SHACL-aware

- **OWL:** with `reasoning: Rdfs` or `OwlQl` in `CypherOptions::query`, `:Person` matches subclasses, and relationship types match their subproperties and inverses.
- **SHACL as the graph schema:** shapes stored in the dataset (`Store::cypher_schema`) make `sh:minCount 1` properties inner joins, `sh:maxCount 1` properties scalars, and `sh:datatype` a static type for the SQL compiler. Writes that break `sh:datatype`, cardinality, `sh:in` or `sh:pattern` are rejected before anything is written.
- **Introspection:** `CALL db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()` and `db.schema.nodeTypeProperties()`.

## How it runs

A statement is parsed, validated and lowered to SPARQL algebra, which oxilite compiles to one SQL statement. What SQL cannot express (writes, lists, maps, `collect()`, temporal arithmetic) runs in Rust over the rows. `shortestPath` is a breadth-first search, one SQL request per level. The whole statement is a sans-IO `CypherJob`, so the same code runs on every backend: hosts step it with `CypherJob::step` and answer `CypherStep::Query` / `Sql` / `Write` until `Done`.

## Features

- `tzdb-bundle` (default): embed the IANA time zone database, needed for named zones on WebAssembly. Without it, native builds read `/usr/share/zoneinfo`.

More in [Cypher and property graphs](https://github.com/Volland/oxilite#cypher-and-property-graphs) and the article on [oxilitedb.com](https://oxilitedb.com).

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
