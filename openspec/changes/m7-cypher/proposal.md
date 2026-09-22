## Why

Many graph users and tools speak Cypher and think in property graphs (PGs): nodes with labels and properties, and typed relationships that carry their own properties. oxilite already has the hard parts of a graph database on SQLite and D1: storage, a planner, recursive paths, atomic batches and every backend. Adding Cypher over the same data gives one dataset that SPARQL and Cypher can both query. Because the data stays RDF, Cypher can also benefit from OWL (label and type hierarchies, inverse, symmetric and transitive relations) and from SHACL (shapes act as the PG schema). An engine built from scratch on a native PG layout would lose all of that and duplicate the backends. It would also need its own, non-standard mapping to OWL and SHACL. That alternative is estimated at 30–40 person-weeks, against 22–29 for this change.

## What Changes

- New crate **`oxilite-cypher`** (sans-IO), containing:
  - an openCypher 9 parser with a GQL-compatible AST;
  - a semantic pass (scopes across `WITH`, and vocabulary resolution);
  - lowering to the core compiler;
  - a step machine that decodes results into Cypher values.
- **PG ↔ RDF mapping** (configurable vocabulary):
  - A node is an IRI or blank node. A label is `rdf:type`. A node property is a literal triple.
  - A relationship is an asserted triple, which traversal uses. It also gets an RDF 1.2 reifier (`_:r rdf:reifies <<( a :T b )>>`), but only when it has properties, is a parallel edge, or is bound to a variable.
  - Lists and maps are `rdf:JSON` literals.
  - A policy flag decides how Cypher reads properties that hold several values.
- **Compiler IR refactor in `oxilite-core`.** The compiler currently works directly on `spargebra`. It gets an internal `Op` algebra that SPARQL and Cypher both lower into. `Op` adds operations SPARQL lacks: relationship uniqueness, variable-length paths with trail semantics (no repeated relationship), shortest path, path values, lists and maps. SPARQL behaviour must not change.
- **Cypher reads:**
  - `MATCH`, `OPTIONAL MATCH`, `WHERE`, `WITH`, `RETURN`, `UNWIND` and `UNION`;
  - aggregates, `ORDER BY`, `SKIP`, `LIMIT` and `DISTINCT`;
  - `CASE`, list and pattern comprehensions, and `EXISTS { }` subqueries;
  - variable-length patterns and `shortestPath` / `allShortestPaths` (breadth-first search as a step machine, one request per level, so it works on D1).
- **Cypher writes:** `CREATE`, `MERGE`, `SET`, `REMOVE`, `DELETE` and `DETACH DELETE`. Each statement is one atomic request (one D1 batch), using the existing `update_buffer` and guard-table patterns.
- **Term encoding:** a new inline tag **GeneratedNode** (tag 8). Its IRI `urn:oxilite:n:<payload>` can be decoded from the id alone, so `CREATE` can make fresh nodes per row inside SQL, with no `terms` row and no hash computed in SQL.
- **Storage:** an optional index on `triple_terms(s, p, o)`, so SQL can find the reifier of a traversed relationship. It is optional because every index adds D1 write cost.
- **OWL-aware Cypher** (needs M4): labels and relationship types are rewritten through `tbox_closure`, using the same per-query reasoning option as SPARQL (`None | Rdfs | OwlQl`).
- **SHACL-aware Cypher** (needs M5):
  - Shapes inform compilation: `maxCount 1` makes a property scalar, `minCount 1` allows an inner join, and `datatype` gives static types.
  - Simple constraints are checked inside the same batch as a write, through a guard table.
  - The schema procedures `db.labels()`, `db.relationshipTypes()` and `db.schema()` are derived from the shapes and the statistics tables.
- **Bindings:** a `cypher()` method on the Rust stores, `@oxilite/node`, `@oxilite/d1` and the wasm core, plus `explain()` for Cypher.
- **Harness:** an openCypher TCK runner and a differential corpus that queries the same data through both SPARQL and Cypher.

## Capabilities

### New Capabilities
- `property-graph-mapping`: how PG nodes, labels, properties and relationships (including parallel relationships and relationship properties) are stored as RDF 1.2 quads, and how they are read back.
- `cypher-query`: openCypher read queries compiled to SQL. Covers relationship uniqueness, variable-length and shortest paths, Cypher values, and `explain()`.
- `cypher-update`: atomic Cypher write clauses and fresh node generation.
- `ontology-aware-cypher`: OWL entailment and SHACL-informed compilation and validation for Cypher.

### Modified Capabilities
- `term-encoding`: adds the inline GeneratedNode tag.

## Impact

- New crate `oxilite-cypher`. Parser dependency: an existing crate if its licence and coverage fit, otherwise our own `chumsky` grammar.
- An internal compiler refactor in `oxilite-core`, gated on the full W3C SPARQL suites and the differential harness.
- Schema additions (optional `triple_terms_spo` index and an `oxilite_pg_guard` assertion table for Cypher and SHACL conditions), all created with `IF NOT EXISTS`. `schema_version` is not bumped.
- Depends on M3 for write batches, M4 for OWL-aware Cypher and M5 for SHACL-aware Cypher. The read MVP (tasks 1–4 and 9) does not need M4 or M5.
- Estimated effort is 22–29 person-weeks, 9–12 of them for the read MVP. That excludes M4 and M5.
