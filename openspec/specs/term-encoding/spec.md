# term-encoding Specification

## Purpose
Defines how RDF terms become compact 64-bit integer ids, so storage and query compilation never need a dictionary lookup to encode a constant.

## Requirements

### Requirement: Deterministic term ids
The system SHALL assign every RDF term (IRI, blank node, literal, RDF 1.2 triple term) a positive 64-bit integer id that is a pure function of the term. The same term MUST get the same id on every client and in every database.

#### Scenario: Same term, same id across stores
- **WHEN** the IRI `http://example.com/a` is inserted into two independent stores
- **THEN** both stores record the same id for it

#### Scenario: Different kinds never share an id
- **WHEN** an IRI and a blank node have the same text
- **THEN** their ids differ, and each id falls in the range reserved for its kind

### Requirement: Inline values
The system SHALL encode canonical `xsd:integer` values in the range ±2^58, and canonical `xsd:boolean` values, directly in the id without storing a dictionary row. Inline integer ids MUST sort in the same order as the integer values.

#### Scenario: Integer range filter uses id order
- **WHEN** the integers -5, 0 and 42 are encoded
- **THEN** their ids are in increasing order and decode back to -5, 0 and 42

#### Scenario: Out-of-range integers are not inlined
- **WHEN** an `xsd:integer` greater than 2^58 − 1 is stored
- **THEN** it is stored as a hashed typed literal and still round-trips exactly

### Requirement: Term identity is preserved
The system SHALL keep non-canonical lexical forms as distinct terms (e.g. `"012"^^xsd:integer` ≠ `"12"^^xsd:integer`). A simple literal and the same lexical form typed `xsd:string` MUST be the same term.

#### Scenario: Non-canonical integer round-trips
- **WHEN** `"012"^^xsd:integer` is inserted and read back
- **THEN** the returned literal has lexical form `012`

#### Scenario: xsd:string equals simple literal
- **WHEN** `"abc"` and `"abc"^^xsd:string` are inserted as objects of the same subject and predicate
- **THEN** the store contains one quad

### Requirement: Collision detection
The system SHALL reject any write that would map a new term to an id already used by a different term. The whole atomic request that contains the write MUST be rolled back, and an error identifying a hash collision MUST be returned.

#### Scenario: Colliding insert is rejected atomically
- **WHEN** a batch inserts a term whose id already belongs to a different stored term
- **THEN** the batch fails with a collision error and none of its statements take effect

### Requirement: Typed value columns
The system SHALL store, for each hashed literal, the numeric value and numeric type rank (for numeric datatypes) and an epoch timestamp (for `xsd:dateTime` and `xsd:date`). Value comparisons MUST NOT need to re-parse lexical forms.

#### Scenario: Decimal comparison uses stored value
- **WHEN** `"2.50"^^xsd:decimal` is stored and a query filters `?x > 2`
- **THEN** the literal matches without being parsed at query time
