# datalog-rules Specification

## Purpose
Makes a rule program a materialization source, so user-authored rules extend the reasoner instead
of running beside it.

## Requirements

### Requirement: Materializing a rule program
The system SHALL provide an operation that evaluates a Datalog program and stores the facts it
derives as inferences, separately from asserted data, in the same inference store that OWL 2 RL
materialization uses. Re-running the operation MUST replace the previous inferences.

#### Scenario: Derived facts become queryable
- **WHEN** a program deriving `ex:ancestor` is materialized and a SPARQL query that includes
  inferences asks for `?x ex:ancestor ?y`
- **THEN** the derived pairs are returned

#### Scenario: Asserted data is untouched
- **WHEN** a program is materialized and then the inferences are discarded
- **THEN** the asserted triples are exactly what they were before

#### Scenario: Re-running replaces
- **WHEN** a program is materialized, a rule is changed, and it is materialized again
- **THEN** only the facts derivable from the new program are present

### Requirement: Materialized rules are visible to every dialect
The system SHALL make facts derived by a rule program readable through the same option that exposes
RDFS and OWL inferences, so SPARQL and Cypher see them without a dialect-specific switch.

#### Scenario: Cypher sees a derived relationship
- **WHEN** a program deriving `ex:reportsTo` is materialized and a Cypher query that includes
  inferences matches `-[:reportsTo]->`
- **THEN** the derived relationships are matched

#### Scenario: Default query is unaffected
- **WHEN** a program is materialized and a query runs without asking for inferences
- **THEN** no derived fact is returned

### Requirement: Rule heads must be assertable
The system SHALL materialize only rules whose head is a triple-shaped atom — an IRI predicate with
two arguments, or `triple/3` or `triple/4`. A program that materializes a head with an arity or
shape that has no RDF form MUST be rejected before anything is written.

#### Scenario: Binary head materializes
- **WHEN** `ex:ancestor(?x, ?y) :- ...` is materialized
- **THEN** each derived pair is stored as the triple `?x ex:ancestor ?y`

#### Scenario: Non-triple head is rejected
- **WHEN** a program whose head is `p(?x, ?y, ?z)` with no IRI predicate is materialized
- **THEN** it fails before any write, naming the offending head

### Requirement: Materialization is atomic
The system SHALL write a materialization result in one atomic request, so a backend without
interactive transactions is never left with a partial inference set.

#### Scenario: Atomic on D1
- **WHEN** materialization runs on the D1 backend
- **THEN** the reset and the derived facts are applied in one atomic request

### Requirement: One inference store, one lifecycle
The system SHALL document and enforce that rule materialization and OWL 2 RL materialization share
one inference store: running either replaces the whole inferred set.

#### Scenario: OWL materialization replaces rule inferences
- **WHEN** a rule program is materialized and OWL 2 RL materialization is then run
- **THEN** the rule-derived facts are gone, and the operation reports that it replaced them
