## Purpose

Uses the store's OWL ontology and SHACL shapes in Cypher. OWL entailment widens what patterns match. Shapes act as the property-graph schema: they inform compilation, they are checked inside write batches, and they drive schema introspection.

## ADDED Requirements

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
When a shapes graph is registered, the system SHALL use it in compilation:
- a property with `sh:maxCount 1` is a scalar;
- a property with `sh:minCount 1` allows an inner join;
- a property with `sh:datatype` gets a static value type.

These optimisations MUST NOT change results for data that conforms to the shapes.

#### Scenario: Required property uses an inner join
- **WHEN** a shape requires `ex:name` (`sh:minCount 1`) for `ex:Person`, and `MATCH (p:Person) RETURN p.name` is explained
- **THEN** the SQL joins `ex:name` without `LEFT JOIN`

### Requirement: In-batch shape guards on writes
When shape guards are enabled, the system SHALL check the shape constraints covering the nodes a Cypher write touches, inside the same atomic request. The checked constraints are `sh:datatype`, `sh:minCount`, `sh:maxCount`, `sh:class`, `sh:in` and simple `sh:pattern`. A violation MUST abort the whole request.

#### Scenario: Invalid CREATE aborts on D1
- **WHEN** a shape requires `ex:age` to be `xsd:integer`, and `CREATE (:Person {age: 'old'})` runs on D1
- **THEN** the batch fails with a shape violation, and no quad is added

### Requirement: Schema introspection
The system SHALL answer `CALL db.labels()`, `CALL db.relationshipTypes()` and `CALL db.schema()` from the registered shapes and the planner statistics.

#### Scenario: Labels from shapes and data
- **WHEN** shapes target `ex:Person` and `ex:Company`, and the data holds only persons
- **THEN** `db.labels()` returns both labels
