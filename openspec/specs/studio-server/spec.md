# studio-server Specification

## Purpose
The language server behind oxilite studio: the engine in its own process, driven over LSP.

## Requirements

### Requirement: Project store from workspace files
The server SHALL load every RDF file under the workspace root into a scratch store, each file into
the named graph of its `file:` IRI, skipping hidden directories, `node_modules` and `target`.

#### Scenario: Files load on initialize
- **WHEN** the server is initialized with a root holding two Turtle files
- **THEN** `oxilite/status` reports both files and the sum of their triples

### Requirement: Load errors are diagnostics
The server SHALL publish a diagnostic at the position of a syntax error and SHALL NOT load any
quad of that file.

#### Scenario: Broken Turtle
- **WHEN** a Turtle file has a syntax error on line 3
- **THEN** a diagnostic for that file's URI starts on line 3 (zero-based 2) and the file
  contributes no triples

### Requirement: Query over the union of files
`oxilite/query` SHALL run a SPARQL query with the default graph as the union of all graphs and
return the RDF/JS JSON payload, with at most the requested number of rows and a `truncated` flag.

#### Scenario: Select across files
- **WHEN** a query joins triples that live in two different files
- **THEN** the solutions contain the joined rows

### Requirement: Reload
`oxilite/reload` and watched-file changes SHALL rebuild the store from the files on disk and
notify `oxilite/storeChanged`.

#### Scenario: A new file appears
- **WHEN** a file is added and `oxilite/reload` is sent
- **THEN** its triples are queryable

### Requirement: Live SHACL diagnostics on source lines
The server SHALL validate the Project store in the background after every change and publish
each result as a diagnostic on the source line of the focus node's statement.

#### Scenario: Entailed type violates a shape
- **WHEN** RDFS reasoning makes an employee a person and the person shape requires a name
- **THEN** a diagnostic appears on the employee's line in its data file

### Requirement: Justifications
`oxilite/why` SHALL return a proof tree for an entailed triple, down to asserted statements
with their source lines.

#### Scenario: Rule conclusion
- **WHEN** a rule file derives a triple
- **THEN** the tree names the rule file and its rule, with the premises it matched

### Requirement: Project checks without the editor
`oxilite check` SHALL report load errors, SHACL results and manifest test outcomes, and exit
with status 1 when any fails.

#### Scenario: Failing test
- **WHEN** a manifest test's expected results differ from the actual ones
- **THEN** the check reports what is missing and fails

### Requirement: Registry of a connection
`oxilite/registry` SHALL return the schema registry of the named or active connection as oxilite's
reader sees it, the graphs of the store with their sizes, the `owl:imports` asserted in registered
ontology graphs, the registry's problems and the state of the system graphs.

#### Scenario: Manifest ontology on the Project store
- **WHEN** the manifest maps the ontology graph `g/onto` to `g/staff`
- **THEN** `oxilite/registry` lists `g/onto` with role `ontology` and `appliesTo` `[g/staff]`, and
  `g/staff` among the graphs with its triple count

### Requirement: Registry edits
`oxilite/registryEdit` SHALL register, add a role, remap, activate, deactivate, unregister and drop
schema graphs and install the system graphs through the registry API. An edit on an attached store
SHALL need `confirmed` like an update, and a read-only connection SHALL refuse it.

#### Scenario: Edit an attached store
- **WHEN** `registryEdit` registers a graph on an attached read-write store without `confirmed`
- **THEN** it fails with code 1001, and succeeds once re-sent with `confirmed`

#### Scenario: Add a role
- **WHEN** an ontology graph gets the role `shacl` with `addRole`
- **THEN** the registry lists it once per role with the same targets

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
