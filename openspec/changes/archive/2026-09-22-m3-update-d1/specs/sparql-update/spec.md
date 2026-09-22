## Purpose

Applies SPARQL 1.1 Update operations atomically on every backend, including D1, where the only transaction is a single batch of statements.

## ADDED Requirements

### Requirement: Update operations
The system SHALL execute these operations with SPARQL 1.1 semantics:
- INSERT DATA and DELETE DATA
- DELETE/INSERT … WHERE, DELETE WHERE, and USING / USING NAMED
- CLEAR, DROP and CREATE (DEFAULT, NAMED, ALL, or a graph IRI)
- ADD, MOVE and COPY

LOAD MUST report an error, because loading needs network access that the core doesn't have.

#### Scenario: Delete/insert rename
- **WHEN** `DELETE { ?s ex:old ?o } INSERT { ?s ex:new ?o } WHERE { ?s ex:old ?o }` runs
- **THEN** every `ex:old` triple is replaced by the matching `ex:new` triple

#### Scenario: Create existing graph
- **WHEN** `CREATE GRAPH <g>` runs and `<g>` already exists
- **THEN** an error is returned, unless SILENT is given

### Requirement: Where before modification
The system SHALL evaluate the WHERE clause of a DELETE/INSERT completely before applying any deletion or insertion from it.

#### Scenario: Insert does not see its own deletes
- **WHEN** an update deletes and re-inserts triples matched by its own WHERE clause
- **THEN** the result equals SPARQL's evaluate-then-apply semantics

### Requirement: Atomic update requests
The system SHALL apply a whole update request atomically, as one transaction on native backends and one batch on D1, with each operation seeing the effects of the previous ones. It MUST NOT need interactive read-then-write transactions.

#### Scenario: Failure rolls back
- **WHEN** the third operation of an update request fails
- **THEN** the effects of the first two operations are not visible afterwards

### Requirement: Fresh blank nodes
The system SHALL create distinct blank nodes for each solution of an INSERT template. A blank-node label used several times within a template MUST map to the same new node within one solution.

#### Scenario: One new node per solution
- **WHEN** `INSERT { ?s ex:addr _:a . _:a ex:city ?c } WHERE { ?s ex:city ?c }` runs over two subjects
- **THEN** two distinct new blank nodes are created, each with its own `ex:city`

### Requirement: Conformance
The system SHALL pass the W3C SPARQL 1.1 update test suite on native backends and on D1. Every failure MUST be allow-listed with a reason.

#### Scenario: Suite gate
- **WHEN** the W3C update suite runs in the compatibility harness
- **THEN** there are no divergences that aren't allow-listed
