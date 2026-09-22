# cypher-query Specification

## Purpose

Compiles openCypher read queries to SQL over the oxilite store, with Cypher's semantics for relationship uniqueness, variable-length and shortest paths, and Cypher result values.

## Requirements

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
- **THEN** it uses at most three backend requests: the query, its term resolution, and one request that materialises nodes and relationships

### Requirement: Relationship uniqueness
The system SHALL NOT bind the same relationship to two relationship patterns within one `MATCH` clause.

#### Scenario: Triangle does not reuse an edge
- **WHEN** `MATCH (a)-[:R]-(b)-[:R]-(c) RETURN count(*)` runs over a single `:R` relationship
- **THEN** it returns `0`

### Requirement: Variable-length patterns
The system SHALL evaluate `-[:T*m..n]->` with trail semantics (no repeated relationship). A pattern that binds a relationship or path variable MUST be subject to a configurable depth cap when unbounded. An unbounded, directed pattern without variables MAY be evaluated as reachability (recursive SQL seeded from a bound endpoint), and `explain()` MUST note it.

#### Scenario: Bounded path over a cycle terminates
- **WHEN** `MATCH (a {id: 1})-[:NEXT*1..5]->(b) RETURN b.id` runs over a three-node cycle
- **THEN** it returns every node reachable by a trail of 1–5 relationships, and never repeats a relationship within a path

### Requirement: Shortest paths
The system SHALL evaluate `shortestPath` and `allShortestPaths` as a breadth-first search that issues one request per frontier level. It therefore works on async backends, including D1.

#### Scenario: Shortest path on D1
- **WHEN** `MATCH p = shortestPath((a {id: 1})-[:R*]-(b {id: 4})) RETURN length(p)` runs on the D1 backend
- **THEN** it returns the length of the shortest path, and it does not need the fallback evaluator

### Requirement: Cypher values
The system SHALL return rows of Cypher values: null, boolean, integer, float, string, temporal (date, time, local time, datetime, local datetime, duration), list, map, node, relationship and path. A node MUST carry its labels and properties, and a relationship its type, endpoints and properties.

#### Scenario: Node value
- **WHEN** `MATCH (n:Person {name: 'Ada'}) RETURN n` runs
- **THEN** the result is a node value with label `Person` and property `name = 'Ada'`

### Requirement: Explain for Cypher
The system SHALL provide `explain()` for Cypher. It MUST report the generated SQL, each pattern's join order with estimated rows, and warnings for Cartesian products, unseeded closures and depth-capped paths.

#### Scenario: Explain shows join order
- **WHEN** `explain()` is called on a two-pattern `MATCH`
- **THEN** it lists both patterns in planned order with estimates

### Requirement: Temporal values
The system SHALL support openCypher's temporal types: construction from ISO 8601 strings and maps, projection between types, component access, truncation, comparison, arithmetic with durations and `duration.between`, with named time zones.

#### Scenario: Week date and named zone
- **WHEN** `RETURN date({year: 1817, week: 1}) AS d, datetime('2015-07-21T21:40:32.142[Europe/London]') AS t` runs
- **THEN** `d` is `'1816-12-30'` and `t` is `'2015-07-21T21:40:32.142+01:00[Europe/London]'`

### Requirement: openCypher TCK
The system SHALL pass at least 80% of the read-only openCypher TCK scenarios; the failing scenarios MUST be listed with their failure in an allow-list that the test enforces in both directions.

#### Scenario: TCK run
- **WHEN** the TCK test runs
- **THEN** every failing scenario is allow-listed and no allow-listed scenario passes

### Requirement: Pattern comprehensions
The system SHALL evaluate pattern comprehensions `[p = (a)-->(b) WHERE … | expr]`, giving each row the list of its own matches, including inside list comprehensions.

#### Scenario: Per-row lists
- **WHEN** `MATCH (p:Person) RETURN p.name, [(p)-[:KNOWS]->(f) | f.name]` runs and one person knows nobody
- **THEN** that person's list is empty and the others list their friends

### Requirement: SPARQL and Cypher agree
The system SHALL answer the same question identically through SPARQL and through Cypher over one dataset.

#### Scenario: Differential corpus
- **WHEN** the differential corpus runs
- **THEN** every question gives the same rows in both languages
