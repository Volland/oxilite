<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-vc

[![crates.io](https://img.shields.io/crates/v/oxilite-vc.svg)](https://crates.io/crates/oxilite-vc) [![docs.rs](https://img.shields.io/docsrs/oxilite-vc)](https://docs.rs/oxilite-vc) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**Verifiable Credentials in oxilite.** It stores W3C Verifiable Credentials and Presentations (VCDM 1.1 and 2.0):
- the credential exactly as issued, under its `id`;
- its claims as RDF in the named graph of the same IRI, queryable with SPARQL;
- its issuer, subject, types and validity window in indexed columns, for fast lookups.

It runs natively and on Cloudflare D1.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-vc) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

## Usage

Enable the `vc` feature of [`oxilite`](https://crates.io/crates/oxilite):

```toml
oxilite = { version = "0.2", features = ["vc"] }
```

```rust
use oxilite::store::Store;
use oxilite::vc::CredentialFilter;

let store = Store::new()?;
let vcs = store.credentials()?;

let key = vcs.put_credential(r#"{
  "@context": ["https://www.w3.org/ns/credentials/v2", "https://www.w3.org/ns/credentials/examples/v2"],
  "id": "http://university.example/credentials/3732",
  "type": ["VerifiableCredential", "ExampleDegreeCredential"],
  "issuer": "https://university.example/issuers/565049",
  "validFrom": "2010-01-01T00:00:00Z",
  "credentialSubject": {
    "id": "did:example:ebfeb1f712ebc6f1c276e12ec21",
    "degree": {"type": "ExampleBachelorDegree", "name": "Bachelor of Science and Arts"}
  }
}"#)?;
assert_eq!(key, "http://university.example/credentials/3732");

// Claims, per credential graph, with SPARQL:
store.query(r#"
  SELECT ?cred ?name WHERE {
    GRAPH ?cred { ?s <https://www.w3.org/ns/credentials/examples#degree> ?d .
                  ?d <https://schema.org/name> ?name }
  }"#)?;

// Metadata, from indexed columns:
let now = 1_790_000_000.0; // epoch seconds
let valid = vcs.find_credentials(&CredentialFilter {
    issuer: Some("https://university.example/issuers/565049".into()),
    valid_at: Some(now),
    ..Default::default()
})?;

// The credential, byte for byte, e.g. to verify or present it.
let raw = vcs.get_credential(&key)?.unwrap().json;
# Result::<_, Box<dyn std::error::Error>>::Ok(())
```

`AsyncStore::credentials()` has the same methods for D1. From JavaScript, call `store.credentials()` in [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node) or [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1).

## What happens on `put_credential`

1. **Structure check.** The first `@context` selects VCDM 1.1 or 2.0. The credential is then parsed with [`ssi-vc`](https://crates.io/crates/ssi-vc), which checks context order, the `VerifiableCredential` type, `issuer` and `credentialSubject`. A failure is `JsonLdError::Invalid`, naming the rule, and nothing is written.
2. **Key.** The key is the credential's `id`. A credential without one (optional in VCDM 2.0) gets `urn:oxilite:doc:sha256:<hex>` of its bytes, or is rejected if you configure that.
3. **RDF.** The claims go into the graph `<id>`. JSON-LD puts each proof in its own graph, owned by the credential, so the credential graph never contains `proofValue`s. The W3C credential, security, Data Integrity, cryptosuite and DID contexts are bundled by [`ssi-json-ld`](https://crates.io/crates/ssi-json-ld), so conversion needs no network access, on D1 too.
4. **Metadata.** Issuer, subject, types, `validFrom`/`issuanceDate` and `validUntil`/`expirationDate` are stored in indexed columns.
5. **Write.** Everything goes in one atomic request. Storing a credential with the same key replaces it.

`put_presentation` stores the presentation the same way. It also stores each credential embedded in it (those with an `id`) as a credential of its own, in the same batch, and returns their keys.

**Proofs are not verified.** Storing a credential says nothing about its validity. To verify, run `ssi` on the JSON that `get_credential` returns.

## Options

`CredentialOptions { jsonld, embed_presentation_credentials }`. `jsonld` takes every option of [`oxilite-jsonld`](https://crates.io/crates/oxilite-jsonld), including:
- key strategy (e.g. `/credentialSubject/id`);
- graph strategy (e.g. `https://example.org/holders/{key}`);
- missing-id policy;
- persisted contexts for your own vocabularies;
- which metadata indexes to create (`MetadataIndexes::none()` gives the cheapest D1 writes).

## Size

The `ssi` crates are large: about 430 crates in the build, and roughly 0.9 MB more WebAssembly than `jsonld` alone. That is why they sit behind their own feature. They also enable serde_json's `arbitrary_precision` for the whole build, which oxilite handles.

## The oxilite family

oxilite is an Oxigraph-compatible RDF database and SPARQL 1.1 engine that stores its data in SQLite, so it runs anywhere SQLite runs: in-process, on a system or vendor `libsqlite3`, on Cloudflare D1 and in Durable Objects. The same data can be queried with SPARQL and openCypher, reasoned over with RDFS / OWL, validated with SHACL and ShEx, and loaded from JSON-LD documents and Verifiable Credentials. Read the overview on **[oxilitedb.com](https://oxilitedb.com)** and the full guide in the [main README](https://github.com/Volland/oxilite#readme).

| Package | What it is for |
|---|---|
| [`oxilite`](https://crates.io/crates/oxilite) | The store: a drop-in for `oxigraph::store::Store`, plus `AsyncStore` for D1 |
| [`oxilite-core`](https://crates.io/crates/oxilite-core) | The sans-IO core: term encoding, schema, SPARQL → SQL compiler and planner |
| [`oxilite-rusqlite`](https://crates.io/crates/oxilite-rusqlite) | In-process backend with a bundled SQLite (the default) |
| [`oxilite-dylib`](https://crates.io/crates/oxilite-dylib) | Backend that loads your own `libsqlite3` at runtime |
| [`oxilite-d1`](https://crates.io/crates/oxilite-d1) | Cloudflare D1 backend for Rust Workers |
| [`oxilite-cypher`](https://crates.io/crates/oxilite-cypher) | openCypher over the same data, OWL- and SHACL-aware |
| [`oxilite-jsonld`](https://crates.io/crates/oxilite-jsonld) | JSON-LD documents stored verbatim, one named graph each |
| [`oxilite-vc`](https://crates.io/crates/oxilite-vc) | Verifiable Credentials: stored under their id, indexed, queryable |
| [`oxilite-reason`](https://crates.io/crates/oxilite-reason) | OWL 2 RL materialization with `reasonable` |
| [`oxilite-validate`](https://crates.io/crates/oxilite-validate) | SHACL and ShEx validation with rudof |
| [`oxilite-cli`](https://crates.io/crates/oxilite-cli) | The `oxilite` command and a SPARQL endpoint like `oxigraph serve` |
| [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node) | Node.js bindings, API of Oxigraph's JS package |
| [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1) | Cloudflare D1 and Durable Objects from TypeScript (WebAssembly core) |
| [`@oxilite/common`](https://www.npmjs.com/package/@oxilite/common) | RDF/JS terms and shared TypeScript types |

## License

Dual-licensed under [MIT](https://github.com/Volland/oxilite/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/Volland/oxilite/blob/main/LICENSE-APACHE), at your option, like Oxigraph.
