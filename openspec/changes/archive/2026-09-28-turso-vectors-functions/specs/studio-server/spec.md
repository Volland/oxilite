## Purpose

Gives the studio what it needs to show, create, search and drop vector indexes and to offer host
functions, on any connection.

## ADDED Requirements

### Requirement: Turso connections
`oxilite/attach` SHALL accept `engine: "turso"`, attaching a Turso database file as a connection
of kind `turso` that every connection request can target.

#### Scenario: Attach and query
- **WHEN** a Turso file is attached and `oxilite/query` targets it
- **THEN** the query runs on that file

### Requirement: Vector index requests
The server SHALL answer `oxilite/vectorIndexes` with whether the target supports vector indexes and
each index's name, IRI, property, dimensions, metric, element type, class, row count and built
state; `oxilite/vectorIndexCreate` and `oxilite/vectorIndexDrop` SHALL create and drop an index on
the target; `oxilite/vectorSearch` SHALL search an index with a vector or a node and `k`, returning
hits (node as RDF/JS, distance, score) and the elapsed time. Each SHALL accept an optional
`connection`.

#### Scenario: Unsupported target
- **WHEN** `oxilite/vectorIndexes` targets the project store (bundled SQLite)
- **THEN** it returns `supported: false` and no indexes

#### Scenario: Create, list, search, drop
- **WHEN** an index is created on an attached Turso connection holding embeddings, listed, searched
  and dropped
- **THEN** the listing shows it with its row count, the search returns the nearest nodes first, and
  after the drop the listing is empty

### Requirement: Host function listing
The server SHALL answer `oxilite/functions` with the host functions registered on the server's
stores (IRI, Cypher name, arity, description), for completion and hover.

#### Scenario: No functions
- **WHEN** no function is registered
- **THEN** `oxilite/functions` returns an empty list
