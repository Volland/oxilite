Estimates are in person-weeks for one experienced Rust engineer. The read MVP is sections 1–4 and 9.

## 1. Mapping and decisions (1 wk)

- [ ] 1.1 Finalise the PG ↔ RDF vocabulary configuration (base IRI, overrides, and names derived from SHACL)
- [ ] 1.2 Record D13–D17 in `lat.md/decisions.md`, with the relationship and reifier lifecycle and the `multi_value` policy

## 2. Parser and semantic pass (2–3 wk; about 1 if an existing parser fits)

- [ ] 2.1 Evaluate existing Rust Cypher/GQL parsers for licence and coverage; otherwise write a `chumsky` grammar for openCypher 9
- [ ] 2.2 A GQL-compatible AST for reads and writes
- [ ] 2.3 Semantic pass: scopes across `WITH`, variable kinds (node, relationship, path, value), and vocabulary resolution

## 3. Compiler IR (1.5–2 wk; lands before any Cypher lowering)

- [ ] 3.1 An internal `Op` algebra in `oxilite-core`, with `spargebra` lowered into it
- [ ] 3.2 Port `compiler/mod.rs` and `compiler/ops.rs` to consume `Op`
- [ ] 3.3 The W3C SPARQL query and update suites and the differential corpus stay at their current pass level

## 4. Read MVP (4–5 wk)

- [ ] 4.1 Lower MATCH, OPTIONAL MATCH and WHERE to BGP, LeftJoin and Filter; property access as correlated lookups
- [ ] 4.2 WITH as sealed blocks; aggregates, DISTINCT, ORDER BY, SKIP and LIMIT; UNION; UNWIND through `json_each`
- [ ] 4.3 `CypherResults` and the step that materialises nodes and relationships; the `multi_value` policy
- [ ] 4.4 An optional `triple_terms(s, p, o)` index, and reading relationship properties through reifiers

## 5. Property-graph operations (4–6 wk)

- [ ] 5.1 `UniqueEdges`: relationship uniqueness per MATCH
- [ ] 5.2 `VarLength`: recursive CTE with depth, trail check, seeding and depth cap
- [ ] 5.3 `ShortestPath` / `allShortestPaths` as a breadth-first step machine
- [ ] 5.4 Path values; lists, maps and CASE; list and pattern comprehensions; `EXISTS { }`

## 6. Writes (3–4 wk; needs M3)

- [ ] 6.1 The GeneratedNode tag (tag 8) in `encoding.rs`, with encoder mapping and decoding
- [ ] 6.2 CREATE, SET and REMOVE through `update_buffer` in one atomic request
- [ ] 6.3 MERGE (with ON CREATE / ON MATCH), DELETE and DETACH DELETE, including the reifier lifecycle
- [ ] 6.4 The `oxilite_pg_guard` table and constraint errors (for example, deleting a connected node)

## 7. OWL-aware Cypher (1–2 wk; needs M4)

- [ ] 7.1 The per-query reasoning option on Cypher
- [ ] 7.2 Label rewriting through subclasses; relationship-type rewriting through subproperties, inverses and symmetry; optional transitive expansion

## 8. SHACL-aware Cypher (2–3 wk; needs M5)

- [ ] 8.1 A compiled shape summary per target class and path
- [ ] 8.2 Compilation that uses the shapes (scalar, inner join, static types)
- [ ] 8.3 In-batch shape guards for datatype, cardinality, class, `in` and simple `pattern`
- [ ] 8.4 `db.labels()`, `db.relationshipTypes()` and `db.schema()`

## 9. Bindings and harness (3 wk, plus ongoing work)

- [ ] 9.1 `cypher()` and `explain()` on `Store`, `AsyncStore`, `@oxilite/node`, `@oxilite/d1` and the wasm core
- [ ] 9.2 openCypher TCK runner (Gherkin) in `testsuite/`, on the rusqlite, dylib and D1 backends; at least 80% of the read-only scenarios pass, with an allow-list for the rest
- [ ] 9.3 A differential corpus that queries the same data through SPARQL and Cypher
- [ ] 9.4 Update `lat.md/` (tests and architecture) and run `lat check`
