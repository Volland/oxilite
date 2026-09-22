## ADDED Requirements

### Requirement: Generated node ids
The system SHALL provide an inline GeneratedNode tag whose 59-bit payload fully determines the IRI `urn:oxilite:n:<payload as 15 lowercase hex digits>`. Ids with this tag MUST NOT need a `terms` row. The encoder MUST map any IRI of that form to the GeneratedNode id, so that one IRI never has two ids.

#### Scenario: Generated id decodes without a lookup
- **WHEN** a GeneratedNode id is produced in SQL and decoded
- **THEN** the IRI is computed from the id alone, without reading `terms`

#### Scenario: Same IRI, same id
- **WHEN** the IRI `urn:oxilite:n:00000000000002a` is loaded from Turtle
- **THEN** it gets the GeneratedNode id with payload 42, not a hashed `Iri` id
