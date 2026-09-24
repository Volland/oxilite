# shape-validation Specification

## Purpose
Validates data stored in oxilite against SHACL and ShEx shapes using rudof's engines, on native backends directly and on D1 through a bounded prefetch. Shapes may be supplied as text or read from the shapes graphs the store has registered.

## Requirements

### Requirement: SHACL validation
The system SHALL validate the store against a SHACL shapes graph and return a SHACL validation report (conformance flag and results) matching what rudof produces for the same data in memory.

#### Scenario: Violation reported
- **WHEN** a shape requires `sh:minCount 1` on `ex:name` and a target node has no name
- **THEN** the report doesn't conform and contains a result for that node and path

#### Scenario: Same report as in-memory rudof
- **WHEN** the same data and shapes are validated in oxilite and with rudof over an in-memory graph
- **THEN** the reports are equal

### Requirement: ShEx validation
The system SHALL validate nodes against a ShEx schema with a shape map, and return rudof's result shape map.

#### Scenario: Conforming node
- **WHEN** a node that satisfies its shape is validated
- **THEN** its result is conformant

### Requirement: Bounded validation on D1
The system SHALL validate on D1 by loading only the relevant subgraph with a bounded number of requests. When the subgraph exceeds a configured size, it MUST fail with an error rather than truncate silently.

#### Scenario: Size limit
- **WHEN** the relevant subgraph exceeds the configured limit
- **THEN** validation returns an error stating the limit and the size found

### Requirement: Conformance
The system SHALL pass rudof's SHACL and ShEx test suites when rudof is run over an oxilite-backed store on a native backend.

#### Scenario: Suite gate
- **WHEN** rudof's test suites run against oxilite
- **THEN** results equal those of rudof's in-memory graph

### Requirement: Shapes read from the store
The system SHALL compile a SHACL schema from a shapes graph held in the store, named explicitly
or taken from the registry when exactly one SHACL graph is registered, and validate with it. The
resulting report MUST equal the report produced by supplying the same shapes as text.

#### Scenario: Same report from stored shapes
- **WHEN** a shapes graph is loaded into the store, registered as SHACL shapes, and validation
  runs against the stored shapes
- **THEN** the report equals the one produced by passing those shapes as Turtle

#### Scenario: Named shapes graph
- **WHEN** two shapes graphs are in the store and one is named in the call
- **THEN** only that graph's shapes are applied

#### Scenario: Ambiguous registry
- **WHEN** no graph is named and the registry holds more than one SHACL graph
- **THEN** the call fails with an error naming the candidates, and nothing is validated

#### Scenario: No shapes
- **WHEN** no graph is named and no SHACL graph is registered
- **THEN** the call fails with an error saying so
