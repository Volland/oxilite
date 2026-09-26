## ADDED Requirements

### Requirement: Schema registry from Node
`Store` SHALL provide:
- `registerSchemaGraph(graph, role, {iri, version, sha256, imports, appliesTo, active})`;
- `schemaGraphs()`;
- `setSchemaGraphActive(graph, active)`;
- `unregisterSchemaGraph(graph)`;
- `dropSchemaGraph(graph)`;
- `shapeIndex()`.

`graph` SHALL be a term or an IRI string. Query options SHALL accept `include_schema_graphs`.

#### Scenario: Scoped reasoning
- **WHEN** a Node store registers one of two conflicting ontology graphs and queries with `reasoning: "rdfs"`
- **THEN** only the registered ontology's axioms entail answers
