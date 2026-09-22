# quad-storage Specification

## Purpose
Persists RDF quads and named graphs in SQLite, with an API that mirrors Oxigraph's `Store` for inserting, removing, scanning, loading and dumping.

## Requirements

### Requirement: Schema creation
The system SHALL create its schema idempotently when opening a store, and SHALL record a schema version. Opening an existing store MUST NOT modify its data.

#### Scenario: Reopen keeps data
- **WHEN** a store is opened, receives quads, is closed and reopened
- **THEN** all quads are still present and the schema version is unchanged

#### Scenario: Optional graph index
- **WHEN** a store is created with the graph index disabled
- **THEN** only the spog, posg and ospg orderings of quads are indexed

### Requirement: Insert and remove quads
The system SHALL insert and remove quads idempotently. `insert` MUST report whether the quad was new, and `remove` MUST report whether it was present.

#### Scenario: Duplicate insert
- **WHEN** the same quad is inserted twice
- **THEN** the first insert reports true, the second reports false, and the store contains it once

#### Scenario: Remove absent quad
- **WHEN** a quad that is not stored is removed
- **THEN** removal reports false and the store is unchanged

### Requirement: Atomic batch insert
The system SHALL insert a collection of quads with `extend` atomically: either all quads are stored or none.

#### Scenario: Failure rolls back extend
- **WHEN** an `extend` call fails part-way (for example on a hash collision)
- **THEN** none of its quads are stored

### Requirement: Pattern scans
The system SHALL return every stored quad that matches a pattern in which any of subject, predicate, object and graph is bound or unbound.

#### Scenario: All sixteen bound/unbound combinations
- **WHEN** `quads_for_pattern` is called with each combination of bound and unbound positions
- **THEN** the result equals filtering all stored quads by the bound positions

### Requirement: Named graphs
The system SHALL track named graphs, including empty ones added explicitly. It SHALL support listing, checking, adding, clearing and removing them, and the default graph MUST always exist.

#### Scenario: Empty named graph is listed
- **WHEN** a named graph is added explicitly without quads
- **THEN** it appears in the list of named graphs

#### Scenario: Remove named graph
- **WHEN** a named graph is removed
- **THEN** its quads are deleted and it no longer appears in the list

### Requirement: Load and dump
The system SHALL load data from any RDF format supported by `oxrdfio` (Turtle, TriG, N-Triples, N-Quads, RDF/XML, JSON-LD, N3), and SHALL dump the whole dataset or a single graph to any serializable format. Loading MUST be atomic for `load_from_reader`; the bulk loader MAY commit in chunks.

#### Scenario: Turtle round-trip
- **WHEN** a Turtle document is loaded and dumped as N-Triples
- **THEN** the dumped graph is isomorphic to the loaded one

#### Scenario: Load into a named graph
- **WHEN** a Turtle document is loaded with a target graph name
- **THEN** all its triples are stored in that graph

### Requirement: Size and emptiness
The system SHALL report the number of stored quads and whether the store is empty.

#### Scenario: Count after inserts
- **WHEN** three distinct quads are inserted
- **THEN** `len` returns 3 and `is_empty` returns false
