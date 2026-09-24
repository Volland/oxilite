# verifiable-credentials Specification

## Purpose
A Verifiable Credentials profile on top of JSON-LD document storage. Credentials and presentations are stored verbatim under their credential id, their RDF goes into a named graph equal to that id, and credential metadata is indexed for fast lookup without SPARQL.

## Requirements

### Requirement: Credential id as key and named graph
By default, the system SHALL store a credential under its top-level `id` as the document key and write its RDF into the named graph whose IRI is that `id`. The key strategy and graph strategy MUST be overridable with the same options as for generic JSON-LD documents.

#### Scenario: Default credential storage
- **WHEN** a VCDM 2.0 credential with `"id": "urn:uuid:3978344f-8596-4c3a-a978-8fcaba3903c5"` is stored with `put_credential`
- **THEN** `get_credential("urn:uuid:3978344f-8596-4c3a-a978-8fcaba3903c5")` returns the original JSON, and `GRAPH <urn:uuid:3978344f-8596-4c3a-a978-8fcaba3903c5> { ?vc a <https://www.w3.org/2018/credentials#VerifiableCredential> }` matches

#### Scenario: Overridden key
- **WHEN** the key strategy is the JSON Pointer `/credentialSubject/id` and the graph strategy is the template `https://example.org/holders/{key}`
- **THEN** the credential is stored under the subject's id and its triples are in the templated graph

### Requirement: Credentials without an id
The system SHALL apply a configurable missing-id policy to credentials that have no `id` (the field is optional in VCDM 2.0). The policies are: a content-hash key (the default), rejecting the credential, or requiring a caller-supplied key. The content-hash key MUST be stable for byte-identical input.

#### Scenario: Default content-hash key
- **WHEN** a credential without `id` is stored with default options
- **THEN** it is stored under `urn:oxilite:doc:sha256:<hex of its bytes>` and `put_credential` returns that key

#### Scenario: Reject policy
- **WHEN** the missing-id policy is "reject" and a credential without `id` is stored
- **THEN** the store fails with a missing-id error and writes nothing

### Requirement: Structural credential checks
The system SHALL accept only documents that parse as a VCDM v1.1 or v2.0 credential or presentation. The `@context` MUST begin with the matching base context, `type` MUST include `VerifiableCredential` or `VerifiablePresentation`, and `issuer` and `credentialSubject` MUST be present on credentials. It SHALL record which data model version was detected. Cryptographic proof verification is NOT performed; storing a credential MUST NOT be read as a claim that it is valid.

#### Scenario: Missing base context
- **WHEN** a document with `"type": ["VerifiableCredential"]` whose first context is not a W3C credentials context is stored with `put_credential`
- **THEN** the store fails with an invalid-credential error that names the violated rule, and nothing is written

#### Scenario: Version detection
- **WHEN** a credential using `https://www.w3.org/2018/credentials/v1` and a credential using `https://www.w3.org/ns/credentials/v2` are stored
- **THEN** their recorded profiles are `vc1` and `vc2` respectively

### Requirement: Bundled credential contexts
The system SHALL resolve the W3C credentials v1 and v2 contexts, the security, Data Integrity and common cryptosuite contexts, and the DID v1 context offline, without registration. Additional contexts MUST be addable through the generic context registry.

#### Scenario: Offline credential ingest on D1
- **WHEN** a VCDM 2.0 credential that uses only bundled contexts is stored through the D1 backend
- **THEN** the store succeeds without any network request

### Requirement: Proof graphs owned by the credential
The system SHALL store a credential's embedded proofs in their own graphs, as JSON-LD `@graph` containers require. It SHALL record those graphs as owned by the credential, so replacing or removing the credential removes its proof triples. By default the proof graphs MUST NOT be merged into the credential graph.

#### Scenario: Proof removed with credential
- **WHEN** a credential with a Data Integrity proof is stored and then removed
- **THEN** no quad from the credential graph or its proof graph remains

#### Scenario: Claims queried without proof noise
- **WHEN** `SELECT * WHERE { GRAPH <credential-id> { ?s ?p ?o } }` is run on a credential that has a proof
- **THEN** the results contain the credential's claims and a link to the proof node, but not the proof's `proofValue`

### Requirement: Presentations own embedded credentials
The system SHALL store a presentation with `put_presentation` under the same key and graph rules. When the embed option is enabled (the default), each credential embedded in `verifiableCredential` SHALL also be stored as its own credential document, keyed by its id. The presentation MUST record references to those keys.

#### Scenario: Embedded credentials stored individually
- **WHEN** a presentation that embeds two credentials with ids is stored with default options
- **THEN** `get_credential` returns each embedded credential, and `find_credentials` finds them by issuer

### Requirement: Indexed credential metadata
The system SHALL extract `issuer` (id), the credential subject ids, `type`, the validity start (`validFrom` or `issuanceDate`) and end (`validUntil` or `expirationDate`), and the profile into columns of the document row. Which of these columns are indexed MUST be configurable when the store is created. The defaults index issuer, subject and validity end.

#### Scenario: Metadata extraction across versions
- **WHEN** a v1.1 credential with `issuanceDate`/`expirationDate` and a v2.0 credential with `validFrom`/`validUntil` are stored
- **THEN** both rows have their validity start and end populated as timestamps

#### Scenario: Index configuration
- **WHEN** a store is created with metadata indexing disabled
- **THEN** the metadata columns are still populated, but no index on them exists

### Requirement: Credential lookup without SPARQL
The system SHALL provide `find_credentials(filter)`. The filter supports any combination of issuer, subject, type, "valid at instant" and profile, and results are paged by key. It MUST return keys and raw documents, answered from the document table in one request.

#### Scenario: Valid credentials of an issuer
- **WHEN** three credentials from `did:example:issuer` are stored, one of them expired, and `find_credentials` is called with that issuer and "valid at now"
- **THEN** exactly the two unexpired credentials are returned

#### Scenario: Combine with SPARQL
- **WHEN** a SPARQL query selects `?g` for graphs where `?s schema:degree ?d`, and each `?g` is passed to `document_for_graph`
- **THEN** the raw credentials holding a degree claim are returned
