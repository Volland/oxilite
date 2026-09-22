## Purpose

Executes Cypher write clauses atomically, as one request per statement on every backend, including a single D1 batch.

## ADDED Requirements

### Requirement: Write clauses
The system SHALL execute `CREATE`, `MERGE` (with `ON CREATE SET` and `ON MATCH SET`), `SET`, `REMOVE`, `DELETE` and `DETACH DELETE` with openCypher semantics.

#### Scenario: MERGE is idempotent
- **WHEN** `MERGE (p:Person {name: 'Ada'})` runs twice
- **THEN** exactly one matching node exists

#### Scenario: DETACH DELETE removes relationships
- **WHEN** `MATCH (n {name: 'Ada'}) DETACH DELETE n` runs on a node that has incoming and outgoing relationships, some with reifiers
- **THEN** no quad mentions the node, and no reifier of its relationships remains

### Requirement: Atomic statements
The system SHALL apply the changes of each Cypher statement that writes as one atomic request, so a failure leaves the store unchanged. The statement's reading part is evaluated before any modification; on backends with interactive transactions the read and the write MUST share one transaction.

#### Scenario: DELETE of a connected node fails atomically
- **WHEN** `MATCH (n {name: 'Ada'}) SET n.age = 36 DELETE n` runs on a node that still has relationships
- **THEN** the statement fails with a constraint error, and `n.age` is unchanged

### Requirement: Fresh node ids without read-back
The system SHALL give each node created by `CREATE` or `MERGE` a fresh IRI, computed without reading from the store, even when many nodes are created per matched row. This MUST work on backends without UDFs. A created node MUST exist even without labels, properties or relationships.

#### Scenario: Per-row creation on D1
- **WHEN** `UNWIND range(1, 100) AS i CREATE (:Item {i: i})` runs on D1
- **THEN** 100 distinct nodes are created in one batch
