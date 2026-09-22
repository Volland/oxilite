## Purpose

Offers optional full-text search over string literals using SQLite FTS5, which works both natively and on D1.

## ADDED Requirements

### Requirement: Opt-in text index
The system SHALL create a full-text index over string and language-tagged literals only when it's enabled, and SHALL keep it consistent with inserts and deletes of those literals.

#### Scenario: Disabled by default
- **WHEN** a store is created without enabling text search
- **THEN** no full-text index exists and writes don't pay for one

### Requirement: Text match in SPARQL
The system SHALL provide a SPARQL extension function that matches a literal against a full-text query, compiled to an index lookup when the index exists.

#### Scenario: Word match
- **WHEN** literals "graph database" and "relational store" exist and the text function searches for "graph"
- **THEN** only the first literal matches
