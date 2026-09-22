# oxilite

**An Oxigraph-compatible RDF database and SPARQL engine that uses SQLite as its storage engine. It runs anywhere SQLite runs, including Cloudflare D1.**

> **Status: M1 (storage core) implemented.** The Rust store, native backends and the Oxigraph compatibility harness work today; D1, Node/TypeScript packages, reasoning and validation are specified and being implemented milestone by milestone. See [Roadmap](#roadmap).

---

## Why oxilite?

[Oxigraph](https://github.com/oxigraph/oxigraph) is an excellent Rust SPARQL database. It stores data in RocksDB, which needs a native storage engine and a local filesystem. That rules it out in exactly the places where many modern apps run:

- **Cloudflare D1 and similar edge databases.** You get a managed SQLite you can send SQL to, but you can't install extensions, load native libraries, or run RocksDB.
- **Hosts that ship their own SQLite.** Mobile apps, embedded devices, sandboxes, and platforms where the SQLite library is provided and extensions are disabled.
- **Apps that already use SQLite** and want a knowledge graph in the same file, backed up and replicated with the same tools.

oxilite gives you the Oxigraph experience (same data model, same SPARQL semantics, the same Rust API) using **only plain SQL on a standard SQLite**. It needs no extensions, no custom functions (they're optional, used when available), and no filesystem access beyond what the host SQLite provides.

### What makes it fast

Using SQLite as a triple store is not new. What oxilite adds is a design built around how SQLite executes queries, and around the costs of a *remote* SQLite:

| Technique | Why it matters |
|---|---|
| **Hash ids computed in Rust.** Each term becomes a tagged 64-bit integer (xxh3). | Writes never read anything back. SPARQL constants become integer literals at compile time, with no dictionary lookup. |
| **Inline values.** Canonical integers and booleans live inside the id, and integer ids sort by value. | `FILTER(?age > 30)` on inline integers needs no join, and ranges become id ranges. |
| **Covering `WITHOUT ROWID` indexes.** `quads` is clustered on `spog`, with `posg`, `ospg` and an optional `gspo`. | Every triple-pattern scan is index-only. Only 3–4 B-trees are written per quad, which matters because D1 bills every index entry. |
| **One SQL statement per query.** Joins, OPTIONAL, UNION, MINUS, aggregates, property paths (recursive CTEs) and ORDER BY/LIMIT all compile into a single `SELECT`. | On D1 each round-trip is milliseconds. A query costs 1–2 round-trips, not one per triple pattern or per row. |
| **Our own join ordering.** Per-predicate and per-class statistics drive a greedy planner, enforced with `CROSS JOIN`. | SQLite's planner can't tell `rdf:type` from a rare predicate in an N-way self-join. Choosing the order ourselves avoids scans that are 1000× too large. |
| **Typed side columns.** `num`, `nt` and `ts` columns (with partial indexes) sit on the term dictionary. | Numeric and date comparisons never re-parse lexical forms. |
| **Atomic = one batch.** Every write, including `DELETE/INSERT … WHERE`, compiles to self-reading SQL that fits in one transaction. | D1 has no interactive transactions; `batch()` is its only atomic unit. |

The full rationale is in [`lat.md/decisions.md`](lat.md/decisions.md) and [`lat.md/architecture.md`](lat.md/architecture.md).

### A "super combo" of existing Rust RDF crates

oxilite reuses the RDF ecosystem wherever it can:

| From | Reused for |
|---|---|
| **Oxigraph 0.5 family**: `oxrdf`, `oxrdfio`/`oxttl`, `spargebra`, `sparesults`, `oxsdatatypes`, `spareval` | Data model, all RDF parsers and serializers, the SPARQL parser and algebra, result formats, XSD datatypes, and a fallback evaluator for anything the SQL compiler can't express |
| **[rudof](https://github.com/rudof-project/rudof)**: `srdf`, `shacl_validation`, `shex_validation` | SHACL and ShEx validation over the store (same Oxigraph crate family, so no conversions) |
| **[reasonable](https://github.com/gtfierro/reasonable)** | OWL 2 RL materialization on native backends |
| **SQLite itself** | Storage, indexes, transactions, recursive CTEs, FTS5 |

---

## How it works

```
             SPARQL / RDF / Store API
                        │
        ┌───────────────▼────────────────┐
        │          oxilite-core           │   sans-IO: never touches a database
        │  spargebra → algebra → planner  │
        │  → SQL compiler → Request       │
        │  Response → decoder → results   │
        └───────────────┬────────────────┘
                        │  Request { statements, Read | Atomic }
     ┌──────────────┬───┴───────────┬────────────────┐
     ▼              ▼               ▼                ▼
 rusqlite       dylib (dlopen    D1 (Rust Worker,  wasm core +
 (bundled)      your libsqlite3)  worker::D1)      TS driver on env.DB
```

The core is **sans-IO**. Every operation yields SQL `Request`s and consumes `Response`s. That's why the same compiler serves native SQLite, a SQLite library loaded from a path at runtime, and D1 over the network. A backend is only "run these statements, atomically if asked".

### Storage schema

```sql
CREATE TABLE quads (s INTEGER NOT NULL, p INTEGER NOT NULL, o INTEGER NOT NULL,
                    g INTEGER NOT NULL DEFAULT 0,              -- 0 = default graph
                    PRIMARY KEY (s, p, o, g)) WITHOUT ROWID, STRICT;
CREATE INDEX quads_posg ON quads(p, o, s, g);
CREATE INDEX quads_ospg ON quads(o, s, p, g);
CREATE INDEX quads_gspo ON quads(g, s, p, o);                  -- optional

CREATE TABLE terms (id INTEGER PRIMARY KEY, lex TEXT NOT NULL, dt TEXT, lang TEXT,
                    dir INTEGER, num REAL, nt INTEGER, ts REAL) STRICT;
-- + triple_terms (RDF 1.2), graphs, stats_pred, stats_class, update_buffer, oxilite_meta
```

### What a query becomes

```sparql
SELECT ?name WHERE {
  ?p a ex:Person ; ex:age ?age ; ex:name ?name .
  FILTER(?age > 30)
}
```

compiles to a single statement, with the planner choosing `ex:age` first because statistics say it's more selective than `rdf:type`:

```sql
SELECT q2.o AS v2
FROM quads q1 CROSS JOIN quads q0 CROSS JOIN quads q2
WHERE q1.p = 2305843... AND +q1.g = 0
  AND (CASE (q1.o >> 59) WHEN 6 THEN (q1.o & 576460752303423487) - 288230376151711744
       WHEN 5 THEN (SELECT num FROM terms WHERE id = q1.o AND nt IS NOT NULL) END) > 30
  AND q0.s = q1.s AND q0.p = 2305843... AND q0.o = 2305843... AND +q0.g = 0
  AND q2.s = q1.s AND q2.p = 2305843... AND +q2.g = 0
```

The filter is applied right after the first scan, before any join. The unary `+` on graph conditions keeps SQLite from choosing the graph index for `g = 0` (which nearly every quad matches), so each alias uses the right covering permutation. Inline integers are compared arithmetically; only non-inline numbers (decimals, doubles) look up `terms`. `store.explain(query)` shows the SQL, the join order and the estimates.

### Planner benchmark (M1)

`cargo run --release -p oxilite --example planner_bench` loads 350 010 quads and runs three join-heavy queries with oxilite's planner and with SQLite's own (Apple Silicon laptop, in-process SQLite):

| query | oxilite planner | SQLite planner |
|---|---|---|
| star with a rare badge | 75 µs | 7.2 ms |
| friends of badged people | 96 µs | 87 µs |
| city + age range filter | 478 µs | 7.8 ms |

---

## Usage

### Rust: drop-in for `oxigraph::store::Store`

```toml
[dependencies]
oxilite = "0.1"          # bundled SQLite via rusqlite
```

```rust
use oxilite::store::Store;             // was: use oxigraph::store::Store;
use oxilite::io::RdfFormat;
use oxilite::sparql::QueryResults;

let store = Store::open("data.sqlite")?;             // or Store::new() for in-memory
store.load_from_reader(RdfFormat::Turtle, TURTLE.as_bytes())?;

if let QueryResults::Solutions(solutions) =
    store.query("SELECT ?s WHERE { ?s a <http://schema.org/Person> }")?
{
    for s in solutions {
        println!("{}", s?.get("s").unwrap());
    }
}

store.update("INSERT DATA { <http://ex/a> <http://ex/p> 42 }")?;
store.optimize()?;          // refresh planner statistics (replaces RocksDB compact)
println!("{}", store.explain("SELECT * WHERE { ?s ?p ?o } LIMIT 1")?);
```

### Rust: your own SQLite library, loaded at runtime

```rust
use oxilite::dylib::DylibBackend;

// features = ["dylib"]
let store = oxilite::store::Store::open_with_library("/opt/vendor/lib/libsqlite3.so", "data.sqlite")?;
// or: Store::with_backend(DylibBackend::open(library, database)?)
```

Only the stable SQLite C API is bound (`open_v2`, `prepare_v2`, `step`, `column_*`, `finalize`, `errmsg`, `changes`, `exec`, and optionally `create_function_v2`), so any SQLite ≥ 3.37 works.

### Node.js (TypeScript)

```ts
import { Store } from "@oxilite/node";

const store = new Store("data.sqlite");               // or new Store() in memory
store.load(`@prefix ex: <http://ex/> . ex:a ex:knows ex:b .`, { format: "text/turtle" });

for (const row of store.query("SELECT ?x WHERE { ?x ?p ?o }") as Map<string, Term>[]) {
  console.log(row.get("x")?.value);
}
store.update("DELETE WHERE { ?s <http://ex/knows> ?o }");
```

The API mirrors Oxigraph's JS package (`query`, `update`, `load`, `dump`, `add`, `delete`, `has`, `match`, `size`), with RDF/JS terms. Oxigraph's own `store.test.ts` is part of our test suite.

---

## Using oxilite with Cloudflare D1

D1 is a managed, serverless SQLite. You can't load extensions or native code, and every call is a network round-trip billed per row read and written. oxilite is designed around exactly these constraints.

### 1. Create the database and apply the schema

```bash
npx wrangler d1 create my-graph
npx wrangler d1 migrations create my-graph oxilite-schema
npx oxilite-d1 schema > migrations/0001_oxilite-schema.sql     # schema as a D1 migration
npx wrangler d1 migrations apply my-graph --remote
```

```toml
# wrangler.toml
[[d1_databases]]
binding = "DB"
database_name = "my-graph"
database_id = "<id>"
```

### 2a. TypeScript Worker

```ts
import { D1Store } from "@oxilite/d1";

export default {
  async fetch(req: Request, env: { DB: D1Database }): Promise<Response> {
    const store = await D1Store.open(env.DB);           // init() is idempotent if you skip migrations
    const url = new URL(req.url);

    if (req.method === "POST" && url.pathname === "/update") {
      await store.update(await req.text());             // one atomic D1 batch
      return new Response(null, { status: 204 });
    }
    const q = url.searchParams.get("query") ?? "SELECT * WHERE { ?s ?p ?o } LIMIT 10";
    return new Response(await store.queryJson(q), {
      headers: { "content-type": "application/sparql-results+json" },
    });
  },
};
```

`@oxilite/d1` runs the oxilite core as WebAssembly. The core compiles SPARQL to SQL, the driver sends it to `env.DB`, and the core decodes the rows. No Rust toolchain is needed in your Worker project.

### 2b. Rust Worker

```rust
use worker::*;
use oxilite::{d1::D1Backend, AsyncStore};

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let store = AsyncStore::open(D1Backend::new(env.d1("DB")?)).await.map_err(|e| e.to_string())?;
    let q = req.url()?.query_pairs().find(|(k, _)| k == "query").map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| "ASK { ?s ?p ?o }".into());
    let results = store.query(&q).await.map_err(|e| e.to_string())?;
    Response::ok(results.to_json_string()?)
}
```

### What oxilite does differently on D1

| D1 constraint | How oxilite handles it |
|---|---|
| No extensions or user-defined functions | Every SPARQL function has a pure-SQL form or a rewrite. The few that don't (general `REGEX`, `REPLACE`, hashes) return a clear "unsupported on this backend" error, and `explain()` flags them. |
| No interactive transactions; `batch()` is the only atomic unit | Every write is one batch. `DELETE/INSERT … WHERE` stages its WHERE results in `update_buffer` inside the same batch, so SPARQL's evaluate-then-apply semantics hold. |
| Statement size limit (about 100 KB) and 100 bound parameters | Constants are inlined as escaped SQL literals and never bound. Large inserts are split into statements under the limit. |
| JavaScript numbers lose precision above 2^53 | Ids (60-bit) are selected as TEXT and parsed in the core. |
| Billed per row read and written | Few indexes, read-free writes, no per-write statistics updates, one statement per query, and deduplicated term resolution. |
| Per-invocation query limits | Queries use 1–2 requests; bulk loads are chunked (and documented as non-atomic across chunks, like Oxigraph's `BulkLoader`). |

Check [Cloudflare's current D1 limits](https://developers.cloudflare.com/d1/platform/limits/); oxilite's `Capabilities::d1()` defaults are kept below them and can be tuned.

**Tips for D1:**
- Run `store.optimize()` after large imports. It refreshes planner statistics in one batch that reads the whole table, so don't call it on every request.
- Create the store without the graph index (`graph_index: false`) if you only use the default graph. That saves one index write per quad.
- Use `explain()` in development to confirm your queries compile fully to SQL.

---

## Compatibility with Oxigraph

"Behaves like Oxigraph" is tested, not assumed. The [`oxigraph-compat-harness`](openspec/changes/oxigraph-compat-harness/) runs:
- **Oxigraph's own W3C manifest runner**, ported from `oxigraph/testsuite`, over `rdf-tests` and Oxigraph's `oxigraph-tests`. Each test gets three verdicts: oxilite vs. expected, Oxigraph vs. expected, and oxilite vs. Oxigraph.
- **Oxigraph's store API tests** (`lib/oxigraph/tests/store.rs`) against `oxilite::blocking::Store`, changing only the import.
- **Oxigraph's JS tests** (`js/test/store.test.ts`) against `@oxilite/node`.
- **A differential corpus**: seeded datasets and query families run on both engines, with statistics on and off and with our planner and SQLite's. The results must match.
- **An allow-list** of justified divergences, each linked to a decision, and a generated `COMPATIBILITY.md`.

Intentional differences:
- `backup` is `VACUUM INTO` and `compact` is `optimize()`; RocksDB-specific methods are absent.
- On D1, queries that need the Rust fallback evaluator return an "unsupported" error instead of running.
- Decimal values are compared as IEEE doubles (the lexical form is always preserved exactly).

---

## Reasoning and validation

- **Reasoning (M4).** Per-query `Rdfs` / `OwlQl` entailment by rewriting against a small materialized TBox closure. Queries stay single statements and there are no extra writes. OWL 2 RL materialization is available on request via `materialize()`, using SQL fixpoint rules everywhere and `reasonable` natively.
- **Validation (M5).** SHACL and ShEx through rudof, unchanged. Native stores implement rudof's `srdf` traits directly. On D1, the relevant subgraph is prefetched in a few batched queries (with a size limit).

---

## Roadmap

| Milestone | Scope | Done when | Status |
|---|---|---|---|
| Compat harness | Oxigraph test ports + differential corpus | runs in CI for every milestone | in progress (W3C suites + store API tests pass) |
| **M1** Storage core | encoding, schema, backends, load/dump, BGP+FILTER → SQL, planner | W3C syntax suites + ported store API tests pass | ✅ done |
| **M2** Full SPARQL 1.1 query | OPTIONAL, UNION, MINUS, aggregates, paths, subqueries, `explain()` | ≥ 95% W3C query suite | specified |
| **M3** Update + D1 | atomic SPARQL UPDATE, `oxilite-d1`, wasm core | W3C update suite on rusqlite and local D1 | specified |
| TS bindings | `@oxilite/node`, `@oxilite/d1` | node:test suites + Oxigraph JS tests | specified |
| **M4** Reasoning | TBox closure, rewriting, OWL 2 RL | entailment tests; agreement with `reasonable` | specified |
| **M5** Validation | rudof SHACL/ShEx | rudof suites over oxilite | specified |
| **M6** Performance | BSBM vs Oxigraph, FTS5 | published comparison | specified |

---

## Project documentation

- [`lat.md/`](lat.md/): the architecture knowledge graph (architecture, decisions, milestones, tests, test plan), checked by `lat check`.
- [`openspec/changes/`](openspec/changes/): one change per milestone, each with a proposal, requirement specs with scenarios, a design, and a task list (`openspec validate --all --strict`).

## License

Dual-licensed under MIT or Apache-2.0, like Oxigraph.
