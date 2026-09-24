## Purpose

Scopes the schema closure to the ontologies the store has been told about, without changing what
reasoning means for a store that has told it nothing.

## MODIFIED Requirements

### Requirement: Schema closure
The system SHALL maintain a closure of the schema, on `optimize()` and after updates that change
schema triples:
- transitive subClassOf and subPropertyOf
- equivalentClass and equivalentProperty
- inverseOf, symmetric properties and transitive properties
- domain and range

The closure SHALL be computed from the active registered ontology graphs when any is registered,
and from every graph otherwise. Registering, deactivating or dropping an ontology graph MUST
update the closure. Queries with reasoning MUST reflect schema triples as of the last closure
computation.

#### Scenario: Transitive subclass chain
- **WHEN** A ⊑ B ⊑ C is stored, the closure is computed, and `?x a C` is queried with reasoning
- **THEN** instances of A are returned

#### Scenario: Reasoning scoped to one ontology
- **WHEN** two ontology graphs are registered, one stating `ex:Dog rdfs:subClassOf ex:Animal` and
  the other `ex:Rock rdfs:subClassOf ex:Animal`, the second is deactivated, and `?x a ex:Animal`
  is queried with RDFS reasoning
- **THEN** instances of `ex:Dog` are returned and instances of `ex:Rock` are not

#### Scenario: Axioms outside registered ontologies are ignored
- **WHEN** one ontology graph is registered and a subClassOf axiom is written to a different,
  unregistered graph
- **THEN** that axiom does not entail anything

## ADDED Requirements

### Requirement: Reactivating an ontology restores its entailments
The system SHALL recompute the closure when a registration changes, so that activating a
previously deactivated ontology makes its entailments available again without reloading data.

#### Scenario: Deactivate then reactivate
- **WHEN** an ontology graph is deactivated and then activated again
- **THEN** queries with reasoning return the same solutions as before the deactivation
