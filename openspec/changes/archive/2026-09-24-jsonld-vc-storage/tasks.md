## 1. Workspace and crates

- [x] 1.1 Add `json-ld` 0.21, `json-syntax`, `iref`, `rdf-types`, `ssi-vc` 0.9 and `ssi-json-ld` 0.3 to `[workspace.dependencies]`, pinning `json-ld` to the version `ssi-json-ld` resolves to; check that `cargo tree -d` shows a single `json-ld` (`pollster` was not needed: jobs poll the processor's futures with a no-op waker)
- [x] 1.2 Create `crates/oxilite-jsonld` (depends on `oxilite-core`; `network` feature with a `ureq` context fetcher on non-wasm only; `json` feature for the bindings) and `crates/oxilite-vc` (depends on `oxilite-jsonld`, `ssi-vc`, `ssi-json-ld`; getrandom 0.2 `js` on wasm32), and add both to the workspace members
- [x] 1.3 Add the `jsonld`, `vc`, `jsonld-network` and `jsonld-json` features to the `oxilite` umbrella and re-export `oxilite::jsonld` / `oxilite::vc`; check that the default build and the wasm build of `oxilite-core` gain no new dependencies

## 2. Schema and core hooks

- [x] 2.1 Add `oxilite_jsonld::schema::create_schema(&MetadataIndexes)` producing the `jsonld_documents`, `jsonld_graphs` and `jsonld_contexts` DDL, the optional partial indexes and the `jsonld_schema` meta row (design §3; `sha256` as hex TEXT)
- [x] 2.2 Add an extension hook to `oxilite_core::schema::schema_sql` (`schema_sql_with`) so extra DDL can be appended for `wrangler d1 migrations`
- [x] 2.3 Expose from `oxilite_core::writer` a way to build an atomic request from a statement prefix plus `EncodedQuads` plus a statement suffix (`atomic_request`), respecting `Capabilities` limits; return `DocumentTooLarge` instead of chunking

## 3. JSON-LD conversion (`oxilite-jsonld`)

- [x] 3.1 Implement `JsonLdOptions`, `KeyStrategy`, `MissingKey` and `GraphStrategy`, including JSON Pointer lookup, content-hash keys (`urn:oxilite:doc:sha256:<hex>` of the raw bytes), `{key}` template substitution with percent-encoding, and the invalid-graph-name error
- [x] 3.2 Implement the loader chain: an in-memory map, a first loader (bundled contexts), misses resolved from `jsonld_contexts` by the job, and an optional synchronous `ContextFetcher` (`http_fetcher()`); add `put_context(iri, json)` / `remove_context(iri)`
- [x] 3.3 Implement the document-scoped blank-node generator `_:d<h>_<n>` (design §5)
- [x] 3.4 Implement the `rdf-types` → `oxrdf` quad conversion, including the `rdf_direction` options (normalized i18n datatypes) and rewriting the default graph to the target graph; collect the owned graphs
- [x] 3.5 Implement the `put_document` step machine: parse → expand → (fetch missing contexts, at most 8 rounds) → convert to RDF → one atomic request with the delete prefix (design §4, §7)
- [x] 3.6 Implement `get_document`, `remove_document` (existence reported from the change count), `list_documents` (keyset paging), `document_graphs` and `document_for_graph`
- [x] 3.7 Implement the fixed-graph and default-graph strategies' exact-quad delete path (read the stored document, then delete its re-derived quads atomically)
- [x] 3.8 Implement `rebuild_graph(key)` and `check_documents()` (design §8)
- [x] 3.9 Implement the `JsonLdStore` (sync) and `AsyncJsonLdStore` handles over `Store<B>` / `AsyncStore<B>` in the umbrella crate, creating the schema on first open

## 4. Verifiable Credentials profile (`oxilite-vc`)

- [x] 4.1 Implement `CredentialOptions` with the VC defaults (key = `id`, graph = key, missing-id → content hash, embed = true, indexes = issuer, subject, valid_until)
- [x] 4.2 Put `ssi_json_ld::ContextLoader::empty().with_static_loader()` first in the loader chain
- [x] 4.3 Implement structural checks by parsing with `ssi_vc::v1` / `v2` `JsonCredential` and `JsonPresentation` (version chosen from the first `@context`) and mapping failures to invalid-credential errors that name the rule; detect the `vc1`/`vc2`/`vp1`/`vp2` profile
- [x] 4.4 Implement metadata extraction (issuer id, first subject id, types, validity start/end across v1.1 and v2 field names, as epoch seconds) into the document row
- [x] 4.5 Implement `put_credential`, `get_credential`, `remove_credential` and `put_presentation`, storing embedded credentials in the same atomic request and filling `refs`
- [x] 4.6 Implement `find_credentials(CredentialFilter)`: issuer, subject, type (exact element match with `instr`, no JSON1 needed), `valid_at`, profile, and keyset paging, as a single `Read` request

## 5. Tests

- [x] 5.1 Add test spec sections for the `jsonld-documents` and `verifiable-credentials` requirements to `lat.md/tests.md`, and reference each one from its test with `// @lat:`
- [x] 5.2 Add a W3C JSON-LD 1.1 `toRdf` runner (offline: the suite's remote contexts are served from the `testsuite/json-ld-api` checkout) with an allowlist of justified skips (the planned `oxjsonld` second-opinion comparison was dropped: no unexplained failures remained to diagnose)
- [x] 5.3 Add unit tests for the key strategies, graph strategies, missing-key policies, blank-node isolation and idempotence, verbatim round-trip, and offline context resolution (in-memory and persisted)
- [x] 5.4 Add atomicity tests: replace drops stale triples; a failing replace (graph owned by another document) leaves the old version; remove clears the owned graphs only
- [x] 5.5 Add VC fixtures (W3C VCDM v1.1 and v2 examples, with and without `id`, with a Data Integrity proof, and a VP embedding two VCs) covering the proof graph separation, embedded credential storage, metadata extraction and `find_credentials`
- [x] 5.6 Run the JSON-LD and VC suites on the rusqlite, dylib (platform SQLite) and D1 sidecar backends
- [x] 5.7 Add a regression test that fixes the blank-node labels of a reference credential (guards against `json-ld` expansion order changes)

## 6. Documentation

- [x] 6.1 Add a "JSON-LD documents" section to `lat.md/architecture.md` (crates, tables, pipeline, "the raw document is the source of truth")
- [x] 6.2 Record the key and graph defaults, the `json-ld`/`ssi` choice and the document-scoped blank nodes in `lat.md/decisions.md`; list the change in `lat.md/milestones.md`; run `lat check`
- [x] 6.3 Add rustdoc examples for `JsonLdStore` and `CredentialStore`, READMEs for both crates, and sections in the main, `oxilite`, `@oxilite/node`, `@oxilite/d1` and `@oxilite/common` READMEs; add the site article and landing-page section, backed by `examples/verifiable-credentials`

## 7. Bindings

- [x] 7.1 Expose `jsonld()` and `credentials()` handles in `@oxilite/node` with TypeScript types (in `@oxilite/common`)
- [x] 7.2 Measure the size of the `oxilite-wasm` bundle with `jsonld` and with `vc` (3.3 MB → 5.0 MB, 1.43 MB gzipped: within the Workers limits); expose both in `@oxilite/d1`, with `schemaSql({ jsonld })` and `oxilite-d1 schema --jsonld`
