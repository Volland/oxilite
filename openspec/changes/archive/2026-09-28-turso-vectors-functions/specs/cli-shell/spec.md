## Purpose

Makes Turso stores, vector indexes and host functions reachable from the command line.

## ADDED Requirements

### Requirement: Turso stores from the command line
Every command that opens a store SHALL accept `--turso`, opening the store (in memory or the named
file) on the Turso engine.

#### Scenario: Query a Turso file
- **WHEN** `oxilite query --turso kg.db 'SELECT …'` runs
- **THEN** the query runs on the Turso store in `kg.db`

### Requirement: Vector commands
The shell SHALL provide `.vector list`, `.vector create NAME PROPERTY DIMENSIONS [METRIC]
[ELEMENT] [--class CLASS]`, `.vector drop NAME` and `.vector search NAME QUERY [K]`, where PROPERTY
and CLASS take the forms GRAPH takes and QUERY is a JSON array or a node, and `.functions` listing
host functions. On a backend without vector functions the vector commands SHALL fail with a message
saying a Turso store is needed.

#### Scenario: Create and search
- **WHEN** a Turso shell session loads embeddings, runs `.vector create docs ex:embedding 3` and
  `.vector search docs [1,0.1,0] 2`
- **THEN** it prints the two nearest nodes with their distances and scores

#### Scenario: Not on SQLite
- **WHEN** `.vector list` runs on a bundled SQLite store
- **THEN** it prints that vector indexes need a Turso store
