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
