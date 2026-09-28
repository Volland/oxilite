## Purpose

k-nearest-neighbour search over a vector index from SPARQL, Cypher, Datalog and the store API, with
the same semantics everywhere and compiled into the same SQL statement as the surrounding query.

## ADDED Requirements

### Requirement: Nearest-neighbour semantics
A search SHALL return at most `k` distinct nodes that have an embedding in the index (and, when the
index has a class, are instances of it), ordered by increasing distance under the index metric, ties
broken by node. A node with several embeddings SHALL count once, at its smallest distance. The
query SHALL be either a vector (a JSON array literal of the index's dimensions) or a node, whose
stored embedding is then used. `k` defaults to 10.

#### Scenario: Cosine ordering
- **WHEN** an index holds `[1,0,0]` for `ex:a`, `[0.7,0.7,0]` for `ex:b` and `[0,1,0]` for `ex:c`
  and is searched with `[1,0.1,0]` and `k = 2`
- **THEN** it returns `ex:a` then `ex:b`

#### Scenario: Query by node
- **WHEN** the same index is searched with the node `ex:a`
- **THEN** `ex:a` is first, at distance 0

#### Scenario: Class restriction
- **WHEN** the index is restricted to `ex:Doc` and only `ex:b` is an `ex:Doc`
- **THEN** only `ex:b` is returned

#### Scenario: Wrong query dimensions
- **WHEN** a three-dimensional index is searched with a two-element vector
- **THEN** the search fails with an error naming the expected dimensions

### Requirement: Distance and score
Every surface SHALL expose the metric's raw distance where it can carry a double, and SHALL expose
a similarity score defined as: cosine `1 − d/2`, Euclidean `1 / (1 + d²)`, dot product `−d`, Jaccard
`1 − d`.

#### Scenario: Cosine score
- **WHEN** a node is at cosine distance 0.2
- **THEN** its score is 0.9

### Requirement: SPARQL vector search
The system SHALL evaluate `SERVICE <oxilite:vector/NAME> { … }` whose body is triples of one subject
with the predicates `oxl:query` (a literal or an IRI; required), `oxl:k` (an integer), `oxl:node`,
`oxl:distance` and `oxl:score` (each a variable or a constant), as the search of the index `NAME`,
binding `oxl:node` to the node, `oxl:distance` to an `xsd:double` and `oxl:score` to an `xsd:double`.
The search SHALL compile into the query's single SQL statement, and SHALL combine with every other
graph pattern, filter, aggregate and modifier.

#### Scenario: Search joined with the graph
- **WHEN** a query joins the SERVICE's `?doc` with `?doc ex:title ?t` and filters on another property
- **THEN** it returns the matching documents with their titles, and `explain()` shows one statement

#### Scenario: Unknown index or predicate
- **WHEN** the SERVICE names an index that does not exist, or uses a predicate other than the five
- **THEN** the query fails with an error naming the index or the predicate

#### Scenario: With a host function
- **WHEN** the same query also projects a host function of `?t`
- **THEN** the results include the function's values, and the k-NN still runs as SQL

### Requirement: Cypher vector search
The system SHALL evaluate `CALL db.index.vector.queryNodes(name, k, query) YIELD node, score` —
`query` a list of numbers, a parameter holding one, or a string node IRI — as the search of the
index, with `node` bound as a node and `score` as a float, and SHALL allow `distance` to be yielded
too. Later `MATCH`, `WHERE`, `WITH` and `RETURN` clauses SHALL treat `node` like any matched node,
and the call SHALL compile into the statement's SQL query.

#### Scenario: Neo4j-shaped query
- **WHEN** `CALL db.index.vector.queryNodes('docs', 2, [1, 0.1, 0]) YIELD node, score RETURN
  node.title, score ORDER BY score DESC` runs
- **THEN** it returns the two nearest documents' titles, highest score first

#### Scenario: Node is a node
- **WHEN** the call is followed by `MATCH (node)-[:CITES]->(x) RETURN node, x`
- **THEN** `node` is returned as a node value with its labels and properties

### Requirement: Datalog vector search
The system SHALL provide the built-in relation `nearest(index, query, k, ?node, ?rank)` and its
four-argument form without rank, where `index` is a string, `query` a string vector or an IRI, `k`
an integer, and `rank` the 1-based position as an `xsd:integer`. It SHALL compile into the program's
SQL. A program that defines rules named `nearest` SHALL keep its own relation.

#### Scenario: Rules over neighbours
- **WHEN** `top(?d) :- nearest("docs", "[1,0.1,0]", 3, ?d, ?r), ?r <= 2. ?- top(?d).` runs
- **THEN** it returns the two nearest documents

#### Scenario: Non-constant arguments are rejected
- **WHEN** `nearest` is given a variable index name or `k`
- **THEN** compilation fails naming the argument

### Requirement: Store API search
The store SHALL provide `vector_search(name, query, k)` returning hits with node, distance and score
in search order.

#### Scenario: API search
- **WHEN** `vector_search("docs", VectorQuery::vector(&[1.0, 0.1, 0.0]), 1)` runs
- **THEN** it returns one hit for the nearest node with its distance and score
