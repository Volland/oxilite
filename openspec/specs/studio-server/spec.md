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
