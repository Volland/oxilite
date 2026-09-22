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
