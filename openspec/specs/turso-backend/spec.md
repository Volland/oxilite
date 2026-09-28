# turso-backend Specification

## Purpose
Runs oxilite on Turso, the Rust reimplementation of SQLite, behind the same sans-IO contract as
every other backend, so the whole query surface works there and vector features become possible.

## Requirements

### Requirement: Turso backend
The system SHALL provide a backend crate that runs oxilite requests on the Turso engine, both
synchronously (for `Store`) and asynchronously (for `AsyncStore`), on an in-memory database or a
database file, and SHALL run an atomic request inside one savepoint that is rolled back entirely
when any statement fails.

#### Scenario: Round trip on Turso
- **WHEN** a store is opened on an in-memory Turso database, Turtle is loaded and a SPARQL query
  with a join, a FILTER, an OPTIONAL and an aggregate runs
- **THEN** it returns the same solutions the bundled SQLite backend returns

#### Scenario: Failed atomic request changes nothing
- **WHEN** an atomic request whose second statement fails is executed
- **THEN** the first statement's effect is gone

#### Scenario: File store survives reopening
- **WHEN** quads are written to a Turso database file, the store is dropped and the file reopened
- **THEN** the quads are there

### Requirement: The full query surface on Turso
The system SHALL support on Turso every SPARQL construct, SPARQL Update operation, Cypher
statement and Datalog program the bundled SQLite backend supports, except the FTS5 text index,
including property paths, schema-registry scoping and recursive Datalog.

#### Scenario: Property path
- **WHEN** `SELECT ?x ?y WHERE { ?x ex:knows+ ?y }` runs on a chain of three people
- **THEN** it returns the three reachable pairs

#### Scenario: Cypher and Datalog
- **WHEN** a Cypher `CREATE` and `MATCH`, and a recursive Datalog program, run on a Turso store
- **THEN** they return what they return on the bundled SQLite backend

### Requirement: Dialect adaptation
The backend SHALL adapt DDL that Turso rejects without changing its meaning: a `WITHOUT ROWID,
STRICT` table SHALL be created as a `STRICT` rowid table whose primary key is enforced as a unique
index, so that its secondary indexes can be created. Statements creating the FTS5 text index SHALL
fail with an unsupported-feature error naming the text index.

#### Scenario: Schema creates on Turso
- **WHEN** a store with the graph index is created on Turso
- **THEN** `quads` and its `posg`, `ospg` and `gspo` indexes exist, and inserting a duplicate quad
  changes nothing

#### Scenario: Text index is refused clearly
- **WHEN** a store is created on Turso with the text index option
- **THEN** opening fails with an unsupported error that mentions the full-text index

### Requirement: Declared vector capabilities
The Turso backend SHALL declare that vector functions and index methods are available, and every
other backend SHALL declare them unavailable.

#### Scenario: Capabilities
- **WHEN** the capabilities of a Turso backend and of the bundled SQLite backend are read
- **THEN** only the Turso backend reports `vectors` and `vector_index_methods`

### Requirement: Opening a Turso store
The umbrella crate SHALL provide, behind a `turso` feature, constructors for a Turso store in
memory and on a file, with and without store options.

#### Scenario: Convenience constructors
- **WHEN** `Store::new_turso()` and `Store::open_turso(path)` are called
- **THEN** each returns a working store whose backend is Turso
