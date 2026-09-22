## Context

See proposal.md (Why) and the two delta specs for the requirements. The design has to fit these existing constraints (`lat.md/architecture.md`):

- **Sans-IO core.** Every operation yields SQL `Request`s (`Read` or `Atomic`), and an `Atomic` request is exactly one D1 batch. Operations that need several round-trips are step machines.
- **Hash-encoded terms.** Term ids are computed in Rust (xxh3 → tagged 59-bit payload), so writes never read back. The writer (`EncodedQuads`) emits `INSERT OR IGNORE` for terms, triple terms, graphs and quads.
- **D1 bills every row and index entry written.** Index count is kept deliberately low, and schema additions must be opt-in.
- **`json-ld` is async.** `json-ld` 0.21 (timothee-haudebourg, also used by spruceid `ssi`) runs expansion and context loading through an async `Loader` trait. `ExpandedDocument::rdf_quads(generator, rdf_direction)` yields `rdf-types` quads, not `oxrdf` quads.
- **Available VC building blocks.** `ssi-vc` 0.9 offers `v1`/`v2` typed JSON credentials and presentations (`AnyJsonCredential`, `AnyJsonPresentation`). `ssi-json-ld` 0.3 offers `ContextLoader::empty().with_static_loader()`, which bundles the credentials v1/v2, security, Data Integrity, cryptosuite and DID contexts. Both depend on `json-ld` 0.21, so the two new crates share one JSON-LD stack.

## Goals / Non-Goals

**Goals:**
- Two crates with a one-way dependency: `oxilite-vc` → `oxilite-jsonld` → `oxilite-core`. The generic JSON-LD layer has no VC types.
- The same operations on every backend (rusqlite, dylib, D1), and no network access unless opted in.
- One atomic request per put, replace or remove.
- No extra cost for stores that don't use JSON-LD.

**Non-Goals:**
- Proof or signature verification, DID resolution, status lists. Callers can use `ssi` directly on the raw document returned by `get_credential`.
- JSON-LD compaction or framing on read. The raw document is the read format; `dump_graph` with `RdfFormat::JsonLd` (oxrdfio/oxjsonld, already in the tree) stays available for graph export.
- A SPARQL extension function that returns raw JSON. Mapping from graph to document happens in the API (`document_for_graph`).
- Keeping raw documents in sync automatically when SPARQL UPDATE edits a document graph. This is detected and repaired, not prevented (see Decisions §8).

## Decisions

### 1. Crate split and dependency choice

```
crates/oxilite-jsonld   json-ld 0.21, json-syntax, iref, rdf-types, sha2, oxilite-core
crates/oxilite-vc       oxilite-jsonld, ssi-vc 0.9, ssi-json-ld 0.3
crates/oxilite          features: jsonld = [oxilite-jsonld], vc = [jsonld, oxilite-vc]
```

- **`json-ld` over `oxjsonld`.** `oxjsonld` (already a transitive dependency via oxrdfio) is a streaming parser/serializer. It has no full expansion API, no pluggable async loader, and no hooks for `@graph` containers. `json-ld` implements the full JSON-LD 1.1 API with a proper loader abstraction. It is also the stack `ssi` builds on, so VC contexts and processing behave the same way as in the wider Rust VC ecosystem.
- **`ssi-vc` over hand-written VC checks.** It already models VCDM v1.1 and v2, including the differences between `issuanceDate` and `validFrom`. We use only its syntax and typed parsing, not its proof or verification machinery. Only `ssi-vc` and `ssi-json-ld` are depended on, never the `ssi` umbrella, which keeps the tree smaller.
- **Separate crates, not features of one crate.** A user who stores generic JSON-LD (schema.org, ActivityStreams) does not pay for `ssi-*` compile time or wasm size. VC code cannot leak into the generic key and graph logic.

### 2. Handles, not new methods on `Store`

`JsonLdStore<'a, B>` wraps `&Store<B>` (and `AsyncJsonLdStore` wraps `&AsyncStore<B>`) and holds `JsonLdOptions`. `CredentialStore` wraps a `JsonLdStore` configured with the VC defaults. `Store` gains only the constructors `jsonld()` / `credentials()`, so its query and update API stays identical to `oxigraph::store::Store`, which protects the compat harness.

*As built:* the handles live in the umbrella crate (`crates/oxilite/src/jsonld_store.rs`, `vc_store.rs`), like `cypher_store.rs`, because `oxilite-jsonld` cannot depend on `oxilite` (the umbrella re-exports it). `oxilite-jsonld` and `oxilite-vc` stay sans-IO and expose jobs (`WriteJob`, `RebuildJob`, `CheckJob`, read jobs) that both stores and the wasm engine drive. `JsonLdOptions` also has `processing_mode` (JSON-LD 1.0 / 1.1), used by the W3C runner.

```rust
pub struct JsonLdOptions {
    pub key: KeyStrategy,            // Id (default) | Pointer(String) | ContentHash | Explicit
    pub on_missing_key: MissingKey,  // Reject (generic default) | ContentHash
    pub graph: GraphStrategy,        // Key (default) | Template(String) | Fixed(NamedNode) | DefaultGraph
    pub base_iri: Option<Iri>,       // for relative ids; default: none
    pub network: bool,               // default false; native + `reqwest` feature only
    pub rdf_direction: Option<RdfDirection>,
}
```

`CredentialOptions` = `JsonLdOptions` with `on_missing_key: ContentHash`, plus `embed_presentation_credentials: bool` (default true) and `metadata_indexes: MetadataIndexes` (default: issuer, subject, valid_until).

Keys are strings. When the graph strategy needs an IRI, the key is parsed with `oxiri`. The content-hash key is `urn:oxilite:doc:sha256:<hex>` over the *raw bytes*, not canonical JSON, because the verbatim bytes are what we store and return. Byte-different but semantically equal documents therefore get different keys. We accept this: the fallback only applies to id-less credentials.

### 3. Schema (opt-in, created on the first handle open)

```sql
CREATE TABLE IF NOT EXISTS jsonld_documents (
  key         TEXT PRIMARY KEY,
  graph       INTEGER NOT NULL,        -- term id of target graph (0 = default graph)
  doc         TEXT NOT NULL,           -- raw JSON, verbatim
  sha256      TEXT NOT NULL,           -- hex (the SQL value model has no BLOBs)
  profile     TEXT NOT NULL,           -- 'jsonld' | 'vc1' | 'vc2' | 'vp1' | 'vp2'
  issuer      TEXT, subject TEXT,      -- VC metadata (NULL for generic docs)
  types       TEXT,                    -- JSON array
  valid_from  REAL, valid_until REAL,  -- epoch seconds, same convention as terms.ts
  refs        TEXT,                    -- JSON array of keys (VP → embedded VCs)
  stored_at   REAL NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS jsonld_graphs (key TEXT NOT NULL, g INTEGER NOT NULL,
  PRIMARY KEY (key, g), UNIQUE (g)) WITHOUT ROWID, STRICT;  -- owned graphs; UNIQUE(g) = one owner
CREATE TABLE IF NOT EXISTS jsonld_contexts (iri TEXT PRIMARY KEY, doc TEXT NOT NULL) STRICT;
-- optional, per MetadataIndexes (partial, so generic docs cost nothing):
CREATE INDEX IF NOT EXISTS jsonld_issuer      ON jsonld_documents(issuer, valid_until) WHERE issuer IS NOT NULL;
CREATE INDEX IF NOT EXISTS jsonld_subject     ON jsonld_documents(subject)             WHERE subject IS NOT NULL;
CREATE INDEX IF NOT EXISTS jsonld_valid_until ON jsonld_documents(valid_until)         WHERE valid_until IS NOT NULL;
```

- The metadata lives in one wide row instead of a separate `vc_metadata` table. A credential put writes one row instead of two, and generic documents leave the columns NULL, which partial indexes skip. This is cheaper on D1, which bills per row written.
- `subject` holds the first `credentialSubject.id`. Credentials with several subjects are rare, so extra subject ids are matched through SPARQL rather than a side table (see Open Questions).
- `jsonld_graphs` owns *every* graph a document writes, including the target graph. Replace and remove therefore work from this one list. `UNIQUE(g)` makes it an error for two documents to write the same graph. For example, with `GraphStrategy::Fixed`, all documents would share a graph. For that case ownership is disabled and deletion falls back to removing exactly the document's quads, which are recomputed from its raw JSON (see §7).
- The DDL lives in `oxilite-jsonld::schema`. `oxilite-core::schema` only gets a `schema_sql` hook to concatenate extra DDL for `wrangler d1 migrations`. The `oxilite_meta` table records `jsonld_schema = 1`.

### 4. Conversion pipeline and the async loader

`put_document` is a step machine, the same pattern as queries:

1. **Parse.** `json_syntax::Value::parse_str` runs on the raw bytes, while the `&str` is kept for storage. The key comes from the key strategy, and the target graph from the graph strategy.
2. **Expand.** `RemoteDocument::expand(&loader)` runs with a `ChainLoader`: registered map → `DbContextCache` → (native, opt-in) `ReqwestLoader`. `oxilite-vc` puts `ssi_json_ld::ContextLoader::empty().with_static_loader()` first. The loader never does I/O itself. On a cache miss, `DbContextCache` records the IRI and returns a sentinel error.
3. **Fetch missing contexts.** If expansion failed only because of missing contexts, the machine yields `Execute(Read: SELECT iri, doc FROM jsonld_contexts WHERE iri IN (…))`, fills the cache and retries expansion. Each round discovers the contexts that the previous round's contexts import. The loop is bounded at 8 rounds, and a context that is still missing fails with `loading remote context failed: <iri>`.
4. **Convert to RDF.** `rdf_quads(generator, rdf_direction)` runs, and `rdf-types` quads are mapped to `oxrdf::Quad` by a small conversion module. The document's default graph is rewritten to the target graph. Named graphs stay as they are, but are added to the owned set.
5. **Encode and emit** one `Atomic` request: the delete prefix (§7), the writer's term, graph and quad inserts, `INSERT INTO jsonld_graphs`, and `INSERT OR REPLACE INTO jsonld_documents`.

Because the loaders never really await, the futures complete on the first poll. The sync `Store` drives them with a minimal `block_on` (`pollster`), and `AsyncStore` simply `.await`s. The `reqwest` loader is only compiled with `features = ["network"]` on non-wasm targets.

*As built:* the job polls the futures itself with a no-op waker (`poll_ready`), for sync and async stores alike, so no executor dependency is needed. Network loading is a synchronous `ContextFetcher` callback (`http_fetcher()` on `ureq`, feature `network`, native only) that the job calls after the database round, instead of an async `ReqwestLoader`, so the processor's futures still never wait. When `cache_fetched` is set, fetched contexts are persisted in the same batch.

**Alternative considered:** pre-scanning `@context` for IRIs and loading them before expansion. This misses contexts imported from other contexts and scoped contexts, so the retry loop is more accurate for the same cost.

### 5. Deterministic, document-scoped blank nodes

The `rdf-types` generator is replaced with one that emits `_:d<h>_<n>`, where `h` is the first 16 hex characters of xxh3-128(key) and `n` is a counter in generation order. The expansion order is deterministic for the same input bytes, so a re-put produces the same labels and hence the same term ids. Oxilite's blank-node ids are hashes of the label, so documents never share a blank node, and the same document always produces the same one.

**Alternative:** skolemizing to `<key>/.well-known/genid/n` IRIs. This was rejected because it changes RDF semantics (`isBlank()`), and the key is not always an IRI.

### 6. Graph mapping for VCs

The v2 context declares `proof` as `@container: @graph`, and so does the `verifiableCredential` property of presentations. JSON-LD deserialization already puts those into blank-node named graphs linked from the credential node. The credential graph therefore holds the claims and `sec:proof _:dH_n`, and each proof lives in its own owned graph. This matches the VC data model (a proof is not part of the claims), and it is why `GRAPH <id> { ?s ?p ?o }` returns no `proofValue`.

For presentations, `put_presentation` stores the VP document under its own key. When embedding is on, each embedded credential that has an `id` is also stored as its own document. This happens in the *same* atomic request, so it has its own key, graph and metadata, and the VP's `refs` lists those keys. The VP's own RDF still contains the embedded graphs, which are owned by the VP. This duplication is deliberate: the VP stays a faithful conversion of its bytes, and the credential is findable on its own. Removing the VP does not remove the separately stored credentials.

### 7. Replace and remove in one batch

Replace and remove need no read before the write. The delete prefix is plain self-reading SQL:

```sql
DELETE FROM quads  WHERE g IN (SELECT g FROM jsonld_graphs WHERE key = '<k>');
DELETE FROM graphs WHERE id IN (SELECT g FROM jsonld_graphs WHERE key = '<k>');
DELETE FROM jsonld_graphs WHERE key = '<k>';
```

This is followed by the inserts for the new version, or by `DELETE FROM jsonld_documents WHERE key = '<k>'` for a remove. `remove_document` reports existence from the change count of that final `DELETE`, the same way `Store::remove` reports it.

With the default-graph and fixed-graph strategies, a document has no graph of its own. Replace and remove then delete the exact quad list that is re-derived from the *stored* raw JSON. The step machine first reads `doc` (one `Read`) and then emits the atomic delete. This costs an extra round-trip, but remains correct when documents share a graph, as long as documents don't assert identical triples. That overlap is documented in the rustdoc of these strategies.

The terms are left in place, which is consistent with the store's "no term GC" rule.

### 8. SPARQL UPDATE vs raw documents

We do not add triggers on `quads`: that would add a write cost to every quad on D1. Instead, `check_documents()` recomputes each document's quads and compares them with the owned graphs. It uses `Read` requests that are paged by key. `rebuild_graph(key)` is simply a re-put of the stored bytes. The rustdoc and `lat.md` state the rule: **the raw document is the source of truth, and graphs are derived data.**

### 9. `find_credentials`

This compiles to a single `SELECT key, doc FROM jsonld_documents WHERE …` query, with inlined, escaped literals (the store's no-bound-parameters rule), keyset paging (`key > '<after>' ORDER BY key LIMIT n`), and type matching through `instr(types, '"<type>"') > 0` on the JSON array text: an exact element match that needs no JSON1, so every backend runs the same SQL. `valid_at(t)` becomes `(valid_from IS NULL OR valid_from <= t) AND (valid_until IS NULL OR valid_until > t)`.

### 10. Implementation notes

- `json-ld` 0.21 emits `rdfDirection: i18n-datatype` IRIs in an older form (`i18n#rtl`, language case kept); the conversion normalizes them to JSON-LD 1.1's `i18n#<lang>_<dir>`.
- `json-ld` reports `"@context": 42` as `invalid context entry`, not the `invalid local context` of the W3C suite; the spec scenario only requires that the JSON-LD error code is surfaced.
- The ownership conflict (`UNIQUE(g)` on `jsonld_graphs`) surfaces as a backend error; `JsonLdError::from_store` and the D1 driver map it to `GraphOwned` / `graph-owned`.
- Errors cross the `oxilite_core::Job` boundary as `Error::Other("oxilite-jsonld:…")` and are decoded back by `from_core`; the JavaScript bindings receive `{"code", "message"}` JSON and raise `JsonLdError`.

## Risks / Trade-offs

- **[`ssi-*` size and churn]** → Only `oxilite-vc` depends on `ssi-*`, and the umbrella feature is off by default. Versions are pinned in the workspace. Measured: the `@oxilite/d1` wasm grows from 3.3 MB to 5.0 MB (1.43 MB gzipped), within the Workers limits (3 MB compressed free, 10 MB paid), so the wasm engine builds `vc` by default.
- **[`arbitrary_precision` leaks into the build]** → `ssi-vc` enables serde_json's `arbitrary_precision`, which broke the untagged `SqlValue` deserialization of REAL values from D1 JSON. `SqlValue` now has a hand-written deserializer accepting both number forms; the workspace test suite and the D1 sidecar runs pass with the feature unified.
- **[`getrandom` 0.2 on wasm32]** → the `ssi` crates need getrandom 0.2's `js` backend on `wasm32-unknown-unknown`; `oxilite-vc` enables it for that target.
- **[`json-ld` 0.21 vs `ssi-json-ld` version skew]** → The workspace pins the `json-ld` version that `ssi-json-ld` resolves to, so a single copy is compiled. CI fails on duplicates (`cargo tree -d`).
- **[Large documents exceed a D1 batch]** → Chunking would break atomicity, so documents whose encoded request exceeds the backend's limits fail with `DocumentTooLarge` instead. The limit is documented; VCs are typically well below 1 000 quads.
- **[Expansion order changes across `json-ld` releases would change blank labels]** → Existing data stays valid, but a re-put would then replace blank nodes rather than no-op. This is covered by a regression test that fixes the labels for a reference credential.
- **[Documents that share a graph (fixed or default strategy)]** → Removal is exact only when documents do not assert identical triples. This is documented. The key-as-graph default avoids the issue.
- **[Raw document and graph drift after SPARQL UPDATE]** → `check_documents` and `rebuild_graph` handle this; see §8.

## Migration Plan

This change is additive. Opening a `JsonLdStore` runs `CREATE TABLE IF NOT EXISTS` for the new tables and inserts `jsonld_schema = 1` into `oxilite_meta`, while `schema_version` stays unchanged. D1 users can instead take the DDL from `schema_sql` with the JSON-LD extension for `wrangler d1 migrations`. To roll back, drop the three tables; quad data written by documents stays as ordinary RDF.

## Open Questions

- Should multi-subject credentials get a `jsonld_subjects(key, subject)` side table? This can be added later as another optional index without changing the specs; for now, lookups use the first subject plus SPARQL.
- *Resolved:* both TypeScript packages expose `jsonld()` and `credentials()`; the measured wasm size fits the Workers limits.
