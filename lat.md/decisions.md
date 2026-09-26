# Decisions

Architecture decisions agreed during the design interview (2026-09-22), each with its rationale and the alternatives rejected.

## D1 SQL executor trait instead of the SQLite C API

The core targets an abstract async SQL executor, not `libsqlite3`, because Cloudflare D1 offers no library to load — only a binding that runs SQL remotely.

Native backends (rusqlite, dlopen of a user path) and D1 all implement the same contract; see [[architecture#Sans-IO core]]. Rejected: reusing Oxigraph's `spareval` evaluator over per-pattern scans as the primary path — on D1 each scan is a network round-trip. Kept as a *fallback* for sync backends ([[architecture#SPARQL to SQL compiler#Fallback evaluator]]).

## D2 Tagged 64-bit hash ids with inline small values

Terms are identified by 4-bit-tagged 59-bit xxh3 hashes computed in Rust; canonical integers and booleans are inlined. Chosen over autoincrement ids.

Autoincrement ids need a lookup-or-insert round-trip per new term, which is prohibitive on D1. Hash ids make writes read-free and query constants compile-time integers. Collision probability is ~1e-9 at 190k terms; collisions are detected atomically by trigger. See [[architecture#Term encoding]].

## D3 Three covering permutations plus optional graph index

`quads` is `WITHOUT ROWID` with PK `spog` and indexes `posg`, `ospg`; `gspo` is optional. Chosen over Oxigraph's nine indexes and over vertical partitioning.

Every extra index multiplies D1 write billing. Vertical partitioning (a table per predicate) creates dynamic schema and needs a UNION over all tables for unbound predicates. See [[architecture#Storage schema]].

## D4 Own statistics-driven join ordering

oxilite orders triple patterns itself and forces the order with `CROSS JOIN`, because SQLite's planner cannot distinguish predicates in an N-way self-join.

Statistics are refreshed explicitly, not per write, to avoid doubling D1 writes. See [[architecture#Query planner]].

## D5 Atomic means one batch

The universal atomicity guarantee is "one request = one transaction = one D1 batch"; SPARQL UPDATE compiles to self-reading SQL that fits in it.

Interactive read-then-write closures exist only natively. Rejected: making Durable Objects SQLite the Cloudflare target — the user requires D1 to work. Durable Objects remain reachable through the same sans-IO driver (an example adapter, not a tested backend). See [[architecture#Updates and atomicity]].

## D6 Three-tier function strategy

SPARQL functions compile to SQL built-ins, then to rewrites, then to native UDFs; anything left falls back to Rust evaluation (sync backends) or reports unsupported (D1) with an `explain()` warning.

`NOW()` is bound once per query; `RAND()`/`UUID()` use SQLite's `random()`/`randomblob()`. `SERVICE` is out of scope for v1. See [[architecture#SPARQL to SQL compiler#Expressions]].

## D7 Reasoning by rewriting over a materialized TBox

Default reasoning rewrites queries against a small TBox closure table; OWL 2 RL materialization is explicit, opt-in and rebuilt from scratch.

Materialization multiplies billed writes on D1; rewriting keeps writes flat and results always fresh. See [[architecture#Reasoning]].

## D8 rudof engines unchanged through srdf traits

Validation reuses rudof's SHACL and ShEx engines by implementing its `srdf` traits on the store, rather than compiling shapes to SQL.

This gives full rudof coverage (SHACL Core, SHACL-SPARQL, ShEx). Its traits are synchronous, so D1 uses a prefetch-into-memory adapter. Aligning on the Oxigraph 0.5 crate family (which `srdf` already uses) removes type conversions. See [[architecture#Validation]].

## D9 Oxigraph-mirroring API, async first

The async API mirrors Oxigraph's `Store` names and types exactly; a `blocking::Store` is a sync drop-in replacement for `oxigraph::store::Store` on native backends.

Results reuse `spareval::QueryResults` and `sparesults` serializers unchanged. RocksDB-specific methods are replaced: `optimize()` for `compact`, `VACUUM INTO` for `backup`.

## D10 Milestones measured by W3C suites

Delivery is split into six milestones, each measured by the W3C test suites Oxigraph runs, plus a differential harness comparing results with in-memory Oxigraph. See [[milestones]].

## D11 Total order for incomparable literals

ORDER BY uses a total SQL order: unbound, blank nodes, IRIs, literals, triple terms; numeric literals first by value, then other literals by lexical form, datatype and language.

SPARQL leaves the relative order of incomparable literals undefined. Oxigraph sorts with a non-transitive comparator, so its output for mixed types depends on its sort algorithm and cannot be reproduced by SQL keys. The divergence is allow-listed in the compatibility harness (`order_terms`).

## D12 Set semantics for merged default graphs

When a query's default graph is the merge of several graphs (several `FROM` clauses, or the union-default-graph option), a triple present in more than one of them matches once.

This is the RDF merge the SPARQL specification requires. Oxigraph concatenates the graphs and can return duplicate solutions; the divergence is allow-listed in the compatibility harness (`corpus:from-two`).

## D13 Cypher as a second frontend over the RDF store

Cypher runs over the same quads as SPARQL, through the same compiler, planner and backends, instead of a separate engine with a native property-graph layout. Delivered in M7.

OWL and SHACL are defined over RDF, so they apply to property graphs only when those graphs are stored as RDF. A native layout (`nodes` and `edges` tables with JSON properties) would make relationship properties cheaper, but it would duplicate four backends and the planner, lose SPARQL interop, and need an invented OWL/SHACL mapping. See [[architecture#Property graph frontend]].

## D14 Relationships as asserted triples, with reifiers only when needed

A relationship `(a)-[:T]->(b)` is always the triple `a :T b`, so a hop is one join. An RDF 1.2 reifier is added only for relationship properties or parallel relationships.

When a second relationship of a triple is created, the existing one gets a reifier too, so all parallel relationships are reifiers of one triple term. The triple is deleted with its last reifier. Modelling every relationship as an intermediate node was rejected: it costs two joins per hop and triples write billing on D1. Named relationship patterns look their reifier up optionally; with `reifier_uniqueness`, uniqueness checks also tell parallel relationships apart.

## D15 Fresh node IRIs from Rust

Nodes created by Cypher get fresh IRIs (`urn:oxilite:node:<random>-<n>`) minted in Rust, and an `rdf:type rdfs:Resource` marker triple. This replaces the planned inline GeneratedNode id tag.

Writes are computed in Rust from the rows of the read ([[decisions#D16 Lowering to SPARQL algebra, with a Rust tail]]), so new terms are hashed as usual and no id needs to be generated inside SQL. The marker keeps nodes without labels, properties or relationships in existence (an RDF resource exists only through its triples); it is optional (`node_marker`) and hidden from `labels()`.

## D16 Lowering to SPARQL algebra, with a Rust tail

Cypher reads are lowered to `spargebra` algebra for the unchanged SPARQL compiler; what SQL cannot express — writes, lists, maps, `collect()`, temporal arithmetic — runs in Rust over the rows. This replaces the planned `Op` algebra.

Lowering to the existing algebra reuses the planner, the reasoning rewrites and the spareval fallback without touching the M2 compiler. Property-graph operations map onto SPARQL 1.2: uniqueness is inequality filters, variable-length patterns are unions of fixed-length branches or property paths, relationship identity is an optional reifier. Shortest paths are a breadth-first step machine. A writing statement is one read followed by one atomic write request: on D1 it is not isolated from concurrent writers between the two. A shared internal algebra remains possible if a feature needs it.

## D17 SHACL shapes as the property-graph schema

The dataset's SHACL shapes are the Cypher schema: mandatory properties join without `OPTIONAL`, single-valued ones read as scalars, datatypes type comparisons, and writes are checked before the batch is sent.

The checks run in Rust on the final state of every node the statement touches (datatype, cardinality, `sh:in`, `sh:pattern`), so an invalid statement sends nothing. Shapes are read once per writing statement, or once per store with `cypher_schema()`. Complete SHACL and ShEx remain rudof's job ([[decisions#D8 rudof engines unchanged through srdf traits]]).

## D18 The raw document is the source of truth

A JSON-LD document is kept byte for byte; its RDF is derived data that can be checked against and rebuilt from it.

Credentials are presented, re-signed and audited as JSON, so the stored bytes must come back unchanged — re-serializing from RDF would lose member order, formatting and anything JSON-LD drops. Graphs stay ordinary RDF that SPARQL may update; instead of triggers on `quads` (a write cost on every quad, billed on D1), `check_documents()` detects drift and `rebuild_graph()` repairs it.

## D19 Credential id as key and named graph

By default a document is keyed by its `@id`/`id` and its triples go to the named graph of that IRI; both are configurable.

One graph per credential makes "which credential says this" a `GRAPH ?g` pattern, lets replace and remove clear exactly the credential's triples without reading them, and matches how credentials are referenced. Credentials without an `id` (optional in VCDM 2.0) get a content-hash key by default, so storing one twice keeps one copy. Proofs, which JSON-LD puts in `@graph` containers, stay in their own graphs owned by the credential rather than being merged into the claims.

## D20 json-ld and ssi, in two crates

JSON-LD processing uses the `json-ld` crate; the credentials profile uses `ssi-vc` and `ssi-json-ld`, in a separate crate so generic JSON-LD users do not pay for it.

`json-ld` implements JSON-LD 1.1 with pluggable loaders (Oxigraph's `oxjsonld` streams RDF but has no expansion API or loader hook), and `ssi` builds on it, so both crates share one JSON-LD stack. `ssi-vc` models VCDM 1.1 and 2.0 and `ssi-json-ld` bundles the W3C contexts, which makes credentials convert offline, on D1 too. The cost: `ssi-vc` pulls in about 430 crates, `reqwest`, and serde_json's `arbitrary_precision` feature, which is unified across a build; oxilite's `SqlValue` deserializer accepts that number form, and the workspace test suite runs with it enabled.

## D21 Document-scoped deterministic blank nodes

Blank nodes of a document are labelled from a hash of its key and their position in `json-ld`'s relabelling, never from a global counter.

Documents therefore never share blank nodes (two credentials' anonymous nodes stay distinct), and re-storing a document produces the same ids, so a repeated put is idempotent. A different relabelling order in a future `json-ld` release would only change labels on the next put of each document; a test pins the labels of a reference credential.

## D22 Schema graphs registered, not separated

A graph that holds an ontology or SHACL shapes is registered by role; its triples stay in `quads`, and derived caches are scoped by that registration. Where registrations live is [[decisions#D34 The schema registry is RDF in a system graph]].

Moving schema into its own tables would force a `UNION` into every pattern scan and would stop SPARQL reading shapes, which are RDF people query; `GRAPH ?g` already separates them. Registering instead makes drop and replace one `DELETE FROM quads WHERE g = ?` with no read, as `jsonld_graphs` does for documents ([[decisions#D18 The raw document is the source of truth]]). An empty registry means "every graph", so the feature is purely additive. See [[architecture#Schema registry]].

## D23 Datalog as a third frontend, sharing the store

A Datalog program is parsed, checked and compiled to SQL over the same quads and the same term encoding as SPARQL and Cypher, rather than evaluated by an engine of its own.

This is [[decisions#D13 Cypher as a second frontend over the RDF store]] applied a third time, and it is what makes rules affordable: the storage schema, the encoding, the backends and the sans-IO protocol are all reused. A native engine — the shape Soufflé, Nemo and Cozo take — would duplicate four backends and, decisively, would have to read the graph out of the store to evaluate it, which on D1 is the network. Rules must run where the data is. See [[architecture#Datalog frontend]].

## D24 SQLite's recursive-CTE restrictions are the safety conditions

Stratification and linearity are enforced because SQLite enforces them, so the compiler reports them as Datalog diagnostics instead of working around them.

`WITH RECURSIVE` allows exactly one reference to the CTE in the recursive term's `FROM` and nowhere else — not in a subquery, not under `NOT EXISTS` — and no aggregate in the recursive term. Read as a Datalog specification those are the classical safety conditions: one self-reference means rules must be linear, no self-reference under `NOT EXISTS` means negation must be stratified, no aggregate means aggregation must be stratified. A violating program is rejected by [[crates/oxilite-datalog/src/program.rs#analyse]] with the cycle named, before any SQL exists. Rejected: emitting the SQL and letting SQLite fail at execution, which would surface an implementation detail as a user error.

## D25 One statement where SQLite allows it, iteration where it does not

A recursive component becomes a `WITH RECURSIVE` member of the same statement; a component whose rules are non-linear, which SQLite cannot express, is iterated to a fixpoint in a work table instead, one request per round.

`compiler/ops.rs` already inlines property-path CTEs this way, so the in-statement shape is proven on every backend. The recursive term uses `UNION`, not `UNION ALL`: deduplication is both Datalog's set semantics and what makes cyclic data terminate, which [[architecture#Reasoning]]'s transitive closure already relies on. Iteration is the fallback rather than the default because each round is a network hop on D1, the cost this project exists to avoid ([[decisions#D1 SQL executor trait instead of the SQLite C API]]); everything expressible stays in one statement and `explain()` says when it could not. The rounds are naive rather than semi-naive: the work table's primary key covers every column, so `INSERT OR IGNORE` deduplicates and the row count is monotone, making the fixpoint detectable with a count instead of a delta relation. A delta would cut re-derivation but not round trips, which dominate here. See [[architecture#Datalog frontend#Iteration]].

## D25a The work table is scoped by run, not by connection

Iteration stages its rows in the persistent `datalog_work` table, keyed by a run id, rather than in a `TEMP` table.

A fixpoint spans several requests, and on D1 those are not one connection, so a temporary table would be gone by the second round. A persistent table with a `run` column survives, keeps concurrent evaluations apart, and makes cleanup exact: an evaluation deletes its own rows and nobody else's. The cost is that a program with a non-linear component needs write access, which is stated rather than discovered at runtime. This is the same staging pattern `update_buffer` uses for SPARQL UPDATE ([[architecture#Updates and atomicity]]).

## D26 Mutual recursion is one CTE with a discriminant

A component defining several predicates compiles to a single recursive member carrying a `tag` column, with one recursive term per rule, each referencing the member exactly once.

SQLite has no mutually recursive CTEs, but since 3.34.0 the recursive term may be a compound, and each arm may hold its own single self-reference — so tagging keeps mutual recursion on the one-round-trip path instead of demoting it. Because that version is not universal and D1's is not ours to assume, backends declare `compound_recursive_cte` ([[architecture#Backends]]); when it is false the program is refused rather than given SQL that would fail.

## D27 Derived facts live in `quads_inf`

Materializing a rule program writes into the inference table OWL 2 RL materialization already uses, instead of a table of its own.

SPARQL and Cypher already read that table through `include_inferred`, so a user rule becomes visible to every dialect the moment it is materialized, with no new schema, no new option and no new cache to invalidate — rules extend the reasoner rather than forming a second inference universe ([[architecture#Reasoning]]). The shared lifecycle this first implied (running either materialization replaced the whole inferred set) proved painful for the studio, which runs OWL 2 RL and several rule files side by side; [[decisions#D28 Inferences are attributed to producers]] resolves it.

## D28 Inferences are attributed to producers

A side table `quads_inf_src(src, s, p, o, g)` records which producer derived each inferred quad (`owl2rl`, or a Datalog `Options::producer` name); `quads_inf` itself is unchanged.

Materializing resets only its own producer: its attributions are deleted, then every inferred quad no producer still claims. After writing, it claims every unclaimed quad. Readers and the fixpoint loop are untouched, so queries, Cypher and Datalog read `quads_inf` exactly as before, and a quad two producers can derive stays until neither does. Rejected: a producer column in `quads_inf`'s key, which would duplicate rows every reader would then have to de-duplicate. The cost is one extra row per inference, written once per run; on D1 that is billed. See [[architecture#Reasoning]].

## D29 The change log is the history, the quad table stays the present

A versioned store keeps `quads` exactly as before and records changes beside it in an append-only `quad_log`, written by triggers on `quads`; past states are read from the log.

Queries on the present — nearly all of them — keep their plans, their SQL and their cost, and every writer is captured because the capture sits below them. Rejected: reading every query from the log (every pattern pays a probe), and validity intervals on `quads` (a delete becomes an update, so rows are not immutable). The cost is billed on D1: one log row and one `tx` index entry per change ([[architecture#Versioning]]).

## D30 One tick per atomic write, opened by the store

The clock is `max(t) + 1` in `ticks`, inserted by the store in front of every request that writes `quads`, not by the writers and not from client clocks.

SQLite and D1 have one writer, so the tick is strictly monotonic without coordination, and a D1 batch is exactly one tick — one commit. Writers need no change: [[crates/oxilite-core/src/version.rs#prepare]] recognizes their statements. Rejected: a hybrid logical clock computed in Rust (no extra row, but not monotonic across clients) and a trigger-opened tick (SQLite has no transaction identity to key it on).

## D31 Genesis records the whole store

Raising a store to `log` copies every quad into the log as the genesis commit, rather than inferring the baseline from quads that have no log entry.

The inference is exact only while every change since genesis is logged; after a freeze and a resume it misattributes quads added in the gap. With a recorded genesis the log alone describes every state, freezes and resumes stay exact, and as-of is one query shape. The cost is one log row per quad, once, at the upgrade — free for a store created versioned.

## D32 Levels change only explicitly

Opening a store never changes its versioning level; raising or lowering it is a level change, applied by the API, the CLI or a D1 migration, and a downgrade deletes history only with `allow_loss`.

Other store options apply when a store is opened (`text_index` even back-fills). Versioning does not: starting history is billed on D1 and takes a snapshot, and deleting it cannot be undone, so neither may happen because a configuration file sets a flag. An empty store opened with a level gets it, which is how a store is created versioned.

## D33 Commits are ticks, and ticks record their terms

In the history's RDF view a commit is its tick as an `xsd:integer`, and a write tick writes its time, author and message as terms in its own batch.

Term ids are xxh3 hashes computed in Rust, so SQL cannot mint an id for a new IRI or literal; an inline integer is the one id SQL can compute, which makes the history graph and the Datalog history relations pure SQL over `ticks` and `quad_log` with no stored copy. The literals a commit is described with are known in Rust when the tick opens, so they are written then — one more statement per versioned batch and about two rows, never one per triple. Rejected: commit IRIs (no SQL-computable id), a materialized history graph (a write per change), and computing the terms at query time (a write during a read).

## D34 The schema registry is RDF in a system graph

Registrations are triples of `<oxilite:schema>` in the `oxl:` vocabulary, not rows of a table, and each may name the graphs it applies to with `oxl:appliesTo`.

A table is invisible to SPARQL, lost in an N-Quads dump, and only oxilite's SQL can maintain it. As RDF, a registry travels with the dataset, is registered and read by plain SPARQL that runs unchanged on Oxigraph, and can express which data each schema describes. Only the derived caches (`tbox_closure`, the shape index) stay SQL: they read the registry triples in place, so scoping costs no extra round trip. Supersedes the `schema_graphs` table of [[decisions#D22 Schema graphs registered, not separated]]; see [[architecture#Schema registry]].

## D35 Registry vocabulary 2: canonical, self-validating, import-aware

The registry writes canonical forms, ships SHACL shapes for itself, resolves `owl:imports` between registered ontologies, and every reader agrees on edge cases. Keeps the flat "graph IRI as subject" model of [[decisions#D34 The schema registry is RDF in a system graph]].

A review found the Rust reader, the SQL scopes and the SPARQL recipes disagreeing (a plain `"false"`, a graph with two roles), meaning carried by absence (no `appliesTo`, the fallback), a deactivated last ontology letting every graph back in, user-typed system graphs escaping the fallback, and imports recorded but useless. Version 2 writes `oxl:appliesTo oxl:AllGraphs` and an `xsd:boolean` flag, reads short forms leniently, keeps the fallback only while nothing is registered, fixes the system graphs in code, types `appliesTo` targets (`oxl:GraphTarget`), makes `oxl:SystemGraph` disjoint from schema graphs, and aligns with SD, Dublin Core, PROV and SPDX. `sh:shapesGraph` is derived by a recipe, not stored, so there is one source of truth.

Deferred, because they break stored data or IRIs: separate registration resources per role, a dereferenceable namespace and a registered IRI scheme for system graphs, and a per-graph shape index. See [[architecture#Schema registry]].
