## Purpose

Moves the Cypher property-graph schema from a per-statement SPARQL query onto the compiled shape
index, without changing what the schema means.

## MODIFIED Requirements

### Requirement: Shape checks on writes
When shape checks are enabled, the system SHALL check the shape constraints covering every node a
Cypher write creates or changes, on the node's final state, before the write request is sent. The
checked constraints are `sh:datatype`, `sh:minCount`, `sh:maxCount`, `sh:in` and `sh:pattern`. A
violation MUST abort the statement with no change applied.

The shapes SHALL be read from the compiled shape index in one request, not by evaluating a SPARQL
query, and the constraints applied MUST be the same as those the equivalent SPARQL query would
report for the same dataset.

#### Scenario: Invalid CREATE aborts on D1
- **WHEN** a shape requires `ex:age` to be `xsd:integer`, and `CREATE (:Person {age: 'old'})` runs
  on D1
- **THEN** the statement fails with a shape violation, and no quad is added

#### Scenario: Index and query agree
- **WHEN** the shape index and the shapes SPARQL query are both read over a dataset with
  datatypes, cardinalities, patterns, `sh:in` lists, relationship-valued shapes and two shapes
  targeting the same class and path
- **THEN** they describe the same constraints

#### Scenario: One request per writing statement
- **WHEN** a writing Cypher statement runs with shape checks enabled and no preloaded schema
- **THEN** reading the shapes costs one request
