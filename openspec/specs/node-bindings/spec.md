# node-bindings Specification

## Purpose
Provides oxilite to Node.js applications with a typed API that mirrors Oxigraph's JavaScript `Store`, backed by SQLite files.

## Requirements

### Requirement: Store lifecycle
The package SHALL open an in-memory store, a store backed by a SQLite file, or a store using a SQLite shared library whose path is given at runtime.

#### Scenario: File store persists
- **WHEN** a store is opened on a file path, receives data, and is reopened in a new process
- **THEN** the data is present

### Requirement: Oxigraph-compatible API
The package SHALL provide `query`, `update`, `load`, `dump`, `add`, `delete`, `has`, `match` and `size` with the same names, argument shapes and result shapes as Oxigraph's JS `Store`:
- SELECT: an array of `Map`s from variable name to term
- ASK: a boolean
- CONSTRUCT and DESCRIBE: an array of quads
- Terms: RDF/JS-compatible objects

#### Scenario: SELECT result shape
- **WHEN** `store.query("SELECT ?s WHERE { ?s ?p ?o }")` runs on a store with one triple
- **THEN** it returns an array with one `Map` whose `s` entry has `termType` `NamedNode`

#### Scenario: Oxigraph JS tests
- **WHEN** Oxigraph's `store.test.ts` runs against the package with only the import changed
- **THEN** all tests pass, except allow-listed differences

### Requirement: Type definitions
The package SHALL ship TypeScript declarations for every exported class, function and result type.

#### Scenario: Strict compile
- **WHEN** an application using the package compiles with `tsc --strict`
- **THEN** it compiles without type errors or `any` leaking from the package

### Requirement: Extras
The package SHALL expose `explain(query)`, which returns the generated SQL and notes, and `optimize()`.

#### Scenario: Explain
- **WHEN** `explain` is called with a SELECT query
- **THEN** the returned object contains the SQL text
