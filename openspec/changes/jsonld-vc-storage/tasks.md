## 1. Workspace and crates

- [ ] 1.1 Add `json-ld` 0.21, `json-syntax`, `iref`, `rdf-types`, `sha2`, `pollster`, `ssi-vc` 0.9 and `ssi-json-ld` 0.3 to `[workspace.dependencies]`, pinning `json-ld` to the version `ssi-json-ld` resolves to; check that `cargo tree -d` shows a single `json-ld`
- [ ] 1.2 Create `crates/oxilite-jsonld` (depends on `oxilite-core`; `network` feature enabling `json-ld/reqwest` on non-wasm only) and `crates/oxilite-vc` (depends on `oxilite-jsonld`, `ssi-vc`, `ssi-json-ld`), and add both to the workspace members
- [ ] 1.3 Add the `jsonld` and `vc` features to the `oxilite` umbrella and re-export `oxilite::jsonld` / `oxilite::vc`; check that the default build and the wasm build of `oxilite-core` gain no new dependencies

## 2. Schema and core hooks

- [ ] 2.1 Add `oxilite_jsonld::schema::create_schema(&MetadataIndexes)` producing the `jsonld_documents`, `jsonld_graphs` and `jsonld_contexts` DDL, the optional partial indexes and the `jsonld_schema` meta row (design §3)
- [ ] 2.2 Add an extension hook to `oxilite_core::schema::schema_sql` so extra DDL can be appended for `wrangler d1 migrations`
- [ ] 2.3 Expose from `oxilite_core::writer` a way to build an atomic request from a statement prefix plus `EncodedQuads` plus a statement suffix, respecting `Capabilities` limits; return `DocumentTooLarge` instead of chunking

## 3. JSON-LD conversion (`oxilite-jsonld`)

- [ ] 3.1 Implement `JsonLdOptions`, `KeyStrategy`, `MissingKey` and `GraphStrategy`, including JSON Pointer lookup, content-hash keys (`urn:oxilite:doc:sha256:<hex>` of the raw bytes), `{key}` template substitution with percent-encoding, and the invalid-graph-name error
- [ ] 3.2 Implement the loader chain: an in-memory map, `DbContextCache` (records misses, returns a sentinel error) and an optional `ReqwestLoader`; add `put_context(iri, json)` / `remove_context(iri)`
- [ ] 3.3 Implement the document-scoped blank-node generator `_:d<h>_<n>` (design §5)
- [ ] 3.4 Implement the `rdf-types` → `oxrdf` quad conversion, including the `rdf_direction` options and rewriting the default graph to the target graph; collect the owned graphs
- [ ] 3.5 Implement the `put_document` step machine: parse → expand → (fetch missing contexts, at most 8 rounds) → convert to RDF → one atomic request with the delete prefix (design §4, §7)
- [ ] 3.6 Implement `get_document`, `remove_document` (existence reported from the change count), `list_documents` (keyset paging), `document_graphs` and `document_for_graph`
- [ ] 3.7 Implement the fixed-graph and default-graph strategies' exact-quad delete path (read the stored document, then delete its re-derived quads atomically)
- [ ] 3.8 Implement `rebuild_graph(key)` and `check_documents()` (design §8)
- [ ] 3.9 Implement the `JsonLdStore` (sync, driven with `pollster`) and `AsyncJsonLdStore` handles over `Store<B>` / `AsyncStore<B>`, creating the schema on first open

## 4. Verifiable Credentials profile (`oxilite-vc`)

- [ ] 4.1 Implement `CredentialOptions` with the VC defaults (key = `id`, graph = key, missing-id → content hash, embed = true, indexes = issuer, subject, valid_until)
- [ ] 4.2 Put `ssi_json_ld::ContextLoader::empty().with_static_loader()` first in the loader chain
- [ ] 4.3 Implement structural checks by parsing with `ssi_vc::AnyJsonCredential` / `AnyJsonPresentation` and mapping failures to invalid-credential errors that name the rule; detect the `vc1`/`vc2`/`vp1`/`vp2` profile
- [ ] 4.4 Implement metadata extraction (issuer id, first subject id, types, validity start/end across v1.1 and v2 field names, as epoch seconds) into the document row
- [ ] 4.5 Implement `put_credential`, `get_credential`, `remove_credential` and `put_presentation`, storing embedded credentials in the same atomic request and filling `refs`
- [ ] 4.6 Implement `find_credentials(CredentialFilter)`: issuer, subject, type (via `json_each`, with a Rust fallback when JSON1 is missing), `valid_at`, profile, and keyset paging, as a single `Read` request

## 5. Tests

- [ ] 5.1 Add test spec sections for the `jsonld-documents` and `verifiable-credentials` requirements to `lat.md/tests.md`, and reference each one from its test with `// @lat:`
- [ ] 5.2 Add a W3C JSON-LD 1.1 `toRdf` runner (offline tests only) with an allowlist of justified skips; compare isomorphism against `oxjsonld`'s parse of the same input as a second opinion
- [ ] 5.3 Add unit tests for the key strategies, graph strategies, missing-key policies, blank-node isolation and idempotence, verbatim round-trip, and offline context resolution (in-memory and persisted)
- [ ] 5.4 Add atomicity tests: replace drops stale triples; an injected failure in a replace leaves the old version; remove clears the owned graphs only
- [ ] 5.5 Add VC fixtures (W3C VCDM v1.1 and v2 examples, with and without `id`, with Data Integrity proofs, and a VP embedding two VCs) covering the proof graph separation, embedded credential storage, metadata extraction and `find_credentials`
- [ ] 5.6 Run the JSON-LD and VC suites on the rusqlite, dylib (platform SQLite) and D1 sidecar backends
- [ ] 5.7 Add a regression test that fixes the blank-node labels of a reference credential (guards against `json-ld` expansion order changes)

## 6. Documentation

- [ ] 6.1 Add a "JSON-LD documents" section to `lat.md/architecture.md` (crates, tables, pipeline, "the raw document is the source of truth")
- [ ] 6.2 Record the key and graph defaults, the `json-ld`/`ssi` choice and the document-scoped blank nodes in `lat.md/decisions.md`; list the change in `lat.md/milestones.md`; run `lat check`
- [ ] 6.3 Add rustdoc examples for `JsonLdStore` and `CredentialStore`, and a README section

## 7. Bindings (follow-up, gated)

- [ ] 7.1 Expose `putDocument` / `getDocument` / `removeDocument` / `putContext` in `@oxilite/node` with TypeScript types
- [ ] 7.2 Measure the size of the `oxilite-wasm` bundle with `jsonld` and with `vc`; expose them in `@oxilite/d1` only if the bundle stays under the Workers 10 MB limit, and record the result in the design's open question
