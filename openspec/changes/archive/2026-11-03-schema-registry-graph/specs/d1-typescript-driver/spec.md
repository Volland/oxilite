## ADDED Requirements

### Requirement: Schema registry on D1
`D1Store` SHALL provide the Node registry methods as promises, running the core's portable
SPARQL through its own `query` and `update`, and SHALL migrate a version 1 registry on `open()`.

#### Scenario: Register on D1
- **WHEN** a graph is registered as `ontology` on a D1 store
- **THEN** `schemaGraphs()` lists it and a query with `include_schema_graphs: false` does not match its triples
