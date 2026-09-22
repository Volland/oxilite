## Purpose

Defines how oxilite reaches a SQLite engine: one I/O-free contract, with native backends that link SQLite or load a user-supplied SQLite library at runtime.

## ADDED Requirements

### Requirement: Backend contract
The system SHALL express every operation as requests. A request is an ordered list of SQL statements marked either read-only or atomic, and gets back one result set per statement. The core MUST NOT perform I/O itself.

#### Scenario: Atomic request
- **WHEN** a backend executes an atomic request in which one statement fails
- **THEN** no statement of the request takes effect and an error is returned

### Requirement: Backend capabilities
The system SHALL let each backend declare these capabilities:
- maximum statement length
- maximum statements per request
- availability of oxilite user-defined functions
- availability of interactive transactions
- whether 64-bit integers must be returned as text

Generated SQL MUST respect the declared limits.

#### Scenario: Statement length limit
- **WHEN** a backend declares a maximum statement length and a large insert is performed
- **THEN** no generated statement exceeds that length

#### Scenario: No bound parameters needed
- **WHEN** any operation is compiled
- **THEN** its statements are self-contained and don't depend on bound parameters

### Requirement: Bundled native backend
The system SHALL provide a native backend on an in-process SQLite that supports in-memory and file databases. It SHALL register the oxilite user-defined functions (regular expressions, replace, hashes, Unicode case mapping).

#### Scenario: In-memory store
- **WHEN** a store is created without a path
- **THEN** it works entirely in memory and its data is gone when it's dropped

### Requirement: Dynamically loaded SQLite
The system SHALL provide a native backend that loads a SQLite shared library from a path supplied at runtime, without linking SQLite at build time.

#### Scenario: Open with a library path
- **WHEN** a store is opened with the path of a `libsqlite3` shared library and a database path
- **THEN** all store operations work through that library

#### Scenario: Invalid library
- **WHEN** the path doesn't point to a usable SQLite library
- **THEN** opening fails with an error naming the missing library or symbol
