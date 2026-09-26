<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# @oxilite/d1

[![npm](https://img.shields.io/npm/v/@oxilite/d1.svg)](https://www.npmjs.com/package/@oxilite/d1) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**An Oxigraph-compatible SPARQL 1.1 and openCypher store on Cloudflare D1 and Durable Objects.** The oxilite core runs as WebAssembly: it compiles each query to SQL for your `env.DB` binding, so a query is one D1 call and an update is one atomic batch. No Rust toolchain needed.

**[Website](https://oxilitedb.com)** · [npm](https://www.npmjs.com/package/@oxilite/d1) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

```bash
npm install @oxilite/d1
```

## 1. Create the schema

```bash
npx wrangler d1 create my-graph
npx oxilite-d1 schema > migrations/0001_oxilite.sql          # --no-graph-index to save writes
                                                             # --jsonld for JSON-LD / credential tables
npx wrangler d1 migrations apply my-graph --remote
```

> **Upgrading to 0.3.0 from 0.2.x:** 0.3.0 adds the `schema_graphs`, `shapes_index`,
> `shapes_in` and `datalog_work` tables. A D1 store is opened with `open_existing`, so no
> DDL runs per request and an existing database will report `no such table: schema_graphs`
> until you apply a new migration. Regenerate and apply it:
>
> ```bash
> npx wrangler d1 migrations create my-graph oxilite-0-3-0
> npx oxilite-d1 schema > migrations/0002_oxilite-0-3-0.sql
> npx wrangler d1 migrations apply my-graph --remote
> ```
>
> Every statement is `CREATE TABLE IF NOT EXISTS`, so re-applying it is safe and your data
> is untouched.

## 2. Query it from a Worker

```ts
import { D1Store } from "@oxilite/d1";
import wasm from "@oxilite/d1/oxilite.wasm";

export default {
  async fetch(req: Request, env: { DB: D1Database }): Promise<Response> {
    const store = await D1Store.open(env.DB, { wasm, migrated: true });
    const url = new URL(req.url);

    if (req.method === "POST" && url.pathname === "/update") {
      await store.update(await req.text());               // one atomic D1 batch
      return new Response(null, { status: 204 });
    }
    const q = url.searchParams.get("query") ?? "SELECT * WHERE { ?s ?p ?o } LIMIT 10";
    return new Response(await store.queryJson(q), {
      headers: { "content-type": "application/sparql-results+json" },
    });
  },
};
```

## Cypher on D1

```ts
await store.cypher(
  "UNWIND $rows AS row MERGE (p:Person {id: row.id}) SET p.name = row.name",
  { rows: [{ id: 1, name: "Ada" }, { id: 2, name: "Alan" }] },
  { base: "https://example.com/" },
);
const r = await store.cypher(
  "MATCH p = shortestPath((a:Person {id: 1})-[:KNOWS*]-(b:Person {id: 2})) RETURN length(p) AS hops",
  {}, { base: "https://example.com/" },
);
```

Every Cypher write is one atomic batch; `shortestPath` is a breadth-first search with one request per level. SPARQL and Cypher see the same data.

## JSON-LD documents and Verifiable Credentials on D1

```ts
const vcs = store.credentials();                     // W3C credential contexts are bundled
const id = await vcs.put(credentialJson);            // one D1 batch: JSON row + RDF in graph <id>
await vcs.putPresentation(presentationJson);         // embedded credentials stored too, same batch
const valid = await vcs.find({ issuer: "did:example:issuer", validAt: new Date() });
const raw = (await vcs.get(id))?.json;               // the exact bytes

const docs = store.jsonld();                         // any JSON-LD document
await docs.putContext("https://example.org/my-context", myContext);   // persisted in D1
await docs.put(documentJson);
```

Contexts load offline: bundled credential contexts, contexts passed in `options.contexts`, and contexts persisted in D1 with `putContext`. A Worker never fetches contexts. Every write is one batch, and a document too large for one batch fails with `document-too-large` rather than being split. Pass `migrated: true` to `jsonld()` / `credentials()` when your migration already created the tables (`oxilite-d1 schema --jsonld`). JSON-LD and credential support adds about 1.7 MB (0.4 MB gzipped) to the wasm module, which stays well within the Workers size limits.

## Durable Objects

The same store runs on a Durable Object's embedded SQLite through a small adapter over `ctx.storage.sql`, which gives every agent or user a private graph. [`examples/do-agent-memory-ts`](https://github.com/Volland/oxilite/tree/main/examples/do-agent-memory-ts) is a tested agent-memory Worker built this way.

## API

`D1Store` mirrors Oxigraph's JavaScript `Store`, asynchronously: `query`, `queryJson`, `update`, `load`, `bulkLoad`, `dump`, `add`, `delete`, `has`, `match`, `size`, `clear`, plus `cypher`, `explain`, `explainUpdate`, `explainCypher`, `optimize`, `materialize` (OWL 2 RL as SQL rules), `clearInferences`, `jsonld(options)` and `credentials(options)`. The [schema registry](https://github.com/Volland/oxilite/blob/main/docs/schema-registry.md) is there too: `registerSchemaGraph(graph, role, { appliesTo })`, `schemaGraphs()`, `setSchemaGraphActive`, `unregisterSchemaGraph`, `dropSchemaGraph`, `shapeIndex()`; registrations are RDF in `<oxilite:schema>`, written with the same portable SPARQL as on Oxigraph. `D1Store.open(db, { systemGraphs: true })` (or `npx oxilite-d1 schema --system-graphs`) starts a blank database with the `oxl:` vocabulary in `<oxilite:vocabulary>`; `installSystemGraphs()` adds it to an existing one. Query options add `reasoning: "rdfs" | "owl-ql"`, `include_inferred` and `include_schema_graphs`; create the store with `textIndex: true` for FTS5 search with `oxl:textMatch`.

| Import | Use |
|---|---|
| `@oxilite/d1` | Workers: pass the wasm module (`@oxilite/d1/oxilite.wasm`) to `D1Store.open` |
| `@oxilite/d1/node` | Node.js, tests and Miniflare: the wasm core loads itself |
| `npx oxilite-d1 schema [--jsonld] [--versioning log]` | Print the schema as a D1 migration (with the JSON-LD tables, with versioning) |
| `npx oxilite-d1 versioning-migration --from off --to log` | Print the migration that changes an existing database's versioning level |

### Versioning

Open or create the store with `versioning: "stamped"` (a store clock) or `"log"` (an immutable change log). Every D1 batch is then one commit:

```ts
const store = await D1Store.open(env.DB, { wasm, versioning: "log" });
await store.withCommit({ author: "ada", message: "close t1" }, (s) => s.update(closeTicket));
const before = await store.query(sparql, { as_of: "HEAD~1" });     // or "#42", "@2026-09-01T12:00:00Z"
const diff = await store.diff("HEAD~1");                            // [{ tick, added, quad }]
const log = await store.history(20);                                // [{ tick, time, kind, author, message, added, removed }]
```

`versioning()`, `setVersioning(level, { allowLoss })`, `setCommitInfo`, `changes(after)`, `resolveVersion` and `purge(pattern, reason)` complete the API. `SERVICE <oxilite:version/HEAD~1> { … }` compares versions inside one query, `GRAPH <oxilite:history>` reads commits and changes as RDF, and `cypher(q, {}, { asOf: "HEAD~1" })` matches the past. Measured on D1: `stamped` writes 4.82 rows per triple against 4.81 for a plain store, `log` 6.82 and `log` with `asOfIndex` 8.82. A store keeps its level: change an existing database with a migration.

## Tips

- Run `store.optimize()` after large imports, not on every request: it refreshes planner statistics.
- Drop the graph index (`--no-graph-index`, `graphIndex: false`) if you only use the default graph: one fewer index write per triple.
- Use `explain()` in development to check a query compiles fully to SQL. Queries that need the Rust fallback evaluator report "unsupported" on D1.

More in the [D1 guide](https://github.com/Volland/oxilite#using-oxilite-with-cloudflare-d1). A complete Worker with its migration and a Miniflare test is in [`examples/d1-worker-ts`](https://github.com/Volland/oxilite/tree/main/examples/d1-worker-ts).

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
