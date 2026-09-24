# schema-registry Specification

## Purpose
Records which named graphs hold schema rather than data — OWL ontologies, SHACL shapes graphs,
ShEx schemas — so that reasoning can be scoped to chosen ontologies, shapes have one compiled
source of truth, and schema graphs can be hidden from queries over the data.

## Requirements

### Requirement: Schema graphs are registered by role
The system SHALL let a named graph be registered with a role (ontology, SHACL shapes or ShEx
schema), optional ontology IRI, version, content digest and import list, and an active flag. It
SHALL list the registrations, change the active flag, and unregister a graph. Registering MUST
NOT move, copy or rewrite any triple: the graph stays ordinary RDF that SPARQL can read and
update.

#### Scenario: Register and list
- **WHEN** a graph holding an ontology is registered with role `ontology` and version `v1`
- **THEN** listing the registry returns that graph with role `ontology`, version `v1` and active

#### Scenario: Triples stay queryable
- **WHEN** a graph is registered as a shapes graph
- **THEN** `SELECT ?s ?p ?o WHERE { GRAPH <that graph> { ?s ?p ?o } }` still returns its triples

#### Scenario: Unregister keeps the data
- **WHEN** a registered graph is unregistered
- **THEN** the registry no longer lists it and its triples are still in the store

#### Scenario: Dropping a schema graph
- **WHEN** a registered graph is dropped through the registry
- **THEN** both its registration and every one of its quads are gone, in one atomic request

### Requirement: An empty registry means every graph
The system SHALL treat "no active graph registered for a role" as "every graph may contribute
for that role". A store that registers nothing MUST behave exactly as it did before the registry
existed.

#### Scenario: Unregistered store reasons over all graphs
- **WHEN** no ontology graph is registered, `ex:Dog rdfs:subClassOf ex:Animal` is stored in a
  named graph, and `?x a ex:Animal` is queried with RDFS reasoning over the default graph
- **THEN** instances of `ex:Dog` are returned, as before this change

### Requirement: Schema graphs can be hidden from queries
The system SHALL provide a per-query option that excludes every registered schema graph from
pattern matching. Schema graphs SHALL be visible by default. Hiding MUST apply regardless of the
graph's active flag, and MUST cover the default graph, `GRAPH ?g`, property paths and `OPTIONAL`
alike. It MUST NOT change results when no graph is registered.

#### Scenario: Axioms excluded from a data query
- **WHEN** an ontology graph is registered and `SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }`
  runs with schema graphs hidden over a union default graph
- **THEN** no triple of the ontology graph is counted

#### Scenario: Visible by default
- **WHEN** the same query runs without the option
- **THEN** the ontology's triples are counted

### Requirement: Compiled shape index
The system SHALL maintain a compiled index of the SHACL property shapes of the registered
shapes graphs (every graph when none is registered), holding for each target class and property
path the declared `sh:datatype`, `sh:minCount`, `sh:maxCount`, `sh:pattern`, `sh:in` values and
whether the shape is relationship-valued (`sh:class` / `sh:node`). The index SHALL be refreshed
by `optimize()` and, inside the same atomic request, by any write that touches a SHACL
predicate. Reading the index MUST take one request and no SPARQL evaluation.

#### Scenario: Refreshed inside the write
- **WHEN** a shape declaring `ex:age` as `xsd:integer` for `ex:Person` is inserted
- **THEN** the index immediately reports that datatype, with no `optimize()` in between

#### Scenario: Value list
- **WHEN** a shape declares `sh:in ( "a" "b" )` on a path
- **THEN** the index reports both values for that target and path

#### Scenario: Merged shapes
- **WHEN** two shapes target the same class and path, one declaring `sh:minCount` and the other
  `sh:datatype`
- **THEN** the index reports both constraints on that target and path

#### Scenario: Removed shape disappears
- **WHEN** the shape triples are deleted
- **THEN** the index no longer reports that constraint

### Requirement: Registry survives reopening
The system SHALL persist the registry and the compiled index in the database, and opening an
existing store that predates them MUST succeed and leave behaviour unchanged.

#### Scenario: Reopen
- **WHEN** a store with a registered ontology is closed and reopened
- **THEN** the registry still lists it
