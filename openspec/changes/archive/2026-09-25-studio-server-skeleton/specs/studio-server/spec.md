## Purpose

The language server behind oxilite studio: the engine in its own process, driven over LSP.

## ADDED Requirements

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
