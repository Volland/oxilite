<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# @oxilite/node

[![npm](https://img.shields.io/npm/v/@oxilite/node.svg)](https://www.npmjs.com/package/@oxilite/node) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**An Oxigraph-compatible SPARQL 1.1 store for Node.js, on SQLite.** The API of Oxigraph's JavaScript package, with RDF/JS terms, on a single SQLite file. Also query the same data with **openCypher**, reason with RDFS / OWL, and search text with FTS5.

**[Website](https://oxilitedb.com)** · [npm](https://www.npmjs.com/package/@oxilite/node) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

```bash
npm install @oxilite/node
```

## Quick start

```ts
import { Store, namedNode, literal, quad, type Term } from "@oxilite/node";

const store = new Store("data.sqlite");               // new Store() for an in-memory store
store.load(`@prefix ex: <http://example.com/> .
  ex:ada a ex:Person ; ex:name "Ada" ; ex:knows ex:alan .`, { format: "text/turtle" });

store.add(quad(namedNode("http://example.com/alan"), namedNode("http://example.com/name"), literal("Alan")));

for (const row of store.query(
  "SELECT ?name WHERE { ?p <http://example.com/name> ?name }",
) as Map<string, Term>[]) {
  console.log(row.get("name")?.value);
}

store.update("DELETE WHERE { ?s <http://example.com/knows> ?o }");
console.log(store.size);
```

`query` returns what Oxigraph returns: an array of `Map`s for `SELECT`, a boolean for `ASK`, quads for `CONSTRUCT` / `DESCRIBE`, or a string when you pass `results_format`.

## Cypher over the same data

```ts
const opts = { base: "http://example.com/" };
store.cypher("CREATE (:Person {name: 'Grace'})-[:KNOWS {since: 1950}]->(:Person {name: 'Ada'})", {}, opts);

const r = store.cypher(
  "MATCH (a:Person {name: $name})-[:KNOWS]->(b) RETURN b.name AS friend",
  { name: "Grace" }, opts,
);
console.log(r.records);                    // [{ friend: "Ada" }]
console.log(store.explainCypher("MATCH (n:Person) RETURN n", {}, opts));
```

Nodes are IRIs, labels are `rdf:type`, properties are literal triples, relationships are triples (with an RDF 1.2 reifier for their properties), so SPARQL sees everything Cypher writes. With `reasoning: "rdfs"`, labels follow class hierarchies; SHACL shapes in the store check every write.

## API

| Method | Does |
|---|---|
| `new Store(path? \| quads? \| options?)` | Open a SQLite file, an in-memory store, or one filled with quads. Options: `path`, `library` (a `libsqlite3` to load), `graphIndex`, `textIndex` |
| `query(sparql, options?)` | SPARQL 1.1 query. Options as in Oxigraph, plus `reasoning: "rdfs" \| "owl-ql"` and `include_inferred` |
| `update(sparql)` | SPARQL 1.1 Update, atomically |
| `load(data, options)` / `bulkLoad(data, options)` | Parse Turtle, N-Triples, N-Quads, TriG, RDF/XML, JSON-LD… |
| `dump(options)` | Serialize the store or one graph |
| `add`, `addAll`, `delete`, `has`, `match`, `size` | Quad-level access with RDF/JS terms |
| `cypher(query, params?, options?)` | openCypher read or write; returns `{ columns, rows, records, stats }` |
| `explain(sparql)`, `explainUpdate`, `explainCypher` | The SQL a statement compiles to, with the planner's notes |
| `materialize({ engine? })`, `clearInferences()` | OWL 2 RL closure (`"sql"` or `"reasonable"`) |
| `optimize()`, `backup(path)`, `clear()` | Refresh planner statistics, `VACUUM INTO` a copy, empty the store |

Oxigraph's own `store.test.ts` runs unchanged against this package.

## Platforms

Version 0.2 ships a prebuilt native binary for **macOS on Apple silicon** (`darwin-arm64`). On other platforms, build it from a checkout of the [repository](https://github.com/Volland/oxilite) with `npm run build:native -w @oxilite/node` (needs a Rust toolchain). For Cloudflare Workers, use [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1), which needs no native code.

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
