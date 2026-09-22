## Purpose

Compiles openCypher read queries to SQL over the oxilite store, with Cypher's semantics for relationship uniqueness, variable-length and shortest paths, and Cypher result values.

## ADDED Requirements

### Requirement: Read clauses
The system SHALL execute these clauses with openCypher semantics:
- `MATCH`, `OPTIONAL MATCH`, `WHERE`, `WITH`, `RETURN`, `UNWIND`, `UNION` and `UNION ALL`;
- aggregates, `DISTINCT`, `ORDER BY`, `SKIP` and `LIMIT`;
- `CASE`, list comprehensions, pattern comprehensions and `EXISTS { }`.

#### Scenario: WITH pipelines aggregation
- **WHEN** `MATCH (p:Person)-[:KNOWS]->(f) WITH p, count(f) AS n WHERE n > 1 RETURN p.name ORDER BY p.name` runs
- **THEN** it returns the names of people who know more than one person, sorted

#### Scenario: OPTIONAL MATCH yields null
- **WHEN** `MATCH (p:Person) OPTIONAL MATCH (p)-[:OWNS]->(c) RETURN p.name, c` runs and one person owns nothing
- **THEN** that person's row has `c = null`

### Requirement: Single statement compilation
The system SHALL compile a Cypher query without variable-length or shortest paths into one SQL statement, whenever the equivalent SPARQL query would compile to one. The statement MUST use the statistics planner's join order.

#### Scenario: Request count
- **WHEN** a three-hop `MATCH … RETURN` over plain relationships runs
- **THEN** it uses at most two backend requests: the query, plus one step that materialises nodes and relationships

### Requirement: Relationship uniqueness
The system SHALL NOT bind the same relationship to two relationship patterns within one `MATCH` clause.

#### Scenario: Triangle does not reuse an edge
- **WHEN** `MATCH (a)-[:R]-(b)-[:R]-(c) RETURN count(*)` runs over a single `:R` relationship
- **THEN** it returns `0`

### Requirement: Variable-length patterns
The system SHALL evaluate `-[:T*m..n]->` with trail semantics (no repeated relationship) using recursive SQL. When a bound endpoint exists, the recursion MUST be seeded from it. An unbounded pattern MUST be subject to a configurable depth cap, and `explain()` MUST warn about it.

#### Scenario: Bounded path over a cycle terminates
- **WHEN** `MATCH (a {id: 1})-[:NEXT*1..5]->(b) RETURN b.id` runs over a three-node cycle
- **THEN** it returns every node reachable by a trail of 1–5 relationships, and never repeats a relationship within a path

### Requirement: Shortest paths
The system SHALL evaluate `shortestPath` and `allShortestPaths` as a breadth-first search that issues one request per frontier level. It therefore works on async backends, including D1.

#### Scenario: Shortest path on D1
- **WHEN** `MATCH p = shortestPath((a {id: 1})-[:R*]-(b {id: 4})) RETURN length(p)` runs on the D1 backend
- **THEN** it returns the length of the shortest path, and it does not need the fallback evaluator

### Requirement: Cypher values
The system SHALL return rows of Cypher values: null, boolean, integer, float, string, temporal, list, map, node, relationship and path. A node MUST carry its labels and properties, and a relationship its type, endpoints and properties.

#### Scenario: Node value
- **WHEN** `MATCH (n:Person {name: 'Ada'}) RETURN n` runs
- **THEN** the result is a node value with label `Person` and property `name = 'Ada'`

### Requirement: Explain for Cypher
The system SHALL provide `explain()` for Cypher. It MUST report the generated SQL, each pattern's join order with estimated rows, and warnings for Cartesian products, unseeded closures and depth-capped paths.

#### Scenario: Explain shows join order
- **WHEN** `explain()` is called on a two-pattern `MATCH`
- **THEN** it lists both patterns in planned order with estimates
