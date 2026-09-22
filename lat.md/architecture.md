# Architecture

oxilite is an Oxigraph-compatible RDF/SPARQL database that uses SQLite as its only storage engine, so it runs anywhere SQLite runs — including Cloudflare D1.

The guiding constraint: the engine may be *remote* (D1 answers over the network, billed per row) and may forbid extensions, so every operation is compiled to as few self-contained SQL statements as possible. See [[decisions]] for the reasoning behind each choice and [[milestones]] for delivery order.

## Crate layout

The workspace splits a pure, I/O-free core from thin backends and bindings, so the same compiler serves native, D1 and JavaScript hosts.

| Crate / package | Role |
|---|---|
| `oxilite-core` | Sans-IO: term encoding, schema, SPARQL→SQL compiler, planner, update planner, result decoding, spareval fallback |
| `oxilite` | Umbrella with Oxigraph's module layout (`model`, `io`, `sparql`, `store`): `store::Store` is a drop-in for `oxigraph::store::Store`, `AsyncStore<B>` offers the same API asynchronously |
| `oxilite-rusqlite` | Native backend on rusqlite (bundled or system SQLite) with `oxilite_*` UDFs |
| `oxilite-dylib` | Native backend that `dlopen`s a user-supplied `libsqlite3` path — no build-time link |
| `oxilite-d1` | Rust Workers backend over `worker::D1Database` (wasm32) |
| `oxilite-wasm` | wasm-bindgen export of the sans-IO core for JavaScript drivers |
| `@oxilite/node` | napi-rs Node.js binding with TypeScript types |
| `@oxilite/d1` | TypeScript D1 driver running the wasm core against `env.DB` |
| `oxilite-compat` (`testsuite/`) | Compatibility harness: Oxigraph's W3C runner and store tests run on oxilite, see [[test-plan#Oxigraph compatibility harness]] |
| `oxilite-reason` (M4) | TBox closure, query rewriting, OWL 2 RL materialization |
| `oxilite-validate` (M5) | rudof `srdf` trait implementation and D1 prefetch adapter |

Upstream reuse: `oxrdf`, `oxrdfio`/`oxttl`, `spargebra`, `spareval`, `sparesults`, `oxsdatatypes` (Oxigraph 0.5 family, `rdf-12`/`sparql-12` on), rudof's `srdf`/`shacl_*`/`shex_*` (same Oxigraph family), and `reasonable` for OWL 2 RL.

## Sans-IO core

The core never performs I/O: each operation yields SQL `Request`s and consumes `Response`s, so drivers for rusqlite, dlopen, D1 and JavaScript are each a few lines.

A `Request` is a list of statements plus a `Mode` (`Read` or `Atomic`). `Atomic` maps to one SQLite transaction natively and to one `db.batch()` on D1 — the only atomic unit D1 offers. Operations that need several round-trips (a query, then a term-lookup for its results) are step machines: `step(response) -> Execute(request) | Done(output)`.

`Capabilities` describe the backend: maximum SQL length, maximum statements per request, whether `oxilite_*` UDFs exist, whether interactive transactions exist, and whether 64-bit integers must travel as TEXT. The compiler adapts SQL to them. See [[crates/oxilite-core/src/sql.rs#Capabilities]].

All constants are inlined as SQL literals (ids are integers, strings are escaped), which avoids D1's 100-bound-parameter limit and makes every statement self-contained.

## Term encoding

Every RDF term maps to a tagged, positive 64-bit integer computed in Rust, so writes never need a read-back and SPARQL constants compile to integer literals.

Layout: sign bit 0, 4 tag bits, 59 payload bits. Tags: IRI, blank node, simple string, language string, directional language string, other typed literal, triple term — all *hashed* (xxh3-64 of a canonical key, masked to 59 bits) and stored in `terms` — plus inline `xsd:integer` (canonical, ±2^58) and inline `xsd:boolean`, which need no `terms` row.

Inline integers carry `value + 2^58`, so integer ids sort by value and range filters on them become id-range scans. Non-canonical lexical forms (`"012"^^xsd:integer`) are hashed, preserving exact term identity. A simple literal and `xsd:string` share one id, following RDF 1.1.

Hash collisions are detected atomically by a `BEFORE INSERT` trigger on `terms` that aborts the batch when an id already maps to a different term. See [[crates/oxilite-core/src/encoding.rs#encode_literal]] and [[crates/oxilite-core/src/encoding.rs#Tag]].

## Storage schema

Seven small tables; the quad table is a `WITHOUT ROWID` clustered index whose secondary indexes contain every column, so every triple-pattern scan is index-only.

- `quads(s, p, o, g)` — `PRIMARY KEY (s,p,o,g) WITHOUT ROWID, STRICT`; indexes `posg`, `ospg`, optional `gspo`. `g = 0` is the default graph.
- `terms(id INTEGER PRIMARY KEY, lex, dt, lang, dir, num, nt, ts)` — typed side columns (`num` numeric value, `nt` numeric type rank, `ts` epoch seconds; `dir` is the base direction of directional strings or the timezone flag of dates) with partial indexes, so value filters never re-parse lexical forms.
- `triple_terms(id, s, p, o, vk, sk)` — RDF 1.2 triple terms, with a value key (equal for value-equal triples, so `=` is one comparison) and a sort key (ORDER BY order), both computed in Rust at write time.
- `graphs(id)` — named graphs, including empty ones created with `CREATE GRAPH`.
- `stats_pred`, `stats_class` — planner statistics ([[architecture#Query planner#Statistics]]).
- `update_buffer(op, s, p, o, g)` — staging for atomic SPARQL UPDATE ([[architecture#Updates and atomicity]]).
- `oxilite_guard(graph_does_not_exist, graph_already_exists)` — assertions inside batches: inserting a non-NULL value fails a `CHECK` constraint named after the violated SPARQL condition (e.g. `CREATE GRAPH` on an existing graph), aborting the batch.
- `oxilite_meta(key, value)` — schema version and options.

Three mandatory permutations cover every bound/unbound combination of s, p, o; `g` is last in each so graph restrictions are checked inside the index. When another position is bound, graph conditions are emitted as `+q.g = …` (SQLite's unary-plus convention) so the planner never picks `gspo` for a `g = 0` that almost every quad matches — this alone made star queries two orders of magnitude faster. Index count is kept deliberately low because D1 bills every index entry written. See [[crates/oxilite-core/src/schema.rs#create_schema]].

## Write path

Inserts are one batch of `INSERT OR IGNORE` statements — terms, triple terms, graph names, then quads — with no read-back, chunked below the backend's SQL-length limit.

Because ids are hashes, the same term always encodes to the same id on every client, and re-inserting is idempotent. `insert` reports whether the quad was new from the statement's change count. Terms are never garbage-collected on delete (like Oxigraph); `optimize()` may offer that later. See [[crates/oxilite-core/src/writer.rs#EncodedQuads]].

## SPARQL to SQL compiler

The compiler lowers `spargebra` algebra to a single SQL `SELECT` per query whenever possible; anything it cannot express raises `Unsupported` and falls back to Rust evaluation.

Variables become integer id columns (`vN`) for stored terms, or value column groups (`vN_i` id, `vN_k` kind, `vN_l` lexical form, `vN_d` datatype, `vN_g` language, `vN_n` number, `vN_t` numeric rank, `vN_s` timestamp, `vN_b` boolean) for computed values and query constants, which need not be stored; the id remains the join key. Each pattern compiles to a *block* (FROM items, WHERE conditions, bindings, and solution-modifier state); blocks are merged as long as they stay plain and are sealed into subqueries only when SQL semantics require it (after DISTINCT, LIMIT, GROUP BY, or across UNION).

### Shallow SQL

Generated SQL must parse on SQLite builds with a fixed parser stack (`YYSTACKDEPTH=100`, as in the system SQLite of macOS and Ubuntu, and possibly D1), and stay under the backend's statement size.

Hence per-term facts that would need deep expressions — triple-term equality and order, date timezone presence — are precomputed at write time; nested UNIONs compile to one flat N-way `UNION ALL`; and a query whose SQL exceeds `Capabilities::max_sql_len` is reported `Unsupported` (fallback) instead of being sent. The harness runs its dylib variant on the platform SQLite to catch regressions.

### Graph patterns

Basic graph patterns become self-joins on `quads` with the planner's order enforced by `CROSS JOIN`; other operators map to SQL join forms.

- BGP → `quads q1 CROSS JOIN quads q2 …` with constant and join conditions in WHERE.
- Join → block merge; shared nullable variables use `(a = b OR a IS NULL OR b IS NULL)` and `COALESCE`.
- OPTIONAL → `LEFT JOIN` (parenthesised joins keep index use), filter in the `ON` clause.
- UNION → `UNION ALL` of sealed branches, missing variables padded with NULL.
- MINUS → `NOT EXISTS` with compatibility and domain-overlap conditions.
- FILTER EXISTS → correlated `EXISTS`, with outer bindings substituted.
- VALUES → a `VALUES` table; BIND → a computed column.
- GRAPH → `g` conditions; default graph `g = 0`; union-default-graph and multi-`FROM` datasets deduplicate with a `NOT EXISTS` on a lower `g`.

### Expressions

SPARQL values are bundles of SQL fragments (kind, lexical form, datatype, language, number, timestamp, boolean); SQL NULL is the SPARQL error, so three-valued logic matches SPARQL.

Inline values decode arithmetically from the id; hashed values use correlated `terms` lookups, which SQLite evaluates at the earliest loop level. Static type knowledge prunes comparison branches (a numeric constant reduces `?x > 5` to a single numeric comparison). Functions are tiered: SQL built-ins everywhere, rewrites (simple REGEX → `instr`/`substr`), native UDFs (`oxilite_regex`, `oxilite_replace`, hashes, Unicode case) only when `Capabilities::udf`, otherwise the fallback. See [[crates/oxilite-core/src/compiler/expr.rs#V]].

### Property paths

Sequence, alternative and inverse paths are rewritten into joins and unions; `*`, `+` and `?` become recursive CTEs seeded from a constant endpoint when one exists.

When the path is joined with a pattern that binds one endpoint (`?c a ex:C . ?c ex:knows+ ?x`), the CTE is seeded from that pattern's distinct values (magic-set style) instead of computing the whole closure; only fully unseeded closures compute all pairs, and `explain()` flags them. Every CTE carries a graph column, so walks stay inside one graph under `GRAPH ?g`.

### Aggregates and solution modifiers

GROUP BY, aggregates, ORDER BY, DISTINCT and LIMIT/OFFSET compile into one SELECT when their SPARQL order allows it, sealing into subqueries otherwise.

`COUNT` produces inline integer ids arithmetically. `SUM`/`AVG` track the numeric type rank for type promotion; an integer `AVG` also returns the exact sum and count so the decoder divides with Oxigraph's decimal precision. `MIN`/`MAX` over stored terms use SQLite's bare-column rule to return the actual term. ORDER BY follows [[decisions#D11 Total order for incomparable literals]]. `REDUCED` behaves like `DISTINCT`, as in Oxigraph.

### Fallback evaluator

When compilation raises `Unsupported`, sync backends evaluate the query with `spareval`, but the largest compilable subtrees still run as SQL.

The query is rewritten so each compilable subtree becomes `SERVICE <urn:oxilite:sql:N> { … }`; a spareval service handler runs the precompiled SQL (memoized, since it does not depend on outer bindings) and only the operators above it — say a custom function in a FILTER — are evaluated in Rust. Subtrees under `GRAPH` are not replaced (a SERVICE would lose the active graph). Remaining quad access goes through a `QueryableDataset` backed by per-pattern SQL scans, where hash ids make `internalize_term` free. On D1 there is no sync access, so queries needing the fallback return `Unsupported`; `explain()` reports the reason and lists the subqueries that still run as SQL.

## Query planner

A greedy, statistics-driven planner orders triple patterns and forces that order with `CROSS JOIN`; SQLite still chooses the index for each pattern.

SQLite's own planner sees N identical copies of `quads` and has only per-index averages, so it cannot tell `rdf:type` from a rare predicate. The planner picks the most selective pattern first, then repeatedly the cheapest pattern *connected* to already-bound variables, avoiding Cartesian products. The right side of an OPTIONAL is planned as if the left side's variables were bound, since SQLite evaluates it per left row — this keeps OPTIONAL on a foreign key linear (Oxigraph's optimizer regression). `explain()` lists every BGP's join order with estimated rows and warns about Cartesian products and unseeded closures. A query option lets SQLite plan instead, for benchmarking.

### Statistics

Per-predicate triple counts and distinct subject/object counts, plus per-class instance counts, refreshed explicitly by `optimize()` or after bulk loads.

Stats are not maintained on every write: on D1 that would double write billing and create a hot row. Stale stats only degrade plans, never correctness. Without stats, static heuristics apply (bound subject ≫ bound object ≫ bound predicate). See [[crates/oxilite-core/src/stats.rs#Stats]].

## Updates and atomicity

Every write — `INSERT DATA`, `DELETE/INSERT … WHERE`, `CLEAR`, `DROP`, `extend` — compiles to self-reading SQL in one atomic request, which is exactly one D1 batch.

`DELETE/INSERT … WHERE` evaluates the WHERE once into `update_buffer` (delete rows and insert rows computed *before* any modification), then deletes, inserts, and clears the buffer — all in the same batch, so semantics match SPARQL's "evaluate WHERE first" rule without interactive transactions. Bulk loads are chunked across batches and documented as non-atomic, like Oxigraph's `BulkLoader`. Interactive `transaction(|tx| …)` exists only on backends reporting `interactive_transactions`.

## Backends

Four ways to reach SQLite, all behind the same sans-IO contract.

### Native rusqlite

In-process SQLite (bundled by default) with `oxilite_*` UDFs registered, Unicode `upper`/`lower`, WAL mode and savepoint-based atomic requests.

### Dynamic libsqlite3

Loads a SQLite shared library from a path at runtime (`libloading`), binding only the C API functions oxilite needs, for hosts that ship their own SQLite.

### Cloudflare D1

Async backend over the D1 binding: `Atomic` requests become `db.batch()`, ids travel as TEXT because JavaScript numbers lose precision above 2^53, and statements stay under D1's size limits.

### JavaScript drivers

The wasm build of the core exposes the step machine to JavaScript, so a TypeScript driver can run SPARQL on `env.DB` without a Rust Worker.

## Reasoning

RDFS/OWL-QL reasoning by query rewriting against a small materialized TBox closure; full OWL 2 RL materialization is explicit and opt-in. Delivered in M4.

`tbox_closure(kind, sub, sup)` holds transitive subClassOf/subPropertyOf plus inverse, symmetric, transitive, domain and range facts; it is recomputed on `optimize()` and schema changes. Reasoning is chosen per query (`None | Rdfs | OwlQl`, default `None` like Oxigraph). `materialize()` writes OWL 2 RL inferences into `quads_inf` using SQL fixpoint rules (D1) or `reasonable` (native), rebuilt from scratch.

## Validation

SHACL and ShEx validation by running rudof's engines unchanged over oxilite through rudof's `srdf` traits. Delivered in M5.

Native backends implement `Rdf + NeighsRDF + QueryRDF` directly (so rudof's SPARQL paths use the oxilite compiler). D1 cannot block, so a prefetch adapter loads the relevant subgraph (target nodes, shape predicates, `sh:node` closure) into rudof's in-memory `SRDFGraph` with batched SQL, subject to a size limit. Behind the `validation` feature because rudof is a large dependency tree.

## Bindings

A Node.js package and a Cloudflare D1 package, both typed TypeScript, over the same core.

`@oxilite/node` (napi-rs) wraps `blocking::Store` on rusqlite or a dlopen'ed library: `query`, `update`, `load`, `dump`, `insert`/`delete`, `explain`, `optimize`, returning RDF/JS-style term objects. `@oxilite/d1` runs the wasm core against a `D1Database` binding and exposes the same API asynchronously.
