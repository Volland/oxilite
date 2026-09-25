## ADDED Requirements

### Requirement: Program version
A program SHALL be able to declare the version it reads with the directive `@version "REF" .`. It then
behaves exactly as the same program run with that version as an option.

#### Scenario: Program at a past version
- **WHEN** `@version "main~1" . ?- ex:status(?s, ?v).` runs
- **THEN** it returns the statuses as they were one commit ago

#### Scenario: Materialization reads the present only
- **WHEN** a program with `@version "HEAD~1"` is materialized
- **THEN** materialization fails, because inferences describe the current state
