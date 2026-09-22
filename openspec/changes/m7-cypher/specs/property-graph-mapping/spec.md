## Purpose

Defines how property-graph (PG) nodes, labels, properties and relationships are stored as RDF 1.2 quads and read back. Data written with Cypher can then be read with SPARQL, and RDF data can be read with Cypher.

## ADDED Requirements

### Requirement: Nodes, labels and node properties
The system SHALL store the parts of a node as follows:
- a PG node is an IRI or a blank node;
- each label is an `rdf:type` triple whose object is the label's IRI;
- each node property is a triple whose object is a literal.

Label, relationship-type and property names SHALL map to IRIs through a configurable vocabulary. By default, a name is appended to a base IRI stored in `oxilite_meta`.

#### Scenario: Cypher node is visible to SPARQL
- **WHEN** `CREATE (:Person {name: 'Ada'})` runs with base IRI `http://ex/`
- **THEN** `SELECT ?n WHERE { ?n a <http://ex/Person> ; <http://ex/name> "Ada" }` returns one solution

#### Scenario: RDF data is visible to Cypher
- **WHEN** `ex:bob a ex:Person ; ex:age 42` is loaded as Turtle
- **THEN** `MATCH (p:Person) RETURN p.age` returns `42`

### Requirement: Relationships are asserted triples
The system SHALL store each relationship `(a)-[:T]->(b)` as the asserted triple `a :T b` in the graph that holds the PG. Traversing a relationship MUST need only one join on the quad table.

#### Scenario: Plain relationship adds one quad
- **WHEN** `CREATE (a)-[:KNOWS]->(b)` runs on two existing nodes, with no properties
- **THEN** exactly one quad `a :KNOWS b` is added and no reifier is created

### Requirement: Relationship properties and parallel relationships
The system SHALL give a relationship an RDF 1.2 reifier (`_:r rdf:reifies <<( a :T b )>>`) when:
- the relationship has properties (stored on the reifier), or
- a second relationship of the same type between the same nodes is created.

Each parallel relationship MUST have its own reifier. Deleting one relationship MUST remove only its reifier. The asserted triple MUST be removed only when its last relationship is deleted.

#### Scenario: Relationship property round-trip
- **WHEN** `CREATE (a)-[:KNOWS {since: 2020}]->(b)` runs
- **THEN** `MATCH (a)-[r:KNOWS]->(b) RETURN r.since` returns `2020`, and SPARQL finds `?r rdf:reifies <<( ?a :KNOWS ?b )>> ; :since 2020`

#### Scenario: Parallel relationships are distinct
- **WHEN** two `:KNOWS` relationships with different `since` values are created between the same nodes
- **THEN** `MATCH (a)-[r:KNOWS]->(b) RETURN count(r)` returns `2`

#### Scenario: Deleting one parallel relationship keeps the other
- **WHEN** one of those two relationships is deleted
- **THEN** the asserted triple `a :KNOWS b` remains, and one relationship is matched

### Requirement: Multi-valued properties
The system SHALL read a property that has several RDF values according to a per-store `multi_value` policy: `list` (the default, values in term order), `first` or `error`. When a SHACL shape declares `sh:maxCount 1` for the property, the property MUST be read as a scalar.

#### Scenario: Several values read as a list
- **WHEN** a node has `ex:tag "a"` and `ex:tag "b"` and the policy is `list`
- **THEN** `RETURN n.tag` returns `['a', 'b']`

### Requirement: Lists and maps
The system SHALL store Cypher list and map property values as `rdf:JSON` literals, and it SHALL decode them back to Cypher values.

#### Scenario: List property round-trip
- **WHEN** `CREATE (n {xs: [1, 2, 3]})` runs
- **THEN** `RETURN n.xs` returns `[1, 2, 3]`
