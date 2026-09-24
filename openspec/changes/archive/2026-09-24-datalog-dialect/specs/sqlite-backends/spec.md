## Purpose

Lets a backend say whether its SQLite is new enough for a compound recursive term, so mutual
recursion is compiled only where it will run.

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

Generated SQL MUST respect the declared limits. A backend whose SQLite version cannot be
established MUST declare the compound recursive term unavailable.

#### Scenario: Statement length limit
- **WHEN** a backend declares a maximum statement length and a large insert is performed
- **THEN** no generated statement exceeds that length

#### Scenario: No bound parameters needed
- **WHEN** any operation is compiled
- **THEN** its statements are self-contained and don't depend on bound parameters

#### Scenario: Compound recursive term is not assumed
- **WHEN** a backend does not declare support for a compound recursive term
- **THEN** no generated statement contains a recursive CTE with more than one recursive term
