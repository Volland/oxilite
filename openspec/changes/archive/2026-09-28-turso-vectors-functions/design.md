## Context

oxilite never talks to a database: every operation is a `Job` that yields SQL `Request`s and consumes
`Response`s ([[architecture#Sans-IO core]]). A backend is "run these statements, atomically if
asked". Turso is a Rust reimplementation of SQLite with the same SQL, so it fits behind that
contract. What it adds is what the other engines lack: vector types, vector distances, and an
index-method framework.

A feasibility probe drove the real `Store` over Turso before anything was designed:

| Turso | Result |
|---|---|
| 0.7.2 (stable) | BGP joins, FILTER, OPTIONAL, UNION/MINUS, aggregates, updates: pass. **Recursive CTEs: "not yet supported"**, so property paths, registry scoping (`optimize`, `DROP GRAPH`) and Datalog recursion fail. |
| 0.8.0-pre.14 | Everything above passes, including recursive CTEs. |
| both | `WITHOUT ROWID, STRICT` is a syntax error; `CREATE INDEX` on a `WITHOUT ROWID` table is "not supported"; `fts5` is not a module. |
| both | `vector32/64/8/1bit`, `vector32_sparse`, `vector_distance_cos/l2/dot/jaccard`, `vector_extract`, triggers with `RAISE`, `json_array_length`, `sqlite_master`, savepoints, CTEs inside derived tables, correlated subqueries: all work. |
| both | Index methods: only `toy_vector_sparse_ivf` (sparse vectors, Jaccard). No dense ANN index yet. |

## Goals / Non-Goals

Goals:

- oxilite on Turso with the full SPARQL, Cypher and Datalog surface that the other native backends
  have, except the FTS5 text index.
- Vector indexes defined once and queried from all three languages with the same semantics, in the
  same single statement as the surrounding graph pattern.
- Index lifecycle reachable from every write surface oxilite has: the Rust API, SPARQL Update,
  Cypher DDL, the shell and the studio.
- User functions written in the host language, callable from all three languages, without giving up
  SQL compilation for the parts of a query that do not call them.

Non-goals: a dense ANN index (Turso has none), vector search on backends without vector functions,
embedding generation (the application or a host function computes embeddings), running host
functions inside SQL, and JavaScript bindings (a follow-up once the Rust API settles).

## Decisions

### D38 Turso is a backend, not a fork of the compiler

`oxilite-turso` implements `SyncBackend` and `AsyncBackend` and rewrites the two DDL forms Turso
rejects. The compiler does not branch on the engine. What differs is declared through
`Capabilities`.

The shim turns `… ) WITHOUT ROWID, STRICT` into `… ) STRICT`. The table becomes a rowid table whose
composite `PRIMARY KEY` is an automatic unique index, and the covering `posg`/`ospg`/`gspo` indexes
then build normally. Every access path the planner relies on survives. The cost is one more B-tree
per table (the rowid table itself), which matters for D1 billing but not in-process. Statements
creating the FTS5 index fail with an `unsupported` error naming the text index, rather than Turso's
"no such module".

`turso` is pinned `=0.8.0-pre.14`: 0.7.2 has no recursive CTEs, and a prerelease moves under a
caret requirement. The backend runs Turso's async API to completion with a minimal executor
(`futures::executor::block_on`) for the sync store; Turso's local IO needs no runtime. Blobs, which
oxilite never selects except vector columns, come back as lowercase hex text.

Rejected: routing Turso through the dylib backend (Turso's C ABI is not the SQLite C API that
backend binds), and a compile-time `engine` switch in the compiler (the SQL is the same; only
capabilities differ).

### D39 A vector index is RDF in `<oxilite:vectors>`, realised as a trigger-maintained table

The definition is data in a system graph, like the schema registry
([[decisions#D34 The schema registry is RDF in a system graph]]). The physical structure is a
cache, rebuilt from that data.

```turtle
<oxilite:vector/docs> a oxl:VectorIndex ;
    oxl:indexName "docs" ;
    oxl:property ex:embedding ;          # literal values "[0.1, 0.2, …]"
    oxl:dimensions 384 ;
    oxl:metric oxl:Cosine ;              # oxl:Cosine | oxl:Euclidean | oxl:DotProduct | oxl:Jaccard
    oxl:elementType oxl:Float32 ;        # oxl:Float32 | oxl:Float64 | oxl:Int8 | oxl:Bit1 | oxl:SparseFloat32
    oxl:class ex:Document .              # optional: only nodes of this type are candidates
```

The graph is `<oxilite:vectors>`. The index IRI `<oxilite:vector/NAME>` is also the `SERVICE` IRI
that queries it. Definitions are therefore queryable and versioned like any data, loadable from a
dump, and visible to all three languages (`GRAPH <oxilite:vectors> { ?i a oxl:VectorIndex }`).

Each index is a table plus two triggers:

```sql
CREATE TABLE vec_docs (s INTEGER NOT NULL, o INTEGER NOT NULL, g INTEGER NOT NULL,
                       e BLOB NOT NULL, PRIMARY KEY (s, o, g));
CREATE TRIGGER vec_docs_ins AFTER INSERT ON quads WHEN NEW.p = <id(ex:embedding)> BEGIN
  SELECT RAISE(ABORT, 'oxilite: vector index docs expects 384 dimensions')
   WHERE COALESCE(json_array_length((SELECT lex FROM terms WHERE id = NEW.o)), -1) <> 384;
  INSERT OR IGNORE INTO vec_docs(s, o, g, e)
    SELECT NEW.s, NEW.o, NEW.g, vector32(lex) FROM terms WHERE id = NEW.o;
END;
CREATE TRIGGER vec_docs_del AFTER DELETE ON quads WHEN OLD.p = <id> BEGIN
  DELETE FROM vec_docs WHERE s = OLD.s AND o = OLD.o AND g = OLD.g;
END;
```

Triggers keep the index exact inside the same transaction as the write, on every write path
(`INSERT DATA`, `DELETE/INSERT … WHERE`, the bulk loader, Cypher `SET`, `CLEAR`), with no extra
code in the writer. This is how `terms_fts` is kept current ([[architecture#Text search]]).
Creation back-fills from `quads` in the same atomic request. A sparse index also gets
`CREATE INDEX vec_docs_ann ON vec_docs USING toy_vector_sparse_ivf (e)`.

Validation is strict: a literal that is not a JSON array of numbers, or has the wrong length,
aborts the write, and creating an index over existing bad data fails and changes nothing. Names
match `[A-Za-z][A-Za-z0-9_]{0,63}`, compared case-insensitively because SQLite table names are.

Writes to `<oxilite:vectors>` through SPARQL Update are detected (as `update_touches_registry`
detects the schema graph). After the update the store runs a **sync**: it reads the definitions and
the `vec_%` tables, drops what is no longer defined (or defined differently), and creates what is
missing. Sync also runs after `create_vector_index`/`drop_vector_index`, and at open for a writable
store whose definitions and tables disagree (a restored dump). The Rust API writes the RDF and
builds the table in one atomic request, so its result is never half-applied.

Rejected: one generic `vectors` table for all indexes (definitions become rows, so SPARQL writes
could maintain it without DDL). The sparse ANN index is per table and matches only
`SELECT … FROM {table} ORDER BY distance LIMIT ?`, so indexes cannot share a table. Also rejected:
a definitions table as the source of truth, because it would be invisible to queries and dumps.

### D40 One k-NN statement, three languages

`vector::knn_sql(index, query, k)` is the only place a nearest-neighbour search is written. It
yields rows `(s, d)` in distance order:

```sql
SELECT s, MIN(d) AS d FROM (
  SELECT s, vector_distance_cos(e, vector32('[…]')) AS d FROM vec_docs
  [WHERE EXISTS (SELECT 1 FROM quads c WHERE c.s = vec_docs.s AND c.p = <rdf:type> AND c.o = <class>)]
) GROUP BY s ORDER BY d, s LIMIT k
```

A node with embeddings in several graphs counts once, at its nearest. For a sparse index the inner
select takes the index-method's pattern shape so Turso can serve it from the IVF index. The query
vector is a JSON array literal, or the IRI of a node whose stored embedding is used
(`(SELECT e FROM vec_docs WHERE s = <id> LIMIT 1)`, "more like this").

- **SPARQL**: `SERVICE <oxilite:vector/NAME> { … }` compiles to that statement as a derived table in
  the query's FROM. `oxl:node` binds the term id; `oxl:distance` binds an `xsd:double`; `oxl:score`
  binds the similarity (below). It then joins, filters and orders like any other pattern, and the
  whole query stays one statement. The partial evaluator treats it as compilable, so a query that
  also calls a host function still runs its k-NN in SQL.
- **Cypher**: `CALL db.index.vector.queryNodes(name, k, vector) YIELD node, score` is lowered like a
  `MATCH` (not like the schema procedures, whose rows are names). `node` becomes a node binding
  and `score` a value binding, both in the SQL stage, so `MATCH (node)-[:CITES]->(x) WHERE score >
  0.8 RETURN node.title` is one statement. `vector` is a list literal or a parameter; a string node
  id queries by node.
- **Datalog**: `nearest("NAME", query, k, ?node, ?rank)` is a built-in relation compiled to the same
  statement with a rank column. A Datalog column is a term id; an inline integer rank has one, while
  a computed double distance would not resolve, so Datalog gets rank and SPARQL and Cypher get
  distance. The query is a string literal (vector) or an IRI (node). `nearest/4` drops the rank.
  As with the history relations, a program that defines its own `nearest` rules keeps them.

Score follows Neo4j so Cypher users get what they expect: cosine `1 − d/2` (in [0, 1]), Euclidean
`1 / (1 + d²)`, dot product `−d` (Turso's dot distance is the negated product), Jaccard `1 − d`.

### D41 Host functions run in Rust, and only the calls leave SQL

A host function is `Arc<dyn Fn(&[Term]) -> Option<Term> + Send + Sync>`. That is spareval's
custom-function signature: `None` is an evaluation error, which SPARQL turns into an unbound value.
It is registered on a store under an IRI, plus an optional Cypher name. The registry is per store
handle and not persisted, because it is code, not data.

- **SPARQL**: the compiler reports a call to an unknown custom function as unsupported. The store
  then takes its partial-evaluation path ([[architecture#SPARQL to SQL compiler#Fallback
  evaluator]]): maximal compilable subtrees still run as SQL, and only the operators that call host
  functions run in spareval, which the store gives every registered function. Updates reach them
  through `delete_insert`.
- **Cypher**: a call whose name is a registered Cypher name (or an IRI local name) lowers to
  `Function::Custom(iri)`, so it rides the SPARQL path above. When the call is in a clause evaluated
  in Rust (lists, `collect()`, writes), the Rust evaluator calls it directly, converting Cypher
  values to RDF terms and back.
- **Datalog**: rules compile to SQL, and Turso cannot call Rust from SQL (scalar-function
  registration is crate-private). A rule that calls a host function, in an expression
  (`?s = ex:slugify(?n)`, `ex:score(?x) > 0.5`) or as an atom (`ex:slugify(?n, ?s)`, where the last
  argument is the result, and `ex:isValid(?x)` as a filter), is split. Its host-free part becomes
  an auxiliary rule that runs as SQL; the calls are applied to its rows in Rust; and the resulting
  head tuples are injected into the program as facts, so later strata, negation and aggregation see
  them. Host rules are processed in dependency order, and a host rule that depends on itself is
  rejected. A goal's host constraints filter its rows in Rust. Each host rule costs one extra
  request.

Rejected: pushing host functions into SQL as UDFs. That works on rusqlite and not on Turso or D1,
and it would give the same function two different evaluation paths.

## API design

### Rust

```rust
use oxilite::store::Store;
use oxilite::vector::{VectorIndex, Metric, ElementType, VectorQuery};
use oxilite::functions::HostFunction;

let store = Store::open_turso("kg.db")?;            // or Store::new_turso() in memory

store.create_vector_index(
    &VectorIndex::new("docs", ex("embedding"), 384)
        .metric(Metric::Cosine)
        .element_type(ElementType::Float32)
        .class(ex("Document")),
)?;
let hits = store.vector_search("docs", &VectorQuery::vector(&embedding), 10)?;  // Vec<VectorHit { node, distance, score }>
let more = store.vector_search("docs", &VectorQuery::node(ex("doc42")), 10)?;
for info in store.vector_indexes()? { println!("{} {} rows", info.index.name, info.rows); }
store.drop_vector_index("docs")?;

store.register_function(
    HostFunction::new("http://example.com/fn#slugify", |args| {
        let Term::Literal(l) = args.first()? else { return None };
        Some(Literal::new_simple_literal(slug(l.value())).into())
    })
    .cypher_name("ex.slugify")
    .arity(1, 1)
    .description("URL-safe slug of a string"),
)?;
```

`TursoBackend::open(path)`, `::memory()` and `::from_connection(conn)` are available for
`Store::with_backend` and `AsyncStore::new` too.

### SPARQL

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
PREFIX ex:  <http://example.com/>
PREFIX fn:  <http://example.com/fn#>
SELECT ?doc ?title ?distance (fn:slugify(?title) AS ?slug) WHERE {
  SERVICE <oxilite:vector/docs> {
    [] oxl:query "[0.12, 0.08, …]" ; oxl:k 10 ; oxl:node ?doc ; oxl:distance ?distance .
  }
  ?doc ex:title ?title ; ex:year ?y FILTER(?y >= 2020)
} ORDER BY ?distance
```

Creating an index with an update:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
INSERT DATA { GRAPH <oxilite:vectors> {
  <oxilite:vector/docs> a oxl:VectorIndex ; oxl:indexName "docs" ;
    oxl:property <http://example.com/embedding> ; oxl:dimensions 384 ; oxl:metric oxl:Cosine .
} }
```

Dropping it is `DELETE WHERE { GRAPH <oxilite:vectors> { <oxilite:vector/docs> ?p ?o } }`.

### Cypher

```cypher
CREATE VECTOR INDEX docs IF NOT EXISTS FOR (d:Document) ON (d.embedding)
OPTIONS { indexConfig: { `vector.dimensions`: 384, `vector.similarity_function`: 'cosine' } };

CALL db.index.vector.queryNodes('docs', 10, $q) YIELD node, score
MATCH (node)-[:AUTHORED_BY]->(a:Person)
WHERE score > 0.8
RETURN node.title, a.name, ex.slugify(node.title) AS slug, score
ORDER BY score DESC;

SHOW VECTOR INDEXES;
DROP INDEX docs IF EXISTS;
```

`vector.similarity_function` takes `cosine`, `euclidean`, `dot` or `jaccard`. oxilite also reads
`vector.element_type` (`float32`, `float64`, `int8`, `bit1`, `sparse`).

### Datalog

```prolog
@prefix ex: <http://example.com/> .
@prefix fn: <http://example.com/fn#> .

similar(?doc, ?rank) :- nearest("docs", "[0.12, 0.08, …]", 10, ?doc, ?rank).
recent(?doc, ?slug)  :- similar(?doc, ?r), ?r <= 5, ex:title(?doc, ?t), ?slug = fn:slugify(?t).
flagged(?doc)        :- recent(?doc, ?s), fn:isReserved(?s).

?- recent(?doc, ?slug).
```

### Shell

```text
oxilite --turso kg.db
oxilite> .vector create docs ex:embedding 384 cosine [float32] [--class ex:Document]
oxilite> .vector list
  name  property      dims  metric  element  class        rows  built
  docs  ex:embedding  384   cosine  float32  ex:Document  1204  yes
oxilite> .vector search docs [0.12, 0.08, …] 5
oxilite> .vector search docs <http://example.com/doc42> 5
oxilite> .vector drop docs
oxilite> .functions
```

## UI design (oxilite studio)

The studio (VS Code extension, separate repository) talks to `oxilite studio-server`. This change
adds the server requests; the panels below are the UI they are shaped for.

**Vector Indexes panel**, a tree view under the Store Explorer with one node per index. Its context
menu has *Search…*, *Show definition (Turtle)*, *Rebuild* and *Drop*; the title bar has
*New index…*.

```text
VECTOR INDEXES                                   [+] [⟳]
▾ docs        ex:embedding · 384d · cosine · 1 204 rows
▾ products    ex:imageVec  · 512d · euclidean · int8 · 88 310 rows
  tags        ex:tagBag    · 2048d · jaccard · sparse (IVF) · ⚠ not built
```

**New index…** is a quick-pick flow: name, then the property (completion from the store's
properties with literal values, showing the modal array length found as a dimension hint), then
metric, then element type, then an optional class. It sends `oxilite/vectorIndexCreate`, and a
failure (a wrong-dimension literal) is shown with the offending subject.

**Search…** opens a Vector Search webview: a query box (a JSON array, a node IRI, or text that the
workspace's embedding command turns into a vector), `k`, and a *Restrict to class* field. Results
come from `oxilite/vectorSearch` as a table (rank, node, distance, score, label) with a bar showing
score. Each row links to the node in the Store Explorer, and *Open as SPARQL* inserts the equivalent
`SERVICE <oxilite:vector/…>` query into a new `.rq` editor, so the UI teaches the query form.

**Language features**: completion offers index names inside `SERVICE <oxilite:vector/…>`, the
`oxl:query/k/node/distance/score` predicates inside it, index names in
`db.index.vector.queryNodes('…')` and `nearest("…")`, and registered host functions (from
`oxilite/functions`) with their descriptions as hover text. Explain output shows the k-NN derived
table.

### Studio server requests

| Request | Params | Result |
|---|---|---|
| `oxilite/vectorIndexes` | `{}` | `{ supported, indexes: [{ name, iri, property, dimensions, metric, elementType, class?, rows, built }] }` |
| `oxilite/vectorIndexCreate` | `{ name, property, dimensions, metric?, elementType?, class? }` | the created index, as in `vectorIndexes` |
| `oxilite/vectorIndexDrop` | `{ name }` | `{ dropped: bool }` |
| `oxilite/vectorSearch` | `{ index, vector? \| node?, k? }` | `{ hits: [{ node, distance, score }], elapsedMs }` (`node` as RDF/JS) |
| `oxilite/functions` | `{}` | `{ functions: [{ iri, cypherName, minArity, maxArity, description }] }` |

Every request takes the optional `connection` the other store requests take: the project store
(bundled SQLite) or an attached store. `oxilite/attach` gains `engine: "turso"`, which attaches a
Turso database file as a connection of kind `turso`, and that is where vector indexes live.
`supported` is false when the target's backend has no vector functions, so the panel can say why
it is empty ("attach a Turso store to use vector indexes").

## Risks / Trade-offs

- **Prerelease pin.** `=0.8.0-pre.14` builds today and every test runs on it; moving the pin is a
  one-line change guarded by the Turso test suite. The shim is two string rewrites and will shrink.
- **Dense search is exact.** An exact scan with a fast distance function suffices up to roughly
  1e5 vectors; beyond that a dense ANN index is needed, which Turso does not have yet. The
  `knn_sql` seam is where it plugs in: an index-method pattern like the sparse one.
- **Trigger cost on writes.** Only quads with an indexed predicate fire the index work (`WHEN
  NEW.p = …`). Every other write pays one integer comparison per trigger.
- **Datalog host rules cost a request each** and their results travel as facts inside the SQL
  text. Very large host-rule outputs grow the statement; the backend's statement limit applies.
- **Host functions are not persisted.** A store opened without registering them fails queries that
  call them, with an error naming the function.

## Migration Plan

Nothing migrates: existing stores and backends are unchanged. Stats load one extra statement only
when the backend has `vectors`. A Turso store is a SQLite-format file, but its tables are rowid
tables where the C engine's are `WITHOUT ROWID`. A store created on one engine should be dumped and
reloaded to move to the other.
