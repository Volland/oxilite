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

Interactive read-then-write closures exist only natively. Rejected: making Durable Objects SQLite the Cloudflare target — the user requires D1 to work. See [[architecture#Updates and atomicity]].

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

Cypher is lowered into the same compiler, planner and backends as SPARQL, over the same quads. It does not run on a separate engine with a native property-graph layout. Planned in M7.

OWL and SHACL are defined over RDF, so they apply to property graphs only when those graphs are stored as RDF. A native layout (`nodes` and `edges` tables with JSON properties) would make relationship properties cheaper, but it would duplicate four backends and the planner, lose SPARQL interop, and need an invented OWL/SHACL mapping. It is estimated at 30–40 person-weeks, against 22–29. See [[architecture#Property graph frontend]].

## D14 Relationships as asserted triples, with reifiers only when needed

A relationship `(a)-[:T]->(b)` is always the triple `a :T b`, so a hop is one join. An RDF 1.2 reifier is added only for relationship properties, parallel relationships or relationship identity.

Parallel relationships are several reifiers of one triple term. The asserted triple is deleted with its last reifier, checked in the same batch. Modelling every relationship as an intermediate node was rejected: it costs two joins per hop and triples write billing on D1. Reaching a traversed triple's reifier needs an optional `triple_terms(s, p, o)` index, kept optional as in [[decisions#D3 Three covering permutations plus optional graph index]].

## D15 Inline generated node ids

Nodes created by Cypher get a new inline GeneratedNode tag (tag 8) whose random payload determines the IRI `urn:oxilite:n:<hex>`, so SQL can create fresh nodes per row.

D1 has no UDFs, so SQL cannot compute xxh3. An inline tag avoids both a `terms` row and a read-back, as inline integers do in [[decisions#D2 Tagged 64-bit hash ids with inline small values]]. The encoder maps every IRI of that form to the tag, so one IRI never has two ids.

## D16 Internal compiler algebra shared by SPARQL and Cypher

The compiler gets an internal `Op` algebra that both `spargebra` and Cypher lower into. It adds property-graph operations SPARQL lacks: relationship uniqueness, trail variable-length paths, shortest path, path values, lists and maps.

Today the compiler matches on `spargebra::GraphPattern` directly. Encoding property-graph operations as special `SERVICE` patterns was rejected as fragile. The refactor lands first, alone, and is gated on the W3C suites and the differential corpus.

## D17 SHACL shapes as the property-graph schema

Registered SHACL shapes act as the Cypher schema. `maxCount 1` makes a property scalar, `minCount 1` allows inner joins and `datatype` gives static types. Simple constraints are checked inside the write batch.

Guards abort the whole D1 batch through an `oxilite_pg_guard` CHECK table, which is the same pattern `oxilite_guard` uses for SPARQL UPDATE. Complete SHACL and ShEx remain rudof's job ([[decisions#D8 rudof engines unchanged through srdf traits]]). The shapes also drive `db.labels()` and `db.schema()`.
