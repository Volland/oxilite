# ontology-aware-cypher Specification

## Purpose

Uses the store's OWL ontology and SHACL shapes in Cypher. OWL entailment widens what patterns match. Shapes act as the property-graph schema: they inform compilation, they are checked inside write batches, and they drive schema introspection.

## Requirements

### Requirement: OWL-aware matching is opt-in per query
The system SHALL apply entailment to Cypher only when the query's reasoning option (`None | Rdfs | OwlQl`, shared with SPARQL) asks for it, and it SHALL rewrite patterns against `tbox_closure` without extra requests. The rewrites are:
- a label matches its subclasses;
- a relationship type matches its subproperties and inverse properties;
- a symmetric type matches in both directions.

#### Scenario: Label hierarchy
- **WHEN** `ex:Employee rdfs:subClassOf ex:Person` and `ex:ada a ex:Employee` are stored and `MATCH (p:Person) RETURN p` runs with RDFS reasoning
- **THEN** `ex:ada` is returned; without reasoning, it is not

#### Scenario: Inverse relationship type
- **WHEN** `ex:hasParent owl:inverseOf ex:hasChild` is stored and `MATCH (c)-[:hasParent]->(p)` runs with OWL-QL reasoning
- **THEN** pairs stored only as `p ex:hasChild c` are returned

### Requirement: SHACL-informed compilation
When shapes are loaded into the options, the system SHALL use them in compilation and reading:
- a property with `sh:maxCount 1` is a scalar;
- a property with `sh:minCount 1` allows an inner join;
- a property with `sh:datatype` gets a static value type, so comparisons on it compile to one typed branch.

These optimisations MUST NOT change results for data that conforms to the shapes.

#### Scenario: Datatype types a comparison
- **WHEN** a shape declares `ex:age` as `xsd:integer` for `ex:Person`, and `MATCH (p:Person) WHERE p.age > 30 RETURN p.name` is explained with the schema
- **THEN** the comparison compiles without the per-type branches, and the answer is unchanged

#### Scenario: Required property uses an inner join
- **WHEN** a shape requires `ex:name` (`sh:minCount 1`) for `ex:Person`, and `MATCH (p:Person) RETURN p ORDER BY p.name` is explained
- **THEN** the SQL joins `ex:name` without `LEFT JOIN`

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

### Requirement: Schema introspection
The system SHALL answer `CALL db.labels()`, `CALL db.relationshipTypes()` and `CALL db.schema()` from the registered shapes and the planner statistics.

#### Scenario: Labels from shapes and data
- **WHEN** shapes target `ex:Person` and `ex:Company`, and the data holds only persons
- **THEN** `db.labels()` returns both labels
