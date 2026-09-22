# reasoning Specification

## Purpose
Answers queries with RDFS and OWL entailments, either by rewriting queries against a precomputed schema closure or by explicitly materializing OWL 2 RL inferences.

## Requirements

### Requirement: Reasoning is opt-in per query
The system SHALL evaluate queries without entailment by default. It SHALL apply RDFS or OWL-QL entailment only when a query asks for it.

#### Scenario: Default has no inference
- **WHEN** `ex:Dog rdfs:subClassOf ex:Animal` and `ex:rex a ex:Dog` are stored and `?x a ex:Animal` is queried without reasoning
- **THEN** no solution is returned

#### Scenario: RDFS subclass entailment
- **WHEN** the same query runs with RDFS reasoning
- **THEN** `ex:rex` is returned

### Requirement: Schema closure
The system SHALL maintain a closure of the schema, on `optimize()` and after updates that change schema triples:
- transitive subClassOf and subPropertyOf
- equivalentClass and equivalentProperty
- inverseOf, symmetric properties and transitive properties
- domain and range

Queries with reasoning MUST reflect schema triples as of the last closure computation.

#### Scenario: Transitive subclass chain
- **WHEN** A ⊑ B ⊑ C is stored, the closure is computed, and `?x a C` is queried with reasoning
- **THEN** instances of A are returned

### Requirement: Rewriting keeps single statements
The system SHALL implement query-time reasoning by rewriting patterns against the closure, and it MUST NOT add round-trips.

#### Scenario: Request count with reasoning
- **WHEN** a BGP query runs with RDFS reasoning
- **THEN** it uses the same number of requests as without reasoning

### Requirement: OWL 2 RL materialization
The system SHALL provide an explicit operation that computes all OWL 2 RL inferences and stores them separately from asserted data. Queries can include those inferences on request. Re-running the operation MUST replace the previous inferences.

#### Scenario: Materialize then query
- **WHEN** `materialize()` runs on data using `owl:sameAs` and a query includes inferences
- **THEN** facts entailed through `owl:sameAs` are returned

#### Scenario: Agreement with reference reasoner
- **WHEN** `materialize()` runs on a sample ontology on a native backend
- **THEN** the inferred triples equal those computed by the `reasonable` crate
