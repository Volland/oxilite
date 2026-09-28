## Purpose

Lets a backend say whether it has vector types and index methods, so vector indexes are built and
searched only where they will run.

## MODIFIED Requirements

### Requirement: Backend capabilities
The system SHALL let each backend declare these capabilities:
- maximum statement length
- maximum statements per request
- availability of oxilite user-defined functions
- availability of interactive transactions
- whether 64-bit integers must be returned as text
- whether a recursive common table expression may have a compound recursive term
  (SQLite 3.34.0 and later)
- availability of vector types and distance functions (`vector32`, `vector_distance_cos`, …)
- availability of index methods (`CREATE INDEX … USING method`)

Generated SQL MUST respect the declared limits. A backend whose SQLite version cannot be
established MUST declare the compound recursive term unavailable. No statement using a vector
function or an index method MUST be generated for a backend that does not declare it.

#### Scenario: Statement length limit
- **WHEN** a backend declares a maximum statement length and a large insert is performed
- **THEN** no generated statement exceeds that length

#### Scenario: No bound parameters needed
- **WHEN** any operation is compiled
- **THEN** its statements are self-contained and don't depend on bound parameters

#### Scenario: Compound recursive term is not assumed
- **WHEN** a backend does not declare support for a compound recursive term
- **THEN** no generated statement contains a recursive CTE with more than one recursive term

#### Scenario: Vector functions are not assumed
- **WHEN** a backend does not declare vector functions and a store is opened, queried and
  optimized on it
- **THEN** no generated statement calls a vector function or reads a vector table
