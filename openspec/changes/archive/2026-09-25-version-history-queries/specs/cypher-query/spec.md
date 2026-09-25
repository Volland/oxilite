## ADDED Requirements

### Requirement: Version option
A Cypher statement SHALL accept a version option (`HEAD~1`, `#42`, `@<xsd:dateTime>`) on a store with a
change log, resolved once per statement, and SHALL then match the property-graph view of the store at that
version, node and relationship materialization included. A writing statement
with a version option SHALL fail, because writes apply to the current state.

#### Scenario: Matching the past
- **WHEN** a node's property is changed and `MATCH (t:Ticket) RETURN t.status` runs with the version `HEAD~1`
- **THEN** it returns the status before the change

#### Scenario: Writing the past
- **WHEN** a `CREATE` statement runs with a version option
- **THEN** it fails and nothing is written
