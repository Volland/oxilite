## Why

oxilite runs on three SQLite engines: the bundled C SQLite, a system `libsqlite3`, and Cloudflare D1.
All three can store a knowledge graph, and none can answer the question an AI application asks of it
first: *what is similar to this?* Embeddings end up in a separate vector database, and every query
that mixes "nearest to this vector" with "connected to that node" becomes two systems stitched
together in application code.

[Turso](https://github.com/tursodatabase/turso) is SQLite rewritten in Rust. It speaks SQLite's SQL
and file format, it is in-process and pure Rust (no C toolchain, and it builds for WebAssembly), and it
has vector types (`vector32`, `vector64`, `vector8`, `vector1bit`, `vector32_sparse`), vector
distances (`vector_distance_cos`, `_l2`, `_dot`, `_jaccard`), and an index-method framework with a
sparse inverted-file index (`toy_vector_sparse_ivf`). Because oxilite is sans-IO and only ever emits
SQL, Turso can be one more backend. Vector search can then compile into the same single SQL
statement as the graph pattern around it.

A second gap sits beside the first: the three query languages have fixed function libraries. A
user who needs a domain function (slugify an IRI, score a record, call a tokenizer) has no way to
reach it from SPARQL, Cypher or Datalog short of patching the engine.

## What Changes

- **New crate `oxilite-turso`**: a `SyncBackend` and `AsyncBackend` over the `turso` crate, with an
  in-memory and a file constructor. It carries a small SQL dialect shim: Turso does not index
  `WITHOUT ROWID` tables, so those tables become rowid tables whose composite primary key is a
  unique index. The FTS5 text index is reported as unsupported. The crate pins
  `turso = "=0.8.0-pre.14"`, the first release with recursive CTEs, which property paths, registry
  scoping and Datalog recursion need.
- **Capabilities** gain `vectors` (vector types and distance functions) and `vector_index_methods`
  (`CREATE INDEX … USING`), both false on every existing backend and true on Turso.
- **Vector indexes** (`oxilite_core::vector`): an index is a description in the system graph
  `<oxilite:vectors>` (index IRI `<oxilite:vector/NAME>`, `oxl:VectorIndex`) naming a property whose
  literal values are embeddings (`"[0.1, 0.2, …]"`), dimensions, metric, element type and an optional
  class. It is realised as a table `vec_NAME` kept current by triggers on `quads` (same batch, same
  transaction), back-filled on creation. A malformed embedding or a dimension mismatch aborts the
  write. Sparse indexes also get a `toy_vector_sparse_ivf` index.
- **Creating and dropping indexes**:
  - from Rust: `Store::create_vector_index`, `drop_vector_index`, `vector_indexes` and
    `sync_vector_indexes`;
  - from SPARQL Update: any write to `<oxilite:vectors>` re-syncs the physical indexes, the same way
    writes to `<oxilite:schema>` refresh the registry caches;
  - from Cypher: Neo4j 5 syntax, `CREATE VECTOR INDEX name [IF NOT EXISTS] FOR (n:Label) ON
    (n.prop) OPTIONS {indexConfig: {…}}`, `DROP INDEX name [IF EXISTS]` and `SHOW VECTOR INDEXES`;
  - from the shell (`.vector …`) and the studio server.
- **Vector search in three languages**, all compiling to the same k-NN SQL:
  - SPARQL: `SERVICE <oxilite:vector/NAME> { [] oxl:query "[…]" ; oxl:k 5 ; oxl:node ?n ;
    oxl:distance ?d ; oxl:score ?s }`, compiled into the query's single statement as a derived
    table.
  - Cypher: Neo4j's `CALL db.index.vector.queryNodes('NAME', k, $vector) YIELD node, score`.
    `node` is a real node variable that later clauses can match on.
  - Datalog: the built-in `nearest("NAME", query, k, ?node, ?rank)`, with a 4-ary form that has no
    rank.
- **Host functions** (`oxilite_core::functions`): Rust closures `Fn(&[Term]) -> Option<Term>`
  registered on a store under an IRI (and an optional Cypher name), callable as:
  - `ex:slugify(?x)` in SPARQL, via spareval custom functions and the partial evaluator, so the
    rest of the query still compiles to SQL;
  - `ex.slugify(x)` in Cypher, lowered to the same SPARQL call and also evaluated in the Rust tail;
  - Datalog, either as an expression `?s = ex:slugify(?x)` or as an atom `ex:slugify(?x, ?s)`, in
    rules and goals. A rule that calls a host function is evaluated in a Rust pass whose results
    feed the rest of the program as facts.
- **Studio server and shell**: `oxilite/vectorIndexes`, `oxilite/vectorSearch`,
  `oxilite/vectorIndexCreate`, `oxilite/vectorIndexDrop` and `oxilite/functions` requests on any
  connection; `oxilite/attach` with `engine: "turso"`; shell commands
  `.vector list|create|drop|search` and `.functions`; and `--turso` to open a store on Turso from
  every command.

## Capabilities

### New Capabilities
- `turso-backend`: oxilite on the Turso engine, sync and async, with its dialect shim and limits.
- `vector-index`: vector index definitions as RDF, their physical realisation, and their lifecycle.
- `vector-search`: k-nearest-neighbour search from SPARQL, Cypher and Datalog.
- `host-functions`: user functions in the host language, callable from SPARQL, Cypher and Datalog.

### Modified Capabilities
- `sqlite-backends`: capabilities `vectors` and `vector_index_methods`.
- `studio-server`: vector index and host function requests.
- `cli-shell`: the `.vector` commands and `--turso`.

## Impact

- New crate `crates/oxilite-turso`; new core modules `vector` and `functions`; the umbrella crate gets
  a `turso` feature and `vector_store.rs`.
- `Stats` gains the vector index definitions and which of them are built (loaded only when the
  backend has `vectors`, so the load request on existing backends is unchanged).
- No schema change for existing stores: vector tables exist only for defined indexes.
- The Turso dependency is a prerelease pinned exactly. When Turso ships 0.8.0 the pin moves; the
  shim is expected to shrink as Turso gains `WITHOUT ROWID` secondary indexes.
- Not in scope: dense ANN indexes (Turso has none yet; dense search is an exact scan, which is
  correct and fast to tens of thousands of vectors), vector search on D1 and the C SQLite backends,
  host functions pushed into SQL (Turso keeps scalar-function registration crate-private), and
  bindings for Node and D1.
