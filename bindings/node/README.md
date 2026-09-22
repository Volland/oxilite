# @oxilite/node

An Oxigraph-compatible SPARQL 1.1 store for Node.js, stored in a SQLite file (bundled SQLite, or a SQLite shared library you choose). Oxigraph's own JavaScript store tests run against it.

```ts
import { Store, type Term } from "@oxilite/node";

const store = new Store("data.sqlite");   // new Store() in memory, or new Store(quads)
store.load("@prefix ex: <http://ex/> . ex:a ex:knows ex:b .", { format: "text/turtle" });
for (const row of store.query("SELECT ?x WHERE { ?x ?p ?o }") as Map<string, Term>[]) {
  console.log(row.get("x")?.value);
}
```

Beyond Oxigraph's API: `explain`, `bulkLoad`, `optimize`, `backup`, `materialize` (OWL 2 RL, SQL rules or the `reasonable` reasoner), query options `reasoning: "rdfs" | "owl-ql"` and `include_inferred`, and full-text search with `{ textIndex: true }`.

**Platforms.** Version 0.1.0 ships a prebuilt binary for macOS on Apple silicon (`darwin-arm64`). On other platforms, build it from a checkout of the [oxilite repository](https://github.com/Volland/oxilite) with `npm run build:native -w @oxilite/node`.
