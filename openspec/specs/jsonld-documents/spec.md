# jsonld-documents Specification

## Purpose
Stores JSON-LD documents in oxilite exactly as received, converts each one to RDF in its own named graph, and keeps the raw document and its quads consistent, so documents can be retrieved verbatim and queried with SPARQL.

## Requirements

### Requirement: Raw document storage
The system SHALL store each JSON-LD document it accepts in a document table, together with its key, the name of the graph its RDF was written to, and a SHA-256 hash of the stored bytes. `get_document(key)` MUST return the document byte-for-byte as it was supplied, not re-serialized, compacted or re-ordered.

#### Scenario: Round-trip is verbatim
- **WHEN** a document with unusual key order, whitespace and Unicode escapes is stored and then read back by its key
- **THEN** the returned bytes are identical to the input and the reported hash is the SHA-256 of those bytes

#### Scenario: Unknown key
- **WHEN** `get_document` is called with a key that was never stored
- **THEN** it returns "not found" and the store is unchanged

### Requirement: Configurable document key
The system SHALL derive each document's key using a configurable key strategy. The strategies are: the document's top-level `@id`/`id` (the default), a JSON Pointer into the document, a content hash, or a caller-supplied key. When the chosen strategy yields no value, the configured missing-key policy MUST apply: reject the document, or fall back to a content-hash key of the form `urn:oxilite:doc:sha256:<hex>`.

#### Scenario: Default key is the document id
- **WHEN** a document with `"id": "https://example.org/docs/1"` is stored with default options
- **THEN** its key is `https://example.org/docs/1`

#### Scenario: JSON Pointer key
- **WHEN** the key strategy is the JSON Pointer `/credentialSubject/id` and a document with that field is stored
- **THEN** its key is the value at that pointer

#### Scenario: Missing key rejected
- **WHEN** the missing-key policy is "reject" and a document has no value for the key strategy
- **THEN** the store fails with a missing-key error and writes nothing

#### Scenario: Content-hash fallback
- **WHEN** the missing-key policy is "content hash" and a document without an id is stored twice
- **THEN** both stores produce the same `urn:oxilite:doc:sha256:<hex>` key and the document exists once

### Requirement: Configurable target graph
The system SHALL write a document's RDF into a graph chosen by a configurable graph strategy. The strategies are: the key as a named graph (the default), an IRI template containing `{key}` (percent-encoded on substitution), one fixed named graph, or the default graph. When the key strategy produces a key that is not an absolute IRI, the key-as-graph strategy MUST fail with an invalid-graph-name error.

#### Scenario: Key used as named graph
- **WHEN** a document with key `urn:uuid:1234` is stored with default options
- **THEN** every triple produced from the document's default graph is stored in named graph `<urn:uuid:1234>`

#### Scenario: Template graph
- **WHEN** the graph strategy is the template `https://example.org/g/{key}` and the key is `a b`
- **THEN** the triples are stored in graph `<https://example.org/g/a%20b>`

#### Scenario: Default-graph strategy
- **WHEN** the graph strategy is "default graph"
- **THEN** the document's triples are stored in the default graph and the document row records the default graph as its target

### Requirement: JSON-LD to RDF conversion
The system SHALL convert each document to RDF following the JSON-LD 1.1 Processing Algorithms (expansion followed by deserialization to RDF), using the document's contexts. Invalid JSON, invalid JSON-LD, and contexts that cannot be loaded MUST each fail with a distinct error that includes the JSON-LD error code where one exists. A failed store MUST write nothing.

#### Scenario: Conformance with the W3C toRdf suite
- **WHEN** the W3C JSON-LD 1.1 `toRdf` tests that need no network access are run through `put_document` with the default-graph strategy
- **THEN** the stored quads are isomorphic to the expected output, except for tests listed with a reason in the allowlist

#### Scenario: Invalid JSON-LD rejected atomically
- **WHEN** a document with an invalid `@context` (for example, a number) is stored
- **THEN** the store fails with a JSON-LD processing error that carries its JSON-LD error code, and neither the document nor any quad is stored

### Requirement: Nested named graphs are owned by the document
The system SHALL store the named graphs that a document itself defines (via `@graph` containers or a top-level `@graph` with an `@id`), and SHALL record them as owned by the document. Blank-node graph names MUST be kept as blank-node graph names, scoped to the document as described in "Blank-node isolation". The list of owned graphs MUST be retrievable from the document key.

#### Scenario: Graph container
- **WHEN** a document whose `proof` property is declared `@container: @graph` is stored
- **THEN** the proof triples are stored in a separate blank-node graph and `document_graphs(key)` lists both the target graph and the proof graph

### Requirement: Blank-node isolation
The system SHALL give every blank node produced from a document a label derived from the document key and the blank node's position in the conversion. Blank nodes from different documents therefore MUST never be equal, and storing the same document again MUST produce the same labels.

#### Scenario: No cross-document merging
- **WHEN** two different documents that both contain an anonymous node `{"name": "x"}` are stored
- **THEN** a SPARQL query for subjects with name `"x"` returns two distinct blank nodes

#### Scenario: Idempotent re-store
- **WHEN** the same document is stored twice under the same key
- **THEN** the number of quads in the store is unchanged after the second store

### Requirement: Atomic replace and delete
Storing a document under an existing key SHALL replace it: the old document row, all quads in the graphs the old document owned, and the ownership records MUST be removed, and the new ones written, in one atomic request. `remove_document(key)` SHALL remove the row, the owned graphs' quads and the graph names in one atomic request. It MUST report whether the document existed.

#### Scenario: Replace drops stale triples
- **WHEN** a document stating `ex:a ex:p "1"` is replaced by a version stating `ex:a ex:p "2"`
- **THEN** the document's graph contains only `"2"`, and a failure part-way through leaves only `"1"` stored

#### Scenario: Remove
- **WHEN** a stored document is removed
- **THEN** `get_document` returns "not found", its owned graphs are empty and no longer listed as named graphs, and quads outside those graphs are untouched

#### Scenario: Remove absent document
- **WHEN** a key that is not stored is removed
- **THEN** removal reports false and the store is unchanged

### Requirement: Offline context loading
The system SHALL resolve remote `@context` IRIs without network access by default. Resolution uses, in order: contexts registered on the store handle, then contexts persisted in the store's context table. Network loading MUST be possible only when explicitly enabled, and only on native backends. A context that cannot be resolved MUST fail the store with a "loading remote context failed" error naming the IRI.

#### Scenario: Persisted context
- **WHEN** a context is saved with `put_context(iri, json)` and a document that references that IRI is stored from another process or from D1
- **THEN** the document is converted without network access

#### Scenario: Unknown context without network
- **WHEN** a document references a context IRI that is neither registered nor persisted, and network loading is disabled
- **THEN** the store fails with an error naming that IRI and writes nothing

### Requirement: SPARQL access to document RDF
The RDF of stored documents SHALL be queryable and updatable with the store's ordinary SPARQL interface, with no special syntax. The system SHALL provide a way to map graph names returned by SPARQL back to document keys and raw documents.

#### Scenario: Query a document's graph
- **WHEN** a document with key `urn:uuid:1234` is stored with default options and `SELECT ?p ?o WHERE { GRAPH <urn:uuid:1234> { ?s ?p ?o } }` is run
- **THEN** the results equal the triples of the document's RDF conversion

#### Scenario: From graph to document
- **WHEN** a SPARQL query binds `?g` to named graphs and each binding is passed to `document_for_graph`
- **THEN** each graph that is a document's target or owned graph returns that document's key and raw JSON, and every other graph returns "not found"

### Requirement: Graph consistency repair
The system SHALL provide `rebuild_graph(key)`, which regenerates a document's owned graphs from its stored raw JSON in one atomic request, and `check_documents()`, which reports each document whose owned graphs differ from a fresh conversion (for example, after a SPARQL UPDATE edited them).

#### Scenario: Repair after SPARQL UPDATE
- **WHEN** a SPARQL UPDATE deletes a triple from a document's graph, and `check_documents()` and then `rebuild_graph(key)` are called
- **THEN** the check reports that document, and after the rebuild the graph again equals the conversion of the raw document

### Requirement: Opt-in schema
The JSON-LD tables SHALL be created only when a JSON-LD handle is first opened on a store. The existing tables and the schema version MUST be unchanged. Stores that never use JSON-LD MUST NOT contain the JSON-LD tables.

#### Scenario: Existing store upgraded in place
- **WHEN** a JSON-LD handle is opened on a store created by an earlier oxilite version
- **THEN** the JSON-LD tables are added, all existing quads are still present, and `schema_version` is unchanged
