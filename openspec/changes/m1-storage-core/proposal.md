## Why

Oxigraph is the reference Rust SPARQL database, but it needs RocksDB and a local filesystem. That rules it out on edge platforms like Cloudflare D1, where you can't install extensions or native storage engines but *can* run SQL on a provided SQLite. M1 builds the foundation for an Oxigraph-compatible store that uses SQLite as its only storage engine: a term encoding, a schema, the backend contract, and a first SPARQL compiler that turns basic graph patterns into single SQL statements.

## What Changes

- New Rust workspace with `oxilite-core` (sans-IO), `oxilite` (Store API), `oxilite-rusqlite` and `oxilite-dylib` backends.
- Tagged 64-bit term ids: hashed IRIs, blank nodes and literals (xxh3, 59-bit), plus inline canonical integers and booleans; atomic collision detection.
- SQLite schema: `quads` `WITHOUT ROWID` with PK spog, covering indexes posg and ospg, optional gspo; `terms` with typed side columns; `triple_terms`, `graphs`, statistics tables, update buffer, metadata.
- Sans-IO request/response contract with backend capabilities (SQL length, statements per request, UDFs, interactive transactions, 64-bit-as-text).
- Loading from and dumping to every `oxrdfio` format; `insert`, `remove`, `extend`, `contains`, `len`, `quads_for_pattern`, named-graph management.
- A SPARQL SELECT/ASK/CONSTRUCT/DESCRIBE compiler for BGPs, FILTER (tier-A functions), projection, DISTINCT, ORDER BY, LIMIT/OFFSET and GRAPH, producing one SQL statement per query plus at most one term-resolution round-trip.
- A statistics-driven greedy join-order planner that enforces its order with `CROSS JOIN`, plus `optimize()` to refresh statistics.
- A `blocking::Store` that is a drop-in for `oxigraph::store::Store`, and an async `Store<B>` with the same method names.
- A native backend that loads a user-supplied `libsqlite3` shared library from a path at runtime.

## Capabilities

### New Capabilities
- `term-encoding`: how RDF terms map to 64-bit ids, which values are inlined, and how collisions are handled.
- `quad-storage`: persisting, removing, scanning, loading and dumping quads and named graphs in SQLite.
- `sqlite-backends`: the backend contract, and the rusqlite and dynamic-library backends.
- `sparql-bgp-query`: evaluating SPARQL queries built from BGPs, filters, GRAPH and solution modifiers as single SQL statements.
- `query-planner`: statistics collection and join ordering.

### Modified Capabilities
<!-- none: this is the first change -->

## Impact

- New crates and dependencies: `oxrdf`, `oxrdfio`, `spargebra`, `spareval`, `sparesults`, `oxsdatatypes` (Oxigraph 0.5 family), `xxhash-rust`, `rusqlite` (bundled), `libloading`, `thiserror`.
- There's no existing code to migrate.
- The on-disk schema is versioned (`oxilite_meta.schema_version = 1`). Later milestones add tables, never incompatible changes, without a version bump and migration.
