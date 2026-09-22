# @oxilite/d1

An Oxigraph-compatible SPARQL 1.1 store on a Cloudflare D1 database. The oxilite core runs as WebAssembly: SPARQL is compiled to SQL and sent to your `env.DB` binding, so every query is one D1 call and every update is one atomic batch.

```ts
import { D1Store } from "@oxilite/d1";
import wasm from "@oxilite/d1/oxilite.wasm";

export default {
  async fetch(req: Request, env: { DB: D1Database }) {
    const store = await D1Store.open(env.DB, { wasm, migrated: true });
    const q = new URL(req.url).searchParams.get("query") ?? "SELECT * WHERE { ?s ?p ?o } LIMIT 10";
    return new Response(await store.queryJson(q), { headers: { "content-type": "application/sparql-results+json" } });
  },
};
```

Create the schema as a migration with `npx oxilite-d1 schema > migrations/0001_oxilite.sql`. In Node (tests, Miniflare), import from `@oxilite/d1/node`.

The API mirrors Oxigraph's JavaScript `Store` (`query`, `update`, `load`, `dump`, `add`, `delete`, `has`, `match`, `size`), asynchronously, plus `explain`, `bulkLoad`, `optimize`, `materialize` (OWL 2 RL) and query options for RDFS / OWL reasoning and full-text search. See the [oxilite repository](https://github.com/Volland/oxilite).
