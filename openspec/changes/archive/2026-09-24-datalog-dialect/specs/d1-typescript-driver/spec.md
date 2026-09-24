## Purpose

Carries the Datalog dialect to the edge, where its one-statement compilation matters most.

## MODIFIED Requirements

### Requirement: Async API parity
The package SHALL provide `query`, `update`, `load`, `dump`, `add`, `delete`, `has`, `match`,
`size`, `explain` and `optimize` as async functions with the same arguments and result types as
the Node package. When the WebAssembly core is built with the Datalog feature, it SHALL also
provide `datalog`, `datalogMaterialize` and `explainDatalog` with the same signatures and result
shapes.

#### Scenario: Update then query
- **WHEN** an INSERT DATA update is awaited and then a query runs
- **THEN** the query sees the inserted data

#### Scenario: Datalog over a D1 binding
- **WHEN** a linear recursive program is awaited against a D1 binding
- **THEN** it returns the same solutions the Node package returns for the same data

## ADDED Requirements

### Requirement: Datalog is opt-in in the WebAssembly core
The Datalog frontend SHALL be a non-default feature of the WebAssembly core, so a Worker that does
not use rules does not carry them, and its cost MUST be documented where the other optional
frontends document theirs.

#### Scenario: Default build is unchanged
- **WHEN** the WebAssembly core is built with default features
- **THEN** it exports no Datalog entry point and its size is what it was
