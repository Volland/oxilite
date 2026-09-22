Status of the implementation. Items that changed during implementation say how; see design.md.

## 1. Mapping and decisions

- [x] 1.1 PG ↔ RDF vocabulary: base IRI, prefixes, overrides and absolute IRIs, set per call through `CypherOptions` (not stored in `oxilite_meta`)
- [x] 1.2 Record D13–D17 in `lat.md/decisions.md`, with the relationship and reifier lifecycle and the `multi_value` policy

## 2. Parser and semantic pass

- [x] 2.1 Hand-written recursive-descent parser for openCypher 9 (no parser dependency, small for wasm)
- [x] 2.2 AST for reads and writes (`oxilite_cypher::ast`)
- [x] 2.3 Validation pass: scopes across `WITH`, variable kinds, aliases, aggregation rules, constant `SKIP`/`LIMIT`

## 3. Compiler IR (changed)

- [x] 3.1 Replaced by lowering to `spargebra` algebra plus a Rust tail (D16); the M2 compiler is unchanged
- [x] 3.2 Not needed: no port of `compiler/mod.rs` and `compiler/ops.rs`
- [x] 3.3 The W3C SPARQL suites and the differential corpus still pass

## 4. Read MVP

- [x] 4.1 MATCH, OPTIONAL MATCH and WHERE to BGPs, left joins and filters; guarded optional property reads; lifted bindings for variables of earlier clauses
- [x] 4.2 WITH as sealed subqueries; aggregates, DISTINCT, ORDER BY, SKIP and LIMIT; UNION; UNWIND of constants as VALUES
- [x] 4.3 `CypherResult` and node/relationship materialization (one request with joined terms); the `multi_value` policy
- [x] 4.4 Relationship properties through optional reifiers (the `triple_terms(s, p, o)` index was not needed)

## 5. Property-graph operations

- [x] 5.1 Relationship uniqueness per MATCH
- [x] 5.2 Variable-length patterns: one branch per length with trail filters; unbounded directed patterns as property paths (reachability)
- [x] 5.3 `shortestPath` / `allShortestPaths` as a breadth-first step machine
- [x] 5.4 Path values; lists, maps and CASE; list comprehensions, quantifiers, `reduce`; `EXISTS { }`; pattern predicates in projections
- [x] 5.5 Pattern comprehensions `[p = (a)-->(b) WHERE … | b.x]`: one keyed side query per comprehension, also inside list comprehensions
- [x] 5.6 Temporal types (`date`, `time`, `localtime`, `datetime`, `localdatetime`, `duration`) with the openCypher temporal functions

## 6. Writes

- [x] 6.1 Fresh node IRIs minted in Rust and an `rdf:type rdfs:Resource` marker (replaces the GeneratedNode tag, D15)
- [x] 6.2 CREATE, SET and REMOVE computed in Rust, applied as one atomic request
- [x] 6.3 MERGE (with ON CREATE / ON MATCH), DELETE and DETACH DELETE, including the reifier lifecycle
- [x] 6.4 Constraint errors (deleting a connected node) raised before the write request is sent (no guard table)

## 7. OWL-aware Cypher

- [x] 7.1 The per-query reasoning option on Cypher (`CypherOptions::query`)
- [x] 7.2 Labels through subclasses, relationship types through subproperties and inverses (the core's rewriting)

## 8. SHACL-aware Cypher

- [x] 8.1 A shape summary per target class and path (`Store::cypher_schema`)
- [x] 8.2 Mandatory properties join without OPTIONAL; single-valued properties read as scalars
- [x] 8.3 Static value types from `sh:datatype`: `QueryOptions::var_types` tells the compiler a variable's type, so comparisons compile to one typed branch
- [x] 8.4 Write checks for datatype, cardinality, `in` and `pattern`, before the write request
- [x] 8.5 `db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()` and `db.schema.nodeTypeProperties()`

## 9. Bindings and harness

- [x] 9.1 `cypher()` / `explain_cypher()` on `Store` and `AsyncStore`; `cypher()` / `explainCypher()` on the wasm engine, `@oxilite/node` and `@oxilite/d1`
- [x] 9.2 openCypher TCK runner in `crates/oxilite-cypher/tests/tck.rs`: 3728 / 3880 scenarios (read-only 96.3%), the rest allow-listed
- [x] 9.3 Specification scenarios on the bundled SQLite, the system `libsqlite3`, the D1 code path and Miniflare D1
- [x] 9.4 A differential corpus posing the same questions through SPARQL and Cypher (`tests/differential.rs`, 21 questions)
- [x] 9.5 Update `lat.md/` and run `lat check`
