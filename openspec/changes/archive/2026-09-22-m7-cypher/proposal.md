## Why

Many graph users and tools speak Cypher and think in property graphs (PGs): nodes with labels and properties, and typed relationships that carry their own properties. oxilite already has the hard parts of a graph database on SQLite and D1: storage, a planner, recursive paths, atomic batches and every backend. Adding Cypher over the same data gives one dataset that SPARQL and Cypher can both query. Because the data stays RDF, Cypher can also benefit from OWL (label and type hierarchies, inverse, symmetric and transitive relations) and from SHACL (shapes act as the PG schema). An engine built from scratch on a native PG layout would lose all of that and duplicate the backends. It would also need its own, non-standard mapping to OWL and SHACL. That alternative is estimated at 30–40 person-weeks, against 22–29 for this change.

## What Changes

- New crate **`oxilite-cypher`** (sans-IO): an openCypher 9 parser, a validation pass, a planner that splits each statement into a SQL part and a Rust *tail*, lowering of the SQL part to `spargebra` algebra, an expression evaluator, temporal types, and a step machine (`CypherJob`) that its drivers run.
- **PG ↔ RDF mapping** (configurable vocabulary):
  - A node is an IRI or blank node. A label is `rdf:type`. A node property is a literal triple. Created nodes get a fresh IRI and an `rdf:type rdfs:Resource` marker.
  - A relationship is an asserted triple, which traversal uses. Relationships with properties, and every relationship of a triple that has several, get an RDF 1.2 reifier (`_:r rdf:reifies <<( a :T b )>>`).
  - Lists and maps are `rdf:JSON` literals; temporal values are `xsd:date`, `xsd:time`, `xsd:dateTime` and `xsd:duration`.
  - A policy decides how Cypher reads properties that hold several values.
- **Reads:** the reading clauses lower to one SPARQL query, compiled to SQL by the unchanged compiler (planner, reasoning rewrites and the spareval fallback included). Relationship uniqueness is inequality filters; variable-length patterns are unions of fixed-length branches (or property paths when unbounded); `shortestPath` is a breadth-first step machine, one request per level. Projections that need lists, maps, `collect()` or temporal arithmetic run in Rust.
- **Writes:** `CREATE`, `MERGE`, `SET`, `REMOVE`, `DELETE` and `DETACH DELETE` are computed in Rust from the read, then applied as one atomic request (one D1 batch); natively the read and the write share one transaction.
- **OWL-aware Cypher:** the per-query reasoning option of SPARQL (`None | Rdfs | OwlQl`) applies to labels and relationship types.
- **SHACL-aware Cypher:** shapes loaded with `cypher_schema()` make mandatory properties join without `OPTIONAL`, single-valued properties read as scalars, and `sh:datatype` type comparisons for the compiler; writes are checked against datatype, cardinality, `sh:in` and `sh:pattern` before the write request. Schema procedures: `db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()`, `db.schema.nodeTypeProperties()`.
- **Bindings:** `cypher()` on `Store`, `AsyncStore`, the wasm engine, `@oxilite/node` and `@oxilite/d1`, with `explain` variants.
- **Harness:** the openCypher TCK (git submodule `testsuite/openCypher`) with a Gherkin runner and an allow-list.

## Capabilities

### New Capabilities
- `property-graph-mapping`: how PG nodes, labels, properties and relationships (including parallel relationships and relationship properties) are stored as RDF 1.2 quads, and how they are read back.
- `cypher-query`: openCypher read queries compiled to SQL. Covers relationship uniqueness, variable-length and shortest paths, Cypher values, and `explain()`.
- `cypher-update`: atomic Cypher write clauses and fresh node generation.
- `ontology-aware-cypher`: OWL entailment and SHACL-informed compilation and validation for Cypher.

### Modified Capabilities
<!-- none: the term encoding and storage schema are unchanged -->

## Impact

- New crate `oxilite-cypher` (dependencies: `regex-lite`, `jiff` with an optional bundled time zone database, `serde_json`). The umbrella crate gains a `cypher` feature; the wasm engine a default `cypher` feature (`cypher-lite` omits the time zone database).
- No change to the term encoding or the storage schema. The compiler gains `QueryOptions::var_types` (static value types of variables).
- Depends on M3 for atomic write requests, M4 for OWL-aware matching and M5 for shapes.
- The wasm module grows from 0.74 MB to 1.18 MB gzipped.
