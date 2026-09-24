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
| `oxilite-cypher` (M7) | openCypher parser, validation, planning and lowering to SPARQL algebra, the Rust tail (writes, lists, temporal values), see [[architecture#Property graph frontend]] |
| `oxilite-datalog` (M8) | Datalog parser, stratification and SQL generation, see [[architecture#Datalog frontend]] |

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

SQL has no common subexpressions, so size also grows when an expression uses an operand several times. Four rules keep it linear:

- Nested function calls whose operand SQL is large (e.g. `xsd:float(xsd:string(?price))`) are lifted: the operand becomes a column of a sealed subquery, in BIND, FILTER and aggregate arguments.
- Lexical parsing binds the lexical form once per row with a correlated `(SELECT … FROM (SELECT lex AS lx))`.
- Expressions that read many fields of a stored term join its `terms` row once instead of running one lookup per field.
- Value aggregates read their argument from a subquery marked `LIMIT -1`, which SQLite does not flatten into the aggregate.

`crates/oxilite/tests/plans.rs` checks every BSBM query stays under D1's 90 KB and scans no quad table or index.

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

An ORDER BY placed directly on a grouped block (algebra built without the parser's `Extend`, as Cypher lowering does) seals the block first when a sort key reads an aggregate, because the sort keys' scalar subqueries cannot contain an aggregate in SQLite.

`COUNT` produces inline integer ids arithmetically. `SUM`/`AVG` track the numeric type rank for type promotion; an integer `AVG` also returns the exact sum and count so the decoder divides with Oxigraph's decimal precision. `MIN`/`MAX` over stored terms use SQLite's bare-column rule to return the actual term. ORDER BY follows [[decisions#D11 Total order for incomparable literals]]. `REDUCED` behaves like `DISTINCT`, as in Oxigraph.

### Fallback evaluator

When compilation raises `Unsupported`, sync backends evaluate the query with `spareval`, but the largest compilable subtrees still run as SQL.

The query is rewritten so each compilable subtree becomes `SERVICE <urn:oxilite:sql:N> { … }`; a spareval service handler runs the precompiled SQL (memoized, since it does not depend on outer bindings) and only the operators above it — say a custom function in a FILTER — are evaluated in Rust. Subtrees under `GRAPH` are not replaced (a SERVICE would lose the active graph). Remaining quad access goes through a `QueryableDataset` backed by per-pattern SQL scans, where hash ids make `internalize_term` free. On D1 there is no sync access, so queries needing the fallback return `Unsupported`; `explain()` reports the reason and lists the subqueries that still run as SQL.

## Query planner

A greedy, statistics-driven planner orders triple patterns and forces that order with `CROSS JOIN`; SQLite still chooses the index for each pattern.

SQLite's own planner sees N identical copies of `quads` and has only per-index averages, so it cannot tell `rdf:type` from a rare predicate. The planner picks the most selective pattern first, then repeatedly the cheapest pattern *connected* to already-bound variables, avoiding Cartesian products. The right side of an OPTIONAL is planned as if the left side's variables were bound, since SQLite evaluates it per left row — this keeps OPTIONAL on a foreign key linear (Oxigraph's optimizer regression). `explain()` lists every BGP's join order with estimated rows and warns about Cartesian products and unseeded closures. A query option lets SQLite plan instead, for benchmarking.

### Statistics

Per-predicate triple counts and distinct subject/object counts, plus per-class instance counts, refreshed explicitly by `optimize()` or after bulk loads.

`stats_po` adds skew: for predicates with at most 1024 distinct objects (besides `rdf:type`, which has per-class counts), pairs at least four times as frequent as their predicate's average, the 2000 most frequent kept. A constant object then gets its real count instead of the average.

Stats are not maintained on every write: on D1 that would double write billing and create a hot row. Stale stats only degrade plans, never correctness. Without stats, static heuristics apply (bound subject ≫ bound object ≫ bound predicate). See [[crates/oxilite-core/src/stats.rs#Stats]].

## Updates and atomicity

Every write — `INSERT DATA`, `DELETE/INSERT … WHERE`, `CLEAR`, `DROP`, `extend` — compiles to self-reading SQL in one atomic request, which is exactly one D1 batch.

`DELETE/INSERT … WHERE` evaluates the WHERE once into `update_buffer` (delete rows and insert rows computed *before* any modification), then deletes, inserts, and clears the buffer — all in the same batch, so semantics match SPARQL's "evaluate WHERE first" rule without interactive transactions. Bulk loads are chunked across batches and documented as non-atomic, like Oxigraph's `BulkLoader`. Interactive `transaction(|tx| …)` exists only on backends reporting `interactive_transactions`.

Conditions SQL cannot express as data become constraint failures on `oxilite_guard`, whose CHECK columns (`graph_does_not_exist`, `graph_already_exists`, `computed_value_not_storable`) abort the whole batch and are mapped back to SPARQL errors (see [[crates/oxilite-core/src/error.rs]]). A template that stores a computed non-integer value (a decimal from arithmetic, a new string) raises `computed_value_not_storable`; native stores then rerun the update through spareval's `delete_insert` inside one transaction, while D1 reports it. `explain_update()` shows the SQL of every operation.

## Backends

Four ways to reach SQLite, all behind the same sans-IO contract.

### Native rusqlite

In-process SQLite (bundled by default) with `oxilite_*` UDFs registered, Unicode `upper`/`lower`, WAL mode and savepoint-based atomic requests.

### Dynamic libsqlite3

Loads a SQLite shared library from a path at runtime (`libloading`), binding only the C API functions oxilite needs, for hosts that ship their own SQLite.

### Cloudflare D1

Async backend over the D1 binding: `Atomic` requests become `db.batch()`, ids travel as TEXT because JavaScript numbers lose precision above 2^53, and statements stay under D1's size limits.

`Capabilities::d1()` encodes the limits the compiler respects: SQL under 90 KB per statement, at most 50 statements per batch (bulk loads are chunked), at most 5 terms per compound SELECT (larger UNIONs nest), GLOB patterns under 50 bytes, no UDFs, and no interactive transactions (so no spareval fallback: queries that do not compile report `unsupported`). The Rust backend reads batch results through js-sys, because worker-rs types `meta.last_row_id` as `i64` and a 60-bit term id arrives as a JavaScript float. The schema ships as a migration from `oxilite_d1::migration_sql`, and the store is opened with `open_existing` so no DDL runs per request. A release that adds a table therefore needs a new migration on D1, unlike the native backends where `create_schema` runs on every open: 0.3.0 added `schema_graphs`, `shapes_index`, `shapes_in` and `datalog_work`.

### JavaScript drivers

The wasm build of the core exposes the step machine to JavaScript, so a TypeScript driver can run SPARQL on `env.DB` without a Rust Worker.

`oxilite-wasm` exports an `Engine` whose methods (query, update, load, match, dump…) return a `Job`; `Job.step(responseJson)` yields either the next SQL request or the final output as JSON (see [[crates/oxilite-core/src/json.rs]]). It is built for the `web` target (Workers) and the `nodejs` target (Miniflare tests, CLI).

## Text search

Optional full-text search over string literals with SQLite FTS5, which works natively and on D1. See [[crates/oxilite-core/src/text.rs]].

With `StoreOptions::text_index`, `terms_fts` (external content over `terms`) indexes simple, language-tagged and directional strings; triggers on `terms` keep it current and enabling it on an existing store back-fills it. `oxl:textMatch(?literal, "fts query")` (`https://oxilite.dev/ns#textMatch`) compiles to `id IN (SELECT rowid FROM terms_fts WHERE terms_fts MATCH …)`. Without the index the fallback evaluator answers with the same tokenization (case-insensitive words, `word*` prefixes, every term required), so native stores get the same answers; D1 reports it unsupported. On D1 the index costs about 0.2 extra rows written per triple (see [[architecture#Benchmarks]]).

## Command line and HTTP endpoint

The `oxilite` binary (`crates/oxilite-cli`) loads, queries, explains and updates stores, and `oxilite serve` exposes the SPARQL 1.1 protocol on the routes of `oxigraph serve` (`/query`, `/update`, `/store`). See [[crates/oxilite-cli/src/main.rs]].

It also checks projects (`oxilite check`, [[architecture#Studio server#Check command]]), serves agents (`oxilite mcp`, [[architecture#Studio server#Agent tools]]) and runs the studio's language server (`oxilite studio-server`). It opens a SQLite file with the bundled SQLite, the same file through a SQLite shared library (`--library`), or a D1 database behind the local sidecar (`--d1-sidecar`). Results follow the `Accept` header (SPARQL JSON/XML/CSV/TSV, RDF formats for graphs). The benchmarks drive it with the official BSBM test driver.

## Studio server

`oxilite studio-server` is the language server behind oxilite studio, the VS Code extension: LSP over standard input and output with custom `oxilite/*` requests. See [[crates/oxilite-cli/src/studio/mod.rs]].

It runs in its own process so a panic or a long operation never takes the editor down, and it links the engine crates directly (SHACL included, which has no JavaScript binding). It is built on `lsp-server`, which needs no async runtime, matching the blocking `Store`. Requests: `oxilite/query` (SPARQL, returning the RDF/JS payload of `output_to_json` plus `elapsedMs` and `truncated`, capped at a row limit), `oxilite/status` and `oxilite/reload`; the server sends `oxilite/storeChanged` after every load. The studio's own design lives in the `oxilite-studio` repository; OpenSpec change `studio-server-skeleton`.

### Project store

The workspace's files loaded into a scratch store: data and ontologies into their graphs, shapes and rules kept aside, inferences materialized by producer. See [[crates/oxilite-cli/src/studio/project.rs#Project]].

The store lives at `<root>/.oxilite/studio.sqlite` (the server writes a `.gitignore` there) and is derived data, deleted on start. Hidden directories, `node_modules` and `target` are skipped; formats come from the extension (`.owl` is RDF/XML). Each file is parsed completely before any quad is written, with its IRI as base and its blank nodes renamed, so a syntax error loads nothing from that file and becomes a diagnostic at the parser's position.

A watched-file change reloads per graph: every graph a changed file belonged to or now belongs to is cleared and refilled from its files, and a manifest change reloads everything. Queries read the union of graphs with the profile's reasoning: `rdfs` and `owlql` rewrite queries, `owl2rl` materializes. Materialization runs OWL 2 RL, then each rule file under its relative path as producer, then OWL 2 RL again when rules exist; a failing rule file is a diagnostic and its producer is cleared. With a manifest, ontology graphs are registered in the schema registry, so reasoning reads their axioms only.

### Project manifest

`oxilite.toml` names graphs (IRI, globs, `data` or `ontology`), shapes and rules globs, the reasoning profile, validation options and tests; without it, conventions decide. See [[crates/oxilite-cli/src/studio/manifest.rs#Layout]].

The manifest is the whole truth: a file no glob matches is not loaded. Without one, a file's graph is its `file:` IRI and its role comes from the extension (`.dl` rules) and content (SHACL shape classes make shapes, `owl:Ontology` an ontology). A manifest that does not parse is a diagnostic on `oxilite.toml`, and the conventions apply until it is fixed.

### Live validation

SHACL runs on a worker thread after every change; each result becomes a diagnostic on the line of the focus node's statement with the failing path, linked to its shape. See [[crates/oxilite-cli/src/studio/validate.rs#Job]].

With reasoning on, a copy of the entailed graph is validated, so `sh:class` sees types that follow from the ontology; `validation.inferred = false` restricts it to asserted triples. A named shape is located where it is defined, a blank property shape where its path appears in a shapes file. Each run carries the project's generation and stops early, or is dropped, once a newer change has arrived. `oxilite/validationStarted` and `oxilite/validationChanged` report progress, and `oxilite/validationReport` returns the whole report.

### Store Explorer

`oxilite/explorer` returns tree nodes lazily: graphs with counts, the asserted class hierarchy with asserted and inferred instance counts, properties by use, files with their roles, and prefixes. See [[crates/oxilite-cli/src/studio/explorer.rs#children]].

`oxilite/describe` marks each statement about a resource as asserted or inferred (absent without reasoning) and names the producers of materialized ones, for the resource view. Queries and explains run on worker threads, so a long query does not block completion or diagnostics.

### Language features

Completion, hover, definitions, references, outlines and live syntax diagnostics for SPARQL and the Turtle family, from a lenient scanner and the strict parsers. See [[crates/oxilite-cli/src/studio/lang.rs#complete]].

The scanner ([[crates/oxilite-cli/src/studio/scanner.rs#scan]]) never fails: it tokenizes half-typed text with UTF-16 positions, resolves prefixed names and relative IRIs, and guesses each term's role (subject, predicate, object) by walking the triple structure. Completion uses the role: predicates in predicate position, classes after `a`, prefixed names in the store's own frequency order, and an edit declaring a prefix the document lacks. Syntax errors come from `spargebra` and the RDF parsers on every change; SPARQL predicates the connected store never uses get a warning.

### Source index

Where every IRI occurs in the workspace's text RDF files, and in which role, built while loading. See [[crates/oxilite-cli/src/studio/index.rs#SourceIndex]].

Definitions are subject occurrences; references are all occurrences; `triple_location` finds the line of a subject's statement with a given predicate, which is where SHACL results are reported. Occurrences are stored with small file ids to stay compact at a million triples.

### Connections

A request runs against the active connection or a named one: the Project store, or an attached SQLite store whose updates persist. See [[crates/oxilite-cli/src/studio/conn.rs#Target]].

`oxilite/query` runs a query, or an update when the text is one; an update on an attached store fails with code 1001 until it is re-sent with `confirmed`, and an update on the Project store is marked ephemeral. `oxilite/explain`, `oxilite/describe`, `oxilite/connections`, `oxilite/attach`, `oxilite/detach` and `oxilite/activate` complete the set, and `oxilite/connectionsChanged` reports changes. Each connection's vocabulary (predicates and classes by frequency, labels, comments) is computed on first use and dropped when the store changes.

### Rules, Cypher and files

`oxilite/datalog` runs a program's goal and `oxilite/cypher` a Cypher statement on the active connection; explain takes a `language`; `oxilite/import` and `oxilite/export` load and dump files. See [[crates/oxilite-cli/src/studio/conn.rs#Target#cypher]].

Datalog results come back as a solutions table; Cypher results as `kind: "cypher"` with the values' JSON (nodes, relationships, paths), which the graph view draws. Cypher names map to IRIs through a vocabulary built from the workspace's prefixes and a base namespace: the manifest's `[cypher] base`, else the store's most used namespace, so `:Person` means the data's own class. A Cypher statement that writes, or an import, on an attached store needs `confirmed` like an update. Datalog documents get parse and program-check diagnostics (safety, stratification, arity) placed on the rule they name, and completion of predicates and classes; Cypher documents get parse diagnostics and completion of labels, relationship types and property keys by their Cypher names.

### Justifications

`oxilite/why` explains an entailed triple as a proof tree: asserted leaves with their source line, inferences with the rule and premises behind them, recursively. See [[crates/oxilite-cli/src/studio/why.rs#Explainer]].

A conclusion of a rule file is explained by re-running that rule's body in the Datalog engine with its head bound to the triple; the first solution gives the premises. OWL 2 RL and RDFS conclusions are explained by rule templates (subclass, equivalence, domain, range, subproperty, inverse, symmetric, transitive, `sameAs`) run as SPARQL with the conclusion bound, over the connection's own reasoning. The producers recorded in `quads_inf_src` pick which rules are tried; a premise already on the path is not expanded again, and depth is capped. A triple that does not hold is reported as absent.

### Knowledge-graph tests

Tests in the manifest compare query results with an expected file, check SHACL conformance or exactly which shapes fail, or check entailments. See [[crates/oxilite-cli/src/studio/kgtest.rs#Runner]].

A test runs against the Project store as it is, or an overlay: its `data` fixture with the project's ontologies, or a copy of the project when it changes reasoning or rules. Results compare as multisets unless the query orders them; expected files are SPARQL results (JSON, XML, CSV, TSV), RDF for graph results, or JSON for Cypher. "Update snapshot" writes the current results in the expected file's format. Shapes in failing-shape lists resolve workspace prefixes, and a blank property shape counts for the named shape owning it. The server offers `oxilite/tests`, `oxilite/runTest` and `oxilite/updateSnapshot` to the editor's Test Explorer.

### Check command

`oxilite check [root]` loads a project exactly like the Project store and reports load and rule errors, SHACL results with file and line, and test outcomes; it exits with 1 on any error, violation or failing test. See [[crates/oxilite-cli/src/studio/check.rs#check]].

`--json` prints the report as JSON for CI annotations. It shares every code path with the editor's live diagnostics, so CI and the studio agree.

### D1 connections

An attached store can be a Cloudflare D1 database over its HTTP API, run through a blocking backend so every studio request works on it unchanged. See [[crates/oxilite-cli/src/studio/d1.rs#D1Http]].

A request's statements go to `/raw` as one multi-statement `sql` string, which D1 runs as a batch; with `Capabilities::d1()` 64-bit ids come back as text, so JSON keeps them exact. A meter adds up requests, rows read and rows written from each result's `meta`, and every payload on D1 carries its own cost. Connections to D1 open read-only unless asked; a write needs confirmation with an estimate (about 4.8 rows per quad for `INSERT DATA` and `DELETE DATA`), and materialization is an explicit, confirmed request since it is not atomic across rounds. Counting a D1 store's triples is a billed scan, so the connection list shows the meter instead. A `wrangler dev` database file under `.wrangler/state` attaches as a local SQLite file. Every connection's store sits behind [[crates/oxilite-cli/src/studio/d1.rs#Handle]], so SQLite and D1 share the code.

### Agent tools

`oxilite mcp` serves the studio's operations as Model Context Protocol tools over standard input and output: SPARQL and Datalog queries, a schema summary, SHACL validation, justifications and reload. See [[crates/oxilite-cli/src/studio/mcp.rs#Mcp]].

It loads a project like the Project store (`--root`, in memory) or opens a store file read-only (`--location`). Answers are compact text an agent reads well: tab-separated tables, N-Triples, validation results with file and line, proof trees as JSON. The editor registers it as an MCP server for the workspace.

### Datalog debugger

`oxilite/datalogDebug` reports, for each rule of a program, how many ways its body matches and how many facts its head predicate holds, with the strata and strategies the engine chose. See [[crates/oxilite-cli/src/studio/debug.rs#debug]].

Each count runs the program with an extra goal over one rule's body or head, capped at 100 000, so a rule that matches nothing or explodes stands out next to its line.

### ShEx and full-text search

ShEx schemas validate with their shape maps in the same background run as SHACL; the manifest's `text_index` builds the store's full-text index. See [[crates/oxilite-cli/src/studio/project.rs#Project#shex_jobs]].

A schema pairs with its manifest `[[shex]]` shape map (a file or inline), or by convention with the `.sm` file of the same name. Each nonconformant node becomes a result and a diagnostic on its statement, linked to the shape's line in the `.shex` file (the scanner indexes ShExC names too). `oxl:textMatch` answers without the index as well; the index makes it a `terms_fts` lookup. `oxilite/ontology` returns classes with instance counts, subclass links and properties with their domains and ranges for the ontology diagram ([[crates/oxilite-cli/src/studio/explorer.rs#ontology]]).

## Benchmarks

The Berlin SPARQL Benchmark runs oxilite (bundled SQLite, system SQLite, D1 through the sidecar) against Oxigraph with RocksDB using the official BSBM tools (`bench/bsbm.sh`, submodule `bench/bsbm-tools`).

The script generates a dataset, loads it into each engine, serves it, runs the explore and business-intelligence mixes with the BSBM test driver, and records load time, database size and the driver's XML results in `bench/results`. `bsbm-report` turns them into `summary.json` and the README table. `write-cost` measures D1 rows written per triple (index entries included) for each schema option on a local D1: about 4.8 by default, 3.8 without the graph index, 5.0 with the text index.

## Schema registry

Which named graphs hold schema rather than data — ontologies, SHACL shapes, ShEx schemas — recorded in `schema_graphs`. See [[crates/oxilite-core/src/registry.rs]] and [[decisions#D22 Schema graphs registered, not separated]].

Registering a graph lets reasoning be scoped to chosen ontologies, gives shapes one compiled source of truth, and lets axioms be kept out of queries over the data.

`schema_graphs(g, role, iri, version, sha256, imports, active, loaded_at)` labels a graph; its triples stay in `quads` and stay queryable. Registering a named graph creates it, so the registry never names a graph the term dictionary has not heard of. Every scoping predicate reads "the active graphs of this role, or every graph while none is registered", which is one SQL subquery, so a store that registers nothing behaves exactly as it did before the registry existed and no operation gains a round trip.

The registry is reached through `Store::register_schema_graph` / `schema_graphs` / `set_schema_graph_active` / `unregister_schema_graph` / `drop_schema_graph` and their async twins ([[crates/oxilite/src/schema_store.rs]]). Because a registration changes the *scope* of the derived caches and not just their content, it rebuilds both of them in the same atomic request. `drop_schema_graph` removes the registration and every quad of the graph in one request.

`QueryOptions::include_schema_graphs` (default true) decides whether registered schema graphs are matched at all. Set to false, [[crates/oxilite-core/src/reason.rs#Entailment]] returns a quad source with those graphs removed, which covers patterns, paths, `OPTIONAL` and `GRAPH ?g` at once; hiding ignores the active flag, since an inactive ontology graph is still schema. `named_graphs()` and the dumps are unaffected: the dataset is still the dataset.

### Compiled shape index

`shapes_index` and `shapes_in` hold the SHACL property shapes of the registered shapes graphs, pre-resolved per target class and path. See [[crates/oxilite-core/src/shapes.rs]].

Each entry carries `sh:datatype`, `sh:minCount`, `sh:maxCount`, `sh:pattern`, the `sh:in` values and whether the shape is relationship-valued (`sh:class` / `sh:node`).

They are caches with the discipline `tbox_closure` follows: rebuilt by pure `INSERT … SELECT` statements over `quads` and `terms` (an `rdf:rest*`/`rdf:first` recursive CTE walks `sh:in`), by `optimize()` and, inside the same atomic request, by any write touching a SHACL predicate ([[crates/oxilite-core/src/shapes.rs#is_shape_quad]]). `rdf:first`/`rdf:rest` are deliberately not triggers — they would refresh the index on every write touching any RDF list — so appending to an existing `sh:in` list without touching a `sh:` predicate leaves the index stale until the next `optimize()`. Shapes that target the same class and path merge field by field, by `GROUP BY` with `MAX`, which ignores NULLs. Target, path, datatype and pattern are stored as text; `sh:in` values keep their id plus the `terms` columns, so Rust rebuilds any term without a second request. `Store::shape_index()` reads both tables in one request.

## Reasoning

RDFS/OWL-QL reasoning by query rewriting against a small materialized TBox closure; full OWL 2 RL materialization is explicit and opt-in. Delivered in M4, see [[crates/oxilite-core/src/reason.rs]].

`tbox_closure(kind, sub, sup)` holds the class closure (subClassOf and equivalentClass), the RDFS and OWL property closures (OWL composes subPropertyOf, equivalentProperty, inverseOf and symmetric properties with a direction bit), transitive properties, and the classes a property's subjects and objects belong to (domains and ranges through sub-properties, super-classes and inverses). It is computed by recursive CTEs from the asserted quads of the active registered ontology graphs — of every graph while none is registered ([[architecture#Schema registry]]) — recomputed by `optimize()`, by any change to a registration, and inside the same atomic request as any write that touches schema triples; stores then reload the transitive properties, which live in memory with the statistics.

Reasoning is chosen per query (`QueryOptions::reasoning`: `None | Rdfs | OwlQl`, default `None` like Oxigraph). The compiler swaps each pattern's `quads` table for a derived table of entailed triples (a `SELECT DISTINCT` over UNION ALL arms: asserted, sub/inverse properties, types through the class closure, domains and ranges), so queries stay single statements and paths, OPTIONAL and the fallback all see the same entailments. A transitive property becomes a recursive CTE, walked from a constant endpoint when there is one. With a merged default graph the derived table merges graphs itself, so each entailed triple appears once. Literals never become subjects.

`materialize()` runs OWL 2 RL rules as `INSERT OR IGNORE INTO quads_inf … SELECT` statements over asserted plus inferred triples (all graphs merged, conclusions in graph 0, triples already asserted there skipped), one atomic request per round until a round adds nothing: the same code on SQLite, dlopen and D1 (where a round is one batch, and the whole run is not atomic). The rules are the ones `reasonable` implements (equality, property axioms, class expressions including lists and property chains, `scm-sco`, `scm-eqc1`, and `owl:Thing` typing); `materialize_with_reasonable()` (feature `reasonable`, crate `oxilite-reason`) computes the same closure in memory, and an agreement test checks both on sample ontologies. `include_inferred` makes queries read `quads ∪ quads_inf`. Every inference is attributed to its producer in `quads_inf_src`, so `clear_inferences_of(producer)` and a re-run of one producer leave the others' conclusions in place, and `inference_producers(quad)` names who derived a quad ([[decisions#D28 Inferences are attributed to producers]]).

## Validation

SHACL and ShEx validation by running rudof's engines unchanged over oxilite through rudof's RDF traits (`rudof_rdf`, formerly `srdf`). Delivered in M5 as the crate `oxilite-validate`, see [[crates/oxilite-validate/src/lib.rs]].

It depends on the umbrella crate, so it is added next to it rather than behind a feature.

Shapes come from a string or from the store: `shacl_schema_from_store` serializes a shapes graph — named explicitly, or the single graph registered as SHACL shapes ([[architecture#Schema registry]]) — and hands it to rudof's own parser, so stored shapes and a shapes file compile identically. More than one registered shapes graph and no explicit name is an error rather than a guess.

`StoreGraph` implements `Rdf + NeighsRDF + QueryRDF` over a blocking `Store` (default graph, or all graphs merged): neighbourhood lookups are SQL pattern scans and rudof's SPARQL-mode validation runs through the oxilite compiler. SHACL uses rudof's `shacl` crate (native and SPARQL engines), ShEx `shex_validation` with compact shape maps. On the W3C SHACL core suite and the shexTest validation suite, reports over a store equal rudof's over its in-memory graph, on bundled SQLite and the system `libsqlite3`.

rudof's validators are compiled out on wasm32, so D1 is validated from native code over any `AsyncBackend` (D1's HTTP API, or the Miniflare sidecar in tests). A prefetch loads what the shapes need into rudof's in-memory graph: targets (target classes, nodes, subjects/objects of target predicates with their triples, implicit class targets), the class hierarchy, and a breadth-first neighbourhood (outgoing triples, incoming ones for inverse-path predicates) to a configured depth, one request per hop and chunk of nodes. Beyond `max_triples` it fails with `Error::TooLarge` (limit and size found) instead of truncating.

## Property graph frontend

Cypher over the same quads as SPARQL: property graphs are an RDF 1.2 view of the dataset, queried through the same compiler, planner and backends. Delivered in M7 by `oxilite-cypher`; see [[decisions#D13 Cypher as a second frontend over the RDF store]].

Pipeline: Cypher text → parser ([[crates/oxilite-cypher/src/parser.rs#parse]]) → validation of scopes and static rules → planning into a SQL part and a Rust *tail* ([[crates/oxilite-cypher/src/plan.rs]]) → lowering of the SQL part to `spargebra` algebra ([[crates/oxilite-cypher/src/lower.rs]]) → the existing SPARQL-to-SQL compiler → the job ([[crates/oxilite-cypher/src/exec.rs#CypherJob]]), which materializes nodes and relationships, runs the tail and applies writes. `Store::cypher`, `AsyncStore::cypher`, the wasm engine and both JavaScript packages drive the same job. With `union_default_graph` (`useDefaultGraphAsUnion` in JavaScript), the materialization and shortest-path reads also span every graph, so JSON-LD documents and credentials read as one property graph; writes stay in the default graph.

### Mapping

Nodes are IRIs or blank nodes, labels are `rdf:type`, node properties are literal triples, and relationships are asserted triples. Relationship properties and parallel relationships live on RDF 1.2 reifiers.

A reifier (`_:r rdf:reifies <<( a :T b )>>`) is created only when a relationship has properties or is one of several between the same nodes; the triple stays for traversal ([[decisions#D14 Relationships as asserted triples, with reifiers only when needed]]). Names map to IRIs through a vocabulary ([[crates/oxilite-cypher/src/vocab.rs#Vocabulary]]): a base IRI, registered prefixes (`` :`schema:Person` ``), overrides, or absolute IRIs. Lists and maps are `rdf:JSON` literals; temporal values are `xsd:date`, `xsd:time`, `xsd:dateTime` and `xsd:duration` (a zoned datetime gets its own datatype). A property with several values reads as a list (`MultiValue::List`, the default), unless a SHACL shape declares `sh:maxCount 1`. Created nodes get a fresh IRI minted in Rust and an `rdf:type rdfs:Resource` marker, so a node without labels or properties still exists ([[decisions#D15 Fresh node IRIs from Rust]]).

### Planning and lowering

Reading clauses become one SPARQL query, compiled to SQL; the first clause SQL cannot express starts the Rust tail, and `MERGE` look-ups become optional matches of the SQL part ([[decisions#D16 Lowering to SPARQL algebra, with a Rust tail]]).

- `MATCH` patterns become BGPs: the planner orders them and SQLite joins them as for SPARQL. A pattern with alternatives (undirected relationships, variable lengths) becomes a `UNION` of branches; bindings and filters that read variables of earlier clauses are applied after the join, keyed by a branch marker, because SPARQL evaluates sub-patterns on their own.
- Variables that may be null (from `OPTIONAL MATCH`) are renamed and tied back with an equality, and property reads on them are guarded, so an unbound variable never joins with everything.
- Relationship uniqueness is pairwise inequality of the relationship triples (and reifiers when named). Variable-length patterns expand into one branch per length with trail filters; an unbounded directed pattern without variables is reachability through a property path (a recursive CTE), which `explain()` notes.
- `shortestPath` / `allShortestPaths` bind their ends in SQL; the job then searches breadth-first, one SQL request per level for all sources at once, so it runs on D1.
- A pattern comprehension `[(a)-->(b) WHERE … | expr]` is its own SPARQL query: the pattern joined with the distinct values of the outer variables it reads. The job keys its rows by those values; nodes of an enclosing list comprehension key a per-row map instead.
- Plain `RETURN` items are evaluated in Rust ([[crates/oxilite-cypher/src/eval.rs]]) — multi-valued properties become lists — while ordering and paging stay in SQL; aggregating projections compile to `GROUP BY`, or run in Rust when they need `collect()`, several `min()`/`max()` or temporal ordering.
- Temporal values ([[crates/oxilite-cypher/src/temporal.rs]]) follow openCypher: construction from strings and maps, projection, truncation, arithmetic with durations, `duration.between`, and Java's ISO 8601 rendering; named zones use the IANA database bundled by `jiff`.

### Writes

A writing statement reads once, computes its changes in Rust, and applies them as one atomic request — one D1 batch; natively the read and the write share a transaction.

The tail evaluates `CREATE`, `MERGE`, `SET`, `REMOVE` and `DELETE` row by row against an in-memory view of the touched entities. Before writing, two SQL probes fetch what the changes depend on: the edges of deleted nodes, whether created relationships' triples exist, and every reifier of the affected triples. The job then decides reifiers (a single plain relationship stays a triple; parallel ones all get reifiers), removes a triple with its last reifier, and rejects `DELETE` of a node that still has relationships. On D1 another writer may change the data between the read and the batch; the batch itself is atomic.

### OWL and SHACL awareness

Reasoning options apply to Cypher as to SPARQL: labels match subclasses, relationship types subproperties and inverses ([[architecture#Reasoning]]). SHACL shapes stored in the dataset act as the property-graph schema ([[decisions#D17 SHACL shapes as the property-graph schema]]).

Shapes are read from the compiled shape index ([[architecture#Schema registry#Compiled shape index]]) — one indexed request per writing statement instead of a SPARQL evaluation, which on D1 is one round trip instead of a compiled query. `schema_query()` remains as the reference the index is checked against. Shapes loaded with `Store::cypher_schema` (and passed in `CypherOptions::schema`) make `sh:minCount 1` properties join without `OPTIONAL`, `sh:maxCount 1` properties scalar, and `sh:datatype` a value type the compiler uses (`QueryOptions::var_types`: one typed comparison instead of one per possible type). A writing statement checks every node it creates or changes against `sh:datatype`, `sh:minCount`, `sh:maxCount`, `sh:in` and `sh:pattern` of the shapes targeting its labels before sending the batch ([[crates/oxilite-cypher/src/schema.rs#Shapes]]). `CALL db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()` and `db.schema.nodeTypeProperties()` read shapes and data. Complete validation stays with rudof ([[architecture#Validation]]).

## Datalog frontend

Datalog over the same quads as SPARQL: RDF-native rules with stratified negation, checked before compilation and lowered to a single SQL statement. Crate `oxilite-datalog`, behind the `datalog` feature.

The dialect is spelled like Turtle and SPARQL: variables are `?x`, IRIs are `<...>` or CURIEs declared with `@prefix`, and literals carry `^^` datatypes and `@` language tags ([[crates/oxilite-datalog/src/lexer.rs]]). An atom names its relation in one of three ways ([[crates/oxilite-datalog/src/ast.rs#Pred]]): an IRI, applied to two arguments as a predicate, so `ex:parent(?x, ?y)` *is* the triple pattern `?x ex:parent ?y`, or to one as a class, so `ex:Person(?x)` is `?x rdf:type ex:Person`; a predicate defined by rules; or the built-in `triple/3` and `triple/4` forms, which reach the quad table directly. A predicate that any rule defines is derived, and is never also read from the store, so a name means one thing throughout a program. A program is a list of rules, with aggregates allowed in a head, and a goal.

### Stratification

The dependency graph, its strongly connected components, stratification, safety and the recursion shape of each component, all checked before any SQL exists. See [[crates/oxilite-datalog/src/program.rs#analyse]].

Edges are positive, negative or aggregating ([[crates/oxilite-datalog/src/program.rs#Dep]]). Tarjan's algorithm finds the components; a negative or aggregating edge inside a component makes the program unstratifiable and is rejected by naming the offending cycle, and the components are ordered topologically into strata. Each stratum carries the shape that decides how it is evaluated ([[crates/oxilite-datalog/src/program.rs#Shape]]): non-recursive, linear (one recursive body atom, one predicate in the component), linear-mutual (several predicates, one tagged common table expression) or non-linear (two or more recursive body atoms). Rules whose head or negated variables are not bound positively in the body are unsafe and rejected with the variable named. Because these checks run first, a bad program is reported as a Datalog error rather than as a SQLite one.

### SQL generation

A checked program compiles to one SQL statement over the term-id encoding, so a derived predicate composes with a triple pattern without any conversion. See [[crates/oxilite-datalog/src/sql.rs#compile]].

Every relation, stored or derived, is a set of rows of tagged 64-bit ids ([[architecture#Term encoding]]). A non-recursive predicate becomes a plain common table expression, a linear component becomes one `WITH RECURSIVE` member, and a mutually recursive component becomes one member carrying a discriminant column, one recursive term per rule ([[decisions#D26 Mutual recursion is one CTE with a discriminant]]). A non-linear component has no single-statement form, so it is iterated instead ([[architecture#Datalog frontend#Iteration]]). Constraints compile over the encoding: a comparison decodes an id through the typed side columns of `terms`, while term equality stays id equality, and a constant carries its own lexical form because the store need never have seen it. `union_default_graph` and `include_inferred` mean what they mean for SPARQL ([[architecture#SPARQL to SQL compiler]]). The result carries the output variables in column order, the constants the program mentions — so the term resolver needs no lookup for them — and notes for `explain()` ([[crates/oxilite-datalog/src/sql.rs#Compiled]]).

### Iteration

A component whose rules are non-linear is evaluated by rounds in `datalog_work`, because SQLite allows only one self-reference per recursive term. See [[crates/oxilite-datalog/src/sql.rs#Fixpoint]].

The compiler emits seed statements (the component's non-recursive rules) and step statements (its recursive ones), each carrying the common table expressions of the earlier strata it reads, since they run in their own requests. The job applies the seed, then repeats the step until the row count stops growing, then runs the goal. Rounds are naive rather than semi-naive: the work table's primary key covers every column, so `INSERT OR IGNORE` deduplicates and the count is monotone, which makes the fixpoint detectable without a delta relation — and a delta would cut re-derivation, not round trips, which are what dominate ([[decisions#D25 One statement where SQLite allows it, iteration where it does not]]). Rows are scoped by a run id and deleted when the evaluation ends, so concurrent programs do not see each other and nothing is left behind ([[decisions#D25a The work table is scoped by run, not by connection]]). `Options::max_iterations` bounds the rounds, and the result reports how many each component took.

### Execution

A compiled program runs as a sans-IO job: the statement, then one term lookup — two requests for any program that compiled to one statement, on every backend including D1. See [[crates/oxilite-datalog/src/exec.rs#DatalogJob]].

The job issues the compiled SQL, then asks the shared term resolver for the terms behind the ids it got back ([[architecture#Sans-IO core]]). A component that has to be iterated is driven first, adding one request per round and one to clean up. Results carry the goal's variables in column order, one row per solution with `None` for an unbound column, and the rounds each iterated component took ([[crates/oxilite-datalog/src/exec.rs#DatalogResult]]).

### Materialization

What a program derives can be stored instead of queried, in `quads_inf` — the table OWL 2 RL materialization already writes. See [[crates/oxilite-datalog/src/materialize.rs#MaterializeJob]].

A user rule is then visible to SPARQL and Cypher through the `include_inferred` option that already exists ([[architecture#Reasoning]]), and each run replaces only its own producer's conclusions ([[decisions#D28 Inferences are attributed to producers]]). Only rule heads with an RDF form are storable — an IRI with two arguments is a predicate, with one a class — so a head that is not a triple is rejected before anything is written rather than half-applied. A component needing iteration is evaluated once and read by every head, since they share a run.

### Reach

The dialect is available from Rust, the command line, WebAssembly and both JavaScript packages, everywhere behind an off-by-default feature.

`oxilite datalog` runs a program, `--explain` prints the strata and strategies, `--materialize` stores the conclusions, and the program comes from `--program`, `--file` or standard input ([[architecture#Command line and HTTP endpoint]]). The WebAssembly core exports `datalog`, `datalog_materialize` and `explain_datalog` under a `datalog` feature that is off by default, so a Worker that does not use rules does not carry the ~0.2 MB ([[architecture#Bindings]]). `@oxilite/common` holds the shared types and the term conversion, so `@oxilite/node` and `@oxilite/d1` return the same shapes; the option names match the Cypher ones for the settings they share.

## JSON-LD documents

JSON-LD documents are stored verbatim in a keyed table and converted to RDF in a named graph per document, so SPARQL queries them and the exact JSON comes back. Crates `oxilite-jsonld` (generic) and `oxilite-vc` (credentials).

Both crates are sans-IO like the core: operations are jobs ([[crates/oxilite-jsonld/src/jobs.rs#WriteJob]], [[crates/oxilite-jsonld/src/jobs.rs#CheckJob]], [[crates/oxilite-jsonld/src/jobs.rs#RebuildJob]]) that yield SQL requests. The umbrella crate drives them from `Store::jsonld()` / `Store::credentials()` and their async twins (features `jsonld`, `vc`) — handles over the store rather than methods on it, so `Store` stays a drop-in for Oxigraph's. The raw document is the source of truth and graphs are derived data ([[decisions#D18 The raw document is the source of truth]]).

### Keys and graphs

A document's key comes from a key strategy (top-level `@id`/`id` by default, a JSON Pointer, a content hash, or explicit); its default-graph triples go to a graph chosen by a graph strategy (the key itself by default).

See [[crates/oxilite-jsonld/src/options.rs#JsonLdOptions]] and [[decisions#D19 Credential id as key and named graph]]. Template graphs substitute the percent-encoded key into `{key}`; fixed and default-graph strategies make documents share a graph. When no key is found, the missing-key policy rejects the document (generic default) or falls back to `urn:oxilite:doc:sha256:<hex>` of the bytes (credentials default).

### Tables

Three tables are created when a JSON-LD handle is first opened, so stores that never use JSON-LD are unchanged and `schema_version` stays put.

- `jsonld_documents(key, graph, doc, sha256, profile, issuer, subject, types, valid_from, valid_until, refs, stored_at)` — the verbatim JSON plus metadata that profiles fill (credentials: issuer, first subject id, types, validity as epoch seconds, a presentation's embedded keys). Partial indexes on issuer, subject and `valid_until` are configurable ([[crates/oxilite-jsonld/src/options.rs#MetadataIndexes]]), since D1 bills every index entry.
- `jsonld_graphs(key, g)` — the graphs a document owns: its target graph when it has one of its own, plus the graphs it defines (proofs). `UNIQUE(g)`: a graph has one owner.
- `jsonld_contexts(iri, doc)` — persisted remote contexts.

`sha256` is hex TEXT and `types`/`refs` are JSON arrays, because the SQL value model has no BLOBs. See [[crates/oxilite-jsonld/src/schema.rs#schema_statements]].

### Context loading

Context loading never does I/O inside the JSON-LD processor: the loader chain answers from memory and records misses, which the job reads from `jsonld_contexts` and retries.

Order: contexts registered on the handle, the first loader (the bundled W3C contexts of `ssi-json-ld` for credentials, none otherwise), then persisted contexts read by the job in rounds (a context may import others; at most 8 rounds). A fetcher callback may download what is still missing — `http_fetcher()` with the `network` feature, native only — and `cache_fetched` persists the result. Because every loader answers immediately, `json-ld`'s futures complete on their first poll ([[crates/oxilite-jsonld/src/loader.rs]]).

### Conversion

The `json-ld` crate expands the document and converts it to RDF; the result is mapped to `oxrdf` quads with the default graph rewritten to the target graph.

Blank nodes are labelled `d<16 hex of xxh3-128(key)>_<n>` ([[crates/oxilite-jsonld/src/convert.rs#blank_prefix]]), so documents never share one and re-storing a document is a no-op ([[decisions#D21 Document-scoped deterministic blank nodes]]). Generalized RDF (blank predicates) is dropped. `rdfDirection: i18n-datatype` IRIs are normalized to JSON-LD 1.1's form, which `json-ld` 0.21 predates. On the W3C `toRdf` suite, 450 tests pass; 4 are allow-listed limitations of `json-ld` 0.21 (compound-literal direction, keyword-like IRIs, an invalid `@base`) and 13 are out of scope (1.0-only, generalized RDF, `expandContext`).

### Write path

A put, replace or remove is one atomic request — one D1 batch — built without reading back, except for strategies where documents share a graph.

The request deletes the quads and graph names of every graph the key owns, deletes its ownership rows, inserts terms, quads, graph names and ownership rows, and writes the document row (long documents are appended in chunks with `doc = doc || …` so each statement stays under the SQL-length limit). A request with more statements than the backend allows fails with `DocumentTooLarge` instead of being split. For fixed and default-graph strategies the previous version is read and its exact triples deleted, natively inside a transaction. `check_documents()` compares every document's graphs with a fresh conversion, and `rebuild_graph()` re-puts the stored bytes; SPARQL UPDATE may edit document graphs, and this is how drift is found and repaired.

### Verifiable Credentials

`oxilite-vc` stores VCDM 1.1 and 2.0 credentials and presentations under their `id`, in the named graph of the same IRI, after a structural check with `ssi-vc`; proofs are not verified.

[[crates/oxilite-vc/src/lib.rs#credential_input]] picks the data model from the first `@context`, parses with `ssi_vc::v1`/`v2` (context order, required types, issuer, subject) and extracts metadata (`issuanceDate`/`validFrom`, `expirationDate`/`validUntil`). JSON-LD `@graph` containers put each proof in its own blank-node graph, owned by the credential, so claims queried from the credential graph contain no `proofValue`. [[crates/oxilite-vc/src/lib.rs#presentation_inputs]] also stores each embedded credential with an id as its own document, in the same batch, and records their keys in the presentation's `refs`. Lookups by issuer, subject, type, validity instant and profile read the indexed columns ([[crates/oxilite-jsonld/src/jobs.rs#find_sql]]); type matching uses `instr` on the JSON array, so it needs no JSON1.

`examples/verifiable-credentials` runs the website's walkthrough on `@oxilite/node` and on Miniflare D1.

The `ssi` crates enable serde_json's `arbitrary_precision` for the whole build, which changes how numbers deserialize; `SqlValue` therefore has a hand-written deserializer that accepts both forms ([[decisions#D20 json-ld and ssi, in two crates]]).

## Bindings

A Node.js package and a Cloudflare D1 package, both typed TypeScript, over the same core.

`@oxilite/node` (napi-rs) wraps `blocking::Store` on rusqlite or a dlopen'ed library: `query`, `update`, `load`, `dump`, `add`/`delete`, `has`, `match`, `size`, `explain`, `optimize`, `backup`, returning RDF/JS-style term objects. `@oxilite/d1` runs the wasm core against a `D1Database` binding and exposes the same API asynchronously.

`@oxilite/node` loads `oxilite.<platform>-<arch>.node` (then a local `oxilite.node` build) and fails with build instructions when no binary matches; Releases up to 0.3.0 are published with the darwin-arm64 binary only.

Every published crate and npm package has its own README (absolute links and logo, so it renders on crates.io and npm) with install, examples, API and a shared table of the oxilite family; the website is https://oxilitedb.com, set as `homepage` everywhere, and each crate's `documentation` points to docs.rs.

Both packages also expose `cypher(query, params, options)` and `explainCypher()`: parameters and options travel as JSON ([[crates/oxilite-cypher/src/json.rs]]), and results are plain objects (`CypherNode`, `CypherRelationship`, `CypherPath`, temporal values as ISO strings) with `records` keyed by column. The wasm engine builds Cypher by default (feature `cypher`; `cypher-lite` leaves out the bundled time zone database). When the database is bundled (`tzdb-bundle`, on by default), native builds use it too instead of `/usr/share/zoneinfo`, since distributions differ on pre-1970 history (Ubuntu keeps `backzone`) and results must not depend on the host.

Both packages also expose JSON-LD documents and Verifiable Credentials: `store.jsonld(options)` and `store.credentials(options)` return handles (`put`, `get`, `remove`, `find`, `graphs`, `documentForGraph`, `putContext`, `check`, `rebuild`; `putPresentation` for credentials), synchronous on Node and asynchronous on D1. Options, filters and stored documents travel as JSON ([[crates/oxilite-jsonld/src/json.rs]]); errors arrive as `JsonLdError` with the JSON-LD error code. The wasm engine builds them by default (feature `vc`): the D1 bundle grows from 3.3 MB to 5.0 MB (1.4 MB gzipped), within the Workers limits. Network context loading exists only on Node.

Both packages share `@oxilite/common` (terms, `DataFactory`, result conversion). The native addon and the wasm engine exchange the same JSON terms and outputs (see [[crates/oxilite-core/src/json.rs]]), so a query returns identical JavaScript values on either. Oxigraph's own `store.test.ts` runs unchanged against `@oxilite/node`; its single failure is the allow-listed merge semantics of `default_graph` lists (D12). Example Workers exist in Rust (`examples/d1-worker`) and TypeScript (`examples/d1-worker-ts`), each with a Miniflare end-to-end test. `examples/do-agent-memory-ts` runs `@oxilite/d1` on a Durable Object's SQLite: a D1-shaped adapter over `ctx.storage.sql` maps `raw()` to `exec` and `batch()` to `transactionSync`, and reports per-statement changes from `total_changes()`. It is tested on Miniflare but is not part of the W3C runs.

## Project website

A static site in `site/` (landing page, articles, and German Impressum, Datenschutz and AGB) is published to GitHub Pages at oxilitedb.com by `.github/workflows/pages.yml` on pushes that touch `site/`.

The workflow uploads `site/` as a Pages artifact with `actions/upload-pages-artifact` and deploys it with `actions/deploy-pages`; there is no build step, so what is in the repository is what is served. The custom domain lives in the repository's Pages settings, not in a `CNAME` file. Pages serves `about.html` for `/about` and a directory's `index.html` for `/dir/`, redirecting `/dir` to `/dir/`, which is what the extensionless URLs in `site/sitemap.xml` rely on.

Cloudflare proxies the DNS, so its dashboard settings (Block AI bots, Bot Fight Mode) still apply, but no code of ours runs per request: `site/robots.txt` asks AI training crawlers to stay out and sets `Content-Signal: search=yes, ai-input=yes, ai-train=no`, and nothing enforces that at the origin. A Cloudflare Worker in `site-worker/` briefly replaced Pages and added per-request security headers and 403s for those crawlers, but its deploys never had an API token, so the live site stayed on Pages and the Worker was removed.

Articles live in `site/articles/`, one HTML file each, behind an index at `site/articles/index.html` reachable at `/articles`. Every page's menu carries an Articles entry pointing there, and the landing page keeps its own `#articles` section of the same cards, linking through to the index.

`site/articles/introducing-oxilite.html` is the overview article and the announcement's canonical URL: the gap Oxigraph leaves, the sans-IO core, term encoding, the schema, the single-statement compiler and its fallback, the planner, atomicity, reasoning, validation, the Cypher frontend, the JSON-LD and Verifiable Credentials layer, the BSBM numbers, the tested Oxigraph compatibility and the limits. It leads the articles index and the landing page's `#articles` section. Its claims are drawn from this file, [[decisions]] and the README rather than from a runnable example, so a change to any of those should be reflected in it.

The landing page's `#studio` section introduces oxilite studio, the VS Code extension built on [[architecture#Studio server]], with its Marketplace install and the build-from-source path for platforms without a bundled server. `site/articles/oxilite-studio.html` is its tour: install, conventions, completion, reasoning with "why?", live SHACL, the manifest and `oxilite check`, attached SQLite and D1 stores, notebooks, MCP, and the limits. Its examples come from the studio server's tests, so a change to those behaviours should be reflected in it. The studio logo is `site/assets/studio.svg` (rendered to `studio.png`). The article's screenshots in `site/assets/studio/` are taken from the extension running on the studio repository's `examples/demo` project, so the text and the images describe the same data.

The site is plain HTML and one stylesheet in a white, black and orange palette. It loads no external fonts, scripts or trackers, which keeps the Datenschutz page to the hosting logs of GitHub Pages and Cloudflare, and Cloudflare's bot-protection cookies. The logo (`site/assets/logo.svg`, rendered to `logo.png` with `rsvg-convert`) combines a SQLite-style tile, a quill drawn as a graph, and a small edge-worker cloud.

The Cypher article walks through the M7 frontend; its steps are asserted by `examples/cypher-property-graph` on `@oxilite/node` and on Miniflare D1. The JSON-LD query tour (`site/articles/querying-jsonld-credentials.html`) queries credentials and a JSON-LD issuer registry with SPARQL (provenance through `GRAPH`, joins, aggregates, typed validity dates, RDFS reasoning, FTS5), metadata lookups and Cypher over the union default graph; every query is asserted by `examples/jsonld-queries` on both packages. It explains that proofs are verified outside oxilite, on the stored JSON.
