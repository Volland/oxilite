## ADDED Requirements

### Requirement: Ontology graphs declare where they apply
A manifest `[[graph]]` with `role = "ontology"` SHALL accept `applies_to`, a list of graph
IRIs, and register the graph with that mapping. Without it, the ontology SHALL apply to all
graphs. Shapes files SHALL stay out of the project store.

#### Scenario: Ontology for one graph
- **WHEN** the manifest maps an ontology graph to `https://ex.org/g/staff` only
- **THEN** reasoning over the project entails from it for staff quads and not for quads of other graphs
