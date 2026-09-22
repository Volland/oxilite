## Why

Verifiable Credentials and most linked-data APIs exchange JSON-LD documents, not quads. Today an oxilite user must convert JSON-LD to RDF outside the store, choose graph names by hand, and keep the original JSON somewhere else. The original matters: it is what gets presented, re-signed or audited. oxilite should accept JSON-LD directly, keep the exact document, and make its RDF queryable with SPARQL. Credentials are the main use case, so the defaults should suit them.

## What Changes

- New crate **`oxilite-jsonld`** (generic JSON-LD, no VC knowledge). It uses the [`json-ld`](https://crates.io/crates/json-ld) 0.21 crate for context processing, expansion and JSON-LD→RDF (toRdf) conversion.
  - It adds a raw-document table `jsonld_documents` keyed by a **configurable document key**. The default key is the document's top-level `@id`/`id`.
  - Each document's RDF goes into a **named graph derived from the key** by default. The graph strategy is configurable: key as graph, IRI template, fixed graph, or default graph.
  - It adds `put_document`, `get_document`, `remove_document`, `list_documents` and `rebuild_graph`. Replacing and deleting a document is atomic (one D1 batch): the raw row and all the quads it produced change together.
  - Context loading is **offline by default**. Contexts come from a pluggable loader chain: contexts registered in memory, then contexts stored in a new `jsonld_contexts` table (so D1/wasm can resolve them), then network loading, which is opt-in and native-only.
  - Blank nodes get deterministic, per-document labels. Documents never share blank nodes, and re-inserting the same document is idempotent.
- New crate **`oxilite-vc`**, a Verifiable Credentials profile on top of `oxilite-jsonld`. It uses [`ssi-vc`](https://crates.io/crates/ssi-vc) (VCDM v1.1 and v2 typed credentials and presentations) and [`ssi-json-ld`](https://crates.io/crates/ssi-json-ld) (bundled W3C credential, security, DID and Data Integrity contexts).
  - `put_credential` / `put_presentation` parse and check the credential's structure. Both use the credential `id` as the document key and as the named graph by default.
  - Credentials without an `id` (optional in VCDM 2.0) get a configurable fallback. The default is a content-hash key `urn:oxilite:doc:sha256:<hex>`; the alternatives are rejecting the credential or requiring a caller-supplied key.
  - VC-optimized columns in `jsonld_documents`: `issuer`, `subject`, `types`, `valid_from`, `valid_until`, `profile`. Their indexes are configurable, so D1 stores that don't need them avoid the write cost.
  - `find_credentials(filter)` answers issuer, subject, type and validity questions from indexed columns, without SPARQL.
  - Proof graphs (`proof` is an `@graph` container) are stored as owned sub-graphs of the credential and removed with it. Presentations own the graphs of the credentials they embed.
- The RDF of every stored document is queried with ordinary SPARQL (`GRAPH <credential-id> { … }`). SPARQL results that bind a graph can be mapped back to raw documents.
- Store schema: two new tables and one ownership table, created on demand, so stores that never use JSON-LD are unchanged. `schema_version` is not bumped for existing stores.

## Capabilities

### New Capabilities
- `jsonld-documents`: storing, replacing, deleting and reading raw JSON-LD documents; configurable document key and graph strategy; conversion to RDF quads in named graphs; offline context loading; blank-node isolation; atomicity; mapping between documents and graphs; SPARQL access to document RDF.
- `verifiable-credentials`: VC/VP profile with credential-id keys and graphs, missing-id fallback, VCDM v1.1/v2 structure checks, proof and embedded-credential graph ownership, indexed credential metadata and `find_credentials`.

### Modified Capabilities
<!-- None: quad storage, query and update requirements are unchanged; the new tables are additive. -->

## Impact

- **New crates**: `crates/oxilite-jsonld` (depends on `oxilite-core`, `json-ld`, `json-syntax`, `iref`, `rdf-types`, `sha2`) and `crates/oxilite-vc` (depends on `oxilite-jsonld`, `ssi-vc`, `ssi-json-ld`). Both are feature-gated in the `oxilite` umbrella (`jsonld`, `vc`), so default builds and wasm size are unaffected.
- **oxilite-core**: small hooks only. The writer accepts a pre-encoded quad set with an additional "delete graphs first" prefix, and the schema module exposes the optional JSON-LD DDL. No compiler changes.
- **Backends**: the same sans-IO `Request`s, so the feature works on rusqlite, dylib and D1. Network context loading (`reqwest`) is available on native builds only.
- **Bindings**: `@oxilite/node` and `@oxilite/d1` exposure is a follow-up task and is gated on wasm size, because `ssi-*` is heavy.
- **Docs**: `lat.md/architecture.md` gets a JSON-LD section, `lat.md/decisions.md` records the key and graph defaults, and `lat.md/milestones.md` lists the change.
- **Out of scope**: proof or signature verification, status-list revocation checks, JSON-LD framing/compaction on read (the raw document is returned), and SHACL validation of credentials (see `m5-validation`).
