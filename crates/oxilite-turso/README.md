# oxilite-turso

The [Turso](https://github.com/tursodatabase/turso) backend for [oxilite](https://oxilitedb.com): the Oxigraph-compatible RDF store, with SPARQL, Cypher and Datalog, on SQLite rewritten in Rust. It is the backend where **vector indexes** work.

```toml
[dependencies]
oxilite = { version = "0.7", features = ["turso", "cypher", "datalog"] }
```

```rust
use oxilite::model::NamedNode;
use oxilite::store::Store;
use oxilite::vector::{QueryVector, VectorIndex};

let store = Store::open_turso("kg.db")?;          // or Store::new_turso() in memory
store.update(r#"PREFIX ex: <http://example.com/>
    INSERT DATA { ex:a ex:embedding "[1, 0, 0]" . ex:b ex:embedding "[0.7, 0.7, 0]" }"#)?;

store.create_vector_index(&VectorIndex::new(
    "docs", NamedNode::new("http://example.com/embedding")?, 3,
))?;
let hits = store.vector_search("docs", &QueryVector::vector(&[1.0, 0.1, 0.0]), 5)?;
```

The same index from each query language:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
SELECT ?doc ?distance WHERE {
  SERVICE <oxilite:vector/docs> { [] oxl:query "[1, 0.1, 0]" ; oxl:k 5 ; oxl:node ?doc ; oxl:distance ?distance }
}
```

```cypher
CALL db.index.vector.queryNodes('docs', 5, [1, 0.1, 0]) YIELD node, score
RETURN node, score
```

```prolog
near(?d, ?rank) :- nearest("docs", "[1, 0.1, 0]", 5, ?d, ?rank).
?- near(?d, ?rank).
```

Indexes are defined as RDF in the system graph `<oxilite:vectors>`, so SPARQL Update and Cypher (`CREATE VECTOR INDEX … OPTIONS {indexConfig: {…}}`) create them too. Embeddings are literals (`"[0.1, 0.2, …]"`) of the indexed property, kept in the index by triggers on every write. Metrics: cosine, Euclidean, dot product, and Jaccard over sparse vectors (with Turso's inverted-file index); element types float32, float64, int8, 1-bit and sparse.

## Using the backend directly

```rust
use oxilite_turso::TursoBackend;
let store = oxilite::store::Store::with_backend(TursoBackend::open("kg.db")?)?;
```

`TursoBackend` implements both `SyncBackend` (for `Store`) and `AsyncBackend` (for `AsyncStore`).

## Differences from the bundled SQLite

- Turso does not index `WITHOUT ROWID` tables, so oxilite's clustered tables are created as rowid tables with a unique primary-key index: one more B-tree per table, same access paths.
- The FTS5 full-text index (`StoreOptions::text_index`) is not available.
- A store file created by one engine should be dumped and reloaded to move to the other.
- The crate pins `turso = "=0.8.0-pre.14"`, the first Turso release with recursive CTEs.

Dense vector search is an exact scan, which is exact and fast up to around 1e5 vectors; Turso has no dense ANN index yet.

License: MIT OR Apache-2.0.
