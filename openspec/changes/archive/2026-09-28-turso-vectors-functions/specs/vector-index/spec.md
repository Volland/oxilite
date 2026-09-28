## Purpose

Vector indexes over embeddings stored as RDF literals: defined as RDF in a system graph, realised
as trigger-maintained tables, and managed from the API, SPARQL Update, Cypher, the shell and the
studio.

## ADDED Requirements

### Requirement: Index definitions are RDF
The system SHALL describe each vector index in the named graph `<oxilite:vectors>` as the resource
`<oxilite:vector/NAME>` of type `oxl:VectorIndex`, with `oxl:indexName`, `oxl:property` (the IRI
whose literal values are embeddings), `oxl:dimensions` (a positive integer), `oxl:metric`
(`oxl:Cosine`, `oxl:Euclidean`, `oxl:DotProduct` or `oxl:Jaccard`), `oxl:elementType`
(`oxl:Float32`, `oxl:Float64`, `oxl:Int8`, `oxl:Bit1` or `oxl:SparseFloat32`) and an optional
`oxl:class`. Metric defaults to cosine and element type to Float32; Jaccard SHALL require
SparseFloat32 and SparseFloat32 SHALL require Jaccard.

#### Scenario: Definition is queryable
- **WHEN** an index is created through the API and `SELECT ?i WHERE { GRAPH <oxilite:vectors> { ?i
  a oxl:VectorIndex } }` runs
- **THEN** it returns `<oxilite:vector/NAME>`

#### Scenario: Invalid definitions are rejected
- **WHEN** an index is created with zero dimensions, a name outside `[A-Za-z][A-Za-z0-9_]{0,63}`,
  a name equal to an existing index's ignoring case, or Jaccard over dense vectors
- **THEN** creation fails naming the problem, and nothing is written

### Requirement: Embeddings are literals
The system SHALL read an embedding from the lexical form of a literal object of the indexed
property, written as a JSON array of numbers (`"[0.1, -2, 3e-4]"`), for every subject and graph.

#### Scenario: Any graph
- **WHEN** the same subject has an embedding in the default graph and in a named graph
- **THEN** both are indexed, and search returns the subject once at its nearest distance

### Requirement: Indexes stay current on every write
The system SHALL keep an index exact in the same transaction as every write to the indexed
property, whatever the write path: `INSERT DATA`, `DELETE DATA`, `DELETE/INSERT … WHERE`, `CLEAR`,
`DROP`, the store's insert and remove, the bulk loader, and Cypher writes.

#### Scenario: Insert and delete
- **WHEN** an embedding triple is inserted, searched for, and deleted
- **THEN** search finds the subject after the insert and not after the delete

#### Scenario: Rolled-back write leaves the index unchanged
- **WHEN** an update inserts an embedding and then fails later in the same atomic request
- **THEN** the index does not contain the embedding

### Requirement: Malformed embeddings abort the write
The system SHALL abort a write that gives the indexed property a value that is not a JSON array of
numbers, or that has a number of elements other than the index's dimensions, with an error naming
the index and the expected dimensions.

#### Scenario: Wrong dimensions
- **WHEN** a three-dimensional index exists and `INSERT DATA { ex:a ex:embedding "[1, 2]" }` runs
- **THEN** the update fails with an error mentioning the index and 3 dimensions, and `ex:a` has no
  embedding

#### Scenario: Creation over bad data
- **WHEN** an index is created over a property that already has a malformed value
- **THEN** creation fails and no index table, trigger or definition remains

### Requirement: Creation back-fills
The system SHALL index every existing value of the property when an index is created, in the same
atomic request that creates it.

#### Scenario: Existing data is searchable
- **WHEN** embeddings are loaded and then an index is created
- **THEN** a search immediately returns the loaded subjects

### Requirement: Index lifecycle from every write surface
The system SHALL create and drop indexes through the store API (`create_vector_index`,
`drop_vector_index`), through SPARQL Update of `<oxilite:vectors>`, through Cypher (`CREATE VECTOR
INDEX name [IF NOT EXISTS] FOR (n:Label) ON (n.prop) OPTIONS {indexConfig: {…}}` and `DROP INDEX
name [IF EXISTS]`), and SHALL list them through the API (`vector_indexes`, with row counts and
whether each is built) and Cypher (`SHOW VECTOR INDEXES`).

#### Scenario: SPARQL Update creates and drops
- **WHEN** a definition is inserted into `<oxilite:vectors>` with `INSERT DATA`, and later deleted
  with `DELETE WHERE`
- **THEN** after the insert the index is built and searchable, and after the delete its table is
  gone

#### Scenario: Cypher DDL
- **WHEN** `CREATE VECTOR INDEX docs FOR (d:Doc) ON (d.embedding) OPTIONS {indexConfig:
  {`vector.dimensions`: 3, `vector.similarity_function`: 'euclidean'}}` runs, then `SHOW VECTOR
  INDEXES`, then `DROP INDEX docs`
- **THEN** the index is created over the property the vocabulary maps `embedding` to, restricted to
  the class `Doc` maps to, listed with its dimensions and similarity, and then dropped

#### Scenario: IF NOT EXISTS and IF EXISTS
- **WHEN** an existing index is created again with `IF NOT EXISTS`, or a missing one dropped with
  `IF EXISTS`
- **THEN** the statement succeeds and changes nothing; without the clause it fails naming the index

### Requirement: Definitions and structures are reconciled
The system SHALL provide a sync operation that makes the built indexes match the definitions in
`<oxilite:vectors>`, creating what is missing, dropping what is no longer defined, and rebuilding
what is defined differently, and SHALL run it after any SPARQL update that writes to that graph and
when a writable store whose definitions and tables disagree is opened.

#### Scenario: Definition loaded from a dump
- **WHEN** a dump that includes `<oxilite:vectors>` is loaded into a fresh Turso store and
  `sync_vector_indexes` runs
- **THEN** the index is built from the loaded embeddings

#### Scenario: Changed definition is rebuilt
- **WHEN** an index's metric is changed by a SPARQL update of its definition
- **THEN** searches use the new metric

### Requirement: Backends without vector functions
The system SHALL refuse to create, sync or search a vector index on a backend that does not
declare vector functions, with an unsupported error that says a Turso backend is needed, and SHALL
not change the store.

#### Scenario: Bundled SQLite
- **WHEN** `create_vector_index` is called on a bundled SQLite store
- **THEN** it fails with an unsupported error mentioning Turso, and no quad is written

### Requirement: Sparse indexes use Turso's inverted-file index
The system SHALL create a `toy_vector_sparse_ivf` index method over the table of a SparseFloat32
index, and SHALL shape its search so the index method can serve it.

#### Scenario: Sparse search
- **WHEN** a sparse Jaccard index over bag-of-words vectors is searched
- **THEN** results are ordered by Jaccard distance
