## Context

See proposal.md for the motivation. The same constraints as the rest of oxilite apply:
- the engine may be remote (D1 bills every row and index entry, has no UDFs, and a batch is its only atomic unit);
- the SQL must parse on SQLite builds with a fixed parser stack (Shallow SQL);
- the core stays sans-IO.

Decisions D13–D17 in `lat.md/decisions.md` record the choices below.

## Goals / Non-Goals

**Goals:**
- Cypher and SPARQL over one dataset, with no copy or sync.
- A Cypher `MATCH` compiles to a single SQL statement whenever the SPARQL equivalent would.
- Cypher writes are atomic in one request on every backend, D1 included.
- When the store has an ontology or shapes, OWL and SHACL change what Cypher returns and how it compiles.
- Pass at least 80% of the read-only openCypher TCK scenarios. Deviations are allow-listed and justified.

**Non-Goals:**
- Full GQL (ISO/IEC 39075). The AST is designed to grow into it later.
- Bolt protocol and Neo4j driver compatibility.
- `owl:sameAs` in Cypher: it breaks id-based joins.
- Point and duration types in the first release.
- APOC and user-defined procedures. Only the schema procedures are built in.
- A native PG storage layout.

## Decisions

### Second frontend over the RDF store (D13)
Cypher is lowered into the same compiler, planner and backends as SPARQL. *Alternative:* a separate PG engine with `nodes` and `edges` tables and JSON properties. It would make relationship properties cheaper and writes lighter, but it would duplicate four backends and the planner. It would also lose SPARQL interop and need an invented OWL/SHACL-over-PG layer. That alternative is estimated at 30–40 person-weeks, against 22–29 for this change. It would only win for edge-property-heavy OLTP workloads that need no SPARQL, OWL or SHACL.

### Relationships as asserted triples, with reifiers only when needed (D14)
`(a)-[:T]->(b)` is always the triple `a :T b`, so a hop costs one join on `posg`/`ospg`. A reifier `_:r rdf:reifies <<( a :T b )>>` is created only in three cases:
- the relationship has properties;
- a second, parallel relationship between the same nodes is created;
- the query binds the relationship to a variable that needs its identity.

Parallel relationships are several reifiers of one triple term. Deleting one relationship deletes its reifier. The asserted triple is deleted in the same batch only when no reifier remains, which one `NOT EXISTS` checks. For a relationship without a reifier, its identity for uniqueness is the tuple `(s, p, o)`. *Alternative:* every relationship as an intermediate node. That costs two joins per hop and triple write amplification.

Going from a traversed triple to its reifier needs `triple_terms(s, p, o)`. The index is optional, following D3's rule of keeping indexes few. Without it, only queries that walk relationship-first can read relationship properties.

### Fresh node IRIs from Rust (D15, changed)
The plan was an inline GeneratedNode id tag, so SQL could create nodes per row. Writes are computed in Rust instead (D16), so a created node gets an IRI minted in Rust (`urn:oxilite:node:<random>-<n>`), hashed like any term. It also gets an `rdf:type rdfs:Resource` marker: an RDF resource exists only through its triples, and a node without labels, properties or relationships would otherwise vanish. The marker is optional (`node_marker`) and hidden from `labels()`.

### Lowering to SPARQL algebra, with a Rust tail (D16, changed)
The plan was an internal `Op` algebra shared by SPARQL and Cypher. Lowering to `spargebra` covers the property-graph operations without it, and leaves the M2 compiler untouched:
- relationship uniqueness is `FILTER(!(sameTerm…))` between hops;
- a variable-length pattern is a `UNION` of one branch per length (per-hop `UNION`s for undirected hops), or a property path when unbounded, directed and unnamed;
- a named relationship looks its reifier up with `OPTIONAL { ?r rdf:reifies <<( ?a ?p ?b )>> }`;
- bindings and filters that read variables of earlier clauses are applied after the join (SPARQL evaluates sub-patterns on their own), keyed by a branch marker when there are several branches.

The first clause SQL cannot express starts the Rust tail, which evaluates projections, `UNWIND` and all writes over the rows of the one query. `shortestPath` binds its ends in SQL and searches breadth-first in the job, one request per level. *Consequence:* a writing statement reads once and then sends one atomic write request; natively both share a transaction, on D1 the write batch is atomic but another writer may change the data in between.

### SHACL shapes as the PG schema (D17, changed)
Shapes are read into a summary per target class and path (datatype, `minCount`/`maxCount`, `sh:in`, `sh:pattern`), once per writing statement or once per store with `cypher_schema()`:
- `minCount 1` → the property joins without `OPTIONAL` (with a schema passed in the options);
- `maxCount 1` → the property reads as a scalar;
- writes: every node the statement created or changed is checked on its final state before the write request is sent. The planned guard table is not needed; a violation sends nothing.

- `datatype` → a value type in `QueryOptions::var_types` (a small core addition), so the compiler compares the property with one typed branch.

A pattern comprehension runs as its own query: the pattern joined with the distinct values of the outer variables it reads; the job keys the rows by those values (or, for nodes of an enclosing list comprehension, by their ids in a per-row map). The shapes also answer `db.labels()` and `db.schema.nodeTypeProperties()`.

### Temporal values
openCypher's temporal types are implemented in Rust (`temporal.rs`): calendar arithmetic on day numbers (any year), ISO 8601 parsing and Java-compatible rendering, construction from maps, projection, truncation, duration arithmetic and `duration.between`. Named time zones resolve through `jiff`'s IANA database (bundled for WebAssembly). They are stored as `xsd:date`, `xsd:time`, `xsd:dateTime` and `xsd:duration`; a datetime in a named zone has its own datatype, since `[Europe/Stockholm]` is not valid `xsd:dateTime`.

### Values and results
`CypherResult` rows hold `Value::{Null, Bool, Int, Float, String, Temporal, List, Map, Node, Relationship, Path}`. Nodes and relationships are materialised in one batched request that fetches `rdf:type` and literal rows for all result ids, with their terms joined. Values with several RDF values follow the `multi_value` policy (`list` by default, or `first` or `error`), unless SHACL says the property is scalar.

## Risks / Trade-offs

- [Trail variable-length paths grow CTE path strings] → A default depth cap, and an `explain()` warning for unbounded patterns.
- [Reified relationships multiply D1 writes] → Create reifiers only when needed.
- [A writing statement is not isolated from concurrent writers on D1] → The write batch is atomic; natively the read and the write share a transaction.
- [TCK null, list and coercion edge cases] → Allow-list them like D11 and D12, with justification.
- [Multi-valued RDF properties from non-Cypher writers] → The `multi_value` policy, and SHACL `maxCount` where shapes exist.

## Migration Plan

This change is purely additive: no schema change, no change to the term encoding or the SPARQL compiler. Existing SPARQL behaviour is unchanged, which the W3C suites verify.
