## Purpose

Lets rudof validate against shapes that live in the store, so a shapes graph does not have to be
supplied as text on every call.

## ADDED Requirements

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
