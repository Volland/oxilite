//! Verifiable Credentials: the `verifiable-credentials` specification scenarios on every
//! test engine (native, dylib, the D1 code path, and Miniflare with `OXILITE_D1_URL`).

#[path = "../../oxilite-jsonld/tests/engine/mod.rs"]
mod engine;

use engine::Engine;
use oxilite::jsonld::{JsonLdError, MetadataIndexes};
use oxilite::model::{GraphName, Term};
use oxilite::sparql::QueryResults;
use oxilite::vc::{CredentialFilter, CredentialOptions, GraphStrategy, KeyStrategy};

/// Calls a credential-handle method on any engine (awaiting it on async ones).
macro_rules! vcs {
    ($e:expr, $o:expr, $m:ident ( $($a:expr),* )) => {
        match $e {
            Engine::Native(s) => s.credentials_with($o.clone()).and_then(|d| d.$m($($a),*)),
            Engine::Dylib(s) => s.credentials_with($o.clone()).and_then(|d| d.$m($($a),*)),
            Engine::D1(s) => futures::executor::block_on(async {
                match s.credentials_with($o.clone()).await {
                    Ok(d) => d.$m($($a),*).await,
                    Err(e) => Err(e),
                }
            }),
            Engine::Miniflare(s, _) => futures::executor::block_on(async {
                match s.credentials_with($o.clone()).await {
                    Ok(d) => d.$m($($a),*).await,
                    Err(e) => Err(e),
                }
            }),
        }
    };
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn opts() -> CredentialOptions {
    CredentialOptions::default()
}

const DEGREE_ID: &str = "http://university.example/credentials/3732";
const VC: &str = "https://www.w3.org/2018/credentials#";

// @lat: [[tests#Verifiable Credentials#Credential id is key and graph]]
#[test]
fn credential_id_is_key_and_graph() {
    let degree = fixture("degree-v2.json");
    for e in Engine::all() {
        let key = vcs!(&e, opts(), put_credential(&degree)).unwrap();
        assert_eq!(key, DEGREE_ID, "{}", e.name());
        let stored = vcs!(&e, opts(), get_credential(DEGREE_ID))
            .unwrap()
            .unwrap();
        assert_eq!(stored.json, degree);
        assert_eq!(
            stored.graph,
            GraphName::NamedNode(oxilite::model::NamedNode::new_unchecked(DEGREE_ID))
        );
        assert!(e.ask(&format!("ASK {{ GRAPH <{DEGREE_ID}> {{ <{DEGREE_ID}> a <{VC}VerifiableCredential> ; <{VC}issuer> <https://university.example/issuers/565049> }} }}")), "{}", e.name());
    }
}

// @lat: [[tests#Verifiable Credentials#Key and graph are configurable]]
#[test]
fn key_and_graph_are_configurable() {
    let degree = fixture("degree-v2.json");
    let mut o = opts();
    o.jsonld.key = KeyStrategy::Pointer("/credentialSubject/id".into());
    o.jsonld.graph = GraphStrategy::Template("https://example.org/holders/{key}".into());
    for e in Engine::all() {
        let key = vcs!(&e, o, put_credential(&degree)).unwrap();
        assert_eq!(key, "did:example:ebfeb1f712ebc6f1c276e12ec21");
        assert!(e.ask("ASK { GRAPH <https://example.org/holders/did:example:ebfeb1f712ebc6f1c276e12ec21> { ?vc a ?t } }"), "{}", e.name());
    }
}

// @lat: [[tests#Verifiable Credentials#Credentials without id]]
#[test]
fn credentials_without_id() {
    let member = fixture("no-id-v2.json");
    for e in Engine::all() {
        let key = vcs!(&e, opts(), put_credential(&member)).unwrap();
        assert!(key.starts_with("urn:oxilite:doc:sha256:"), "{key}");
        assert_eq!(vcs!(&e, opts(), put_credential(&member)).unwrap(), key);
        let mut reject = opts();
        reject.jsonld.on_missing_key = oxilite::jsonld::MissingKey::Reject;
        let err = vcs!(&e, reject, put_credential(&member)).unwrap_err();
        assert!(matches!(err, JsonLdError::MissingKey(_)), "{err}");
        assert_eq!(
            vcs!(
                &e,
                reject,
                put_credential_with_key("urn:member:bob", &member)
            )
            .unwrap(),
            "urn:member:bob"
        );
    }
}

// @lat: [[tests#Verifiable Credentials#Structure is checked]]
#[test]
fn structure_is_checked() {
    let degree = fixture("degree-v2.json");
    let wrong_context = degree.replacen(
        "https://www.w3.org/ns/credentials/v2",
        "https://example.org/ctx",
        1,
    );
    let no_issuer = degree.replace(
        "\"issuer\": \"https://university.example/issuers/565049\",",
        "",
    );
    let not_vc = degree.replace("[\"VerifiableCredential\", ", "[");
    for e in Engine::all() {
        for (bad, needle) in [
            (&wrong_context, "first @context"),
            (&no_issuer, "issuer"),
            (&not_vc, "VerifiableCredential"),
        ] {
            let err = vcs!(&e, opts(), put_credential(bad)).unwrap_err();
            assert!(
                matches!(&err, JsonLdError::Invalid(m) if m.contains(needle)),
                "{}: {err}",
                e.name()
            );
        }
        assert_eq!(e.quads(), 0);
        vcs!(&e, opts(), put_credential(&degree)).unwrap();
        vcs!(&e, opts(), put_credential(&fixture("alumni-v1.json"))).unwrap();
        let profile = |k: &str| {
            vcs!(&e, opts(), get_credential(k))
                .unwrap()
                .unwrap()
                .meta
                .profile
        };
        assert_eq!(profile(DEGREE_ID), "vc2");
        assert_eq!(profile("http://example.edu/credentials/1872"), "vc1");
    }
}

// @lat: [[tests#Verifiable Credentials#Proofs live in owned graphs]]
#[test]
fn proofs_live_in_owned_graphs() {
    let degree = fixture("degree-v2.json");
    for e in Engine::all() {
        vcs!(&e, opts(), put_credential(&degree)).unwrap();
        let sec = "https://w3id.org/security#";
        assert!(!e.ask(&format!(
            "ASK {{ GRAPH <{DEGREE_ID}> {{ ?s <{sec}proofValue> ?v }} }}"
        )));
        assert!(e.ask(&format!("ASK {{ GRAPH <{DEGREE_ID}> {{ <{DEGREE_ID}> <{sec}proof> ?p }} GRAPH ?p {{ ?proof <{sec}proofValue> ?v ; <{sec}cryptosuite> ?suite }} }}")), "{}", e.name());
        assert!(vcs!(&e, opts(), remove_credential(DEGREE_ID)).unwrap());
        assert_eq!(e.quads(), 0, "{}", e.name());
    }
}

// @lat: [[tests#Verifiable Credentials#Presentations store embedded credentials]]
#[test]
fn presentations_store_embedded_credentials() {
    let vp = fixture("presentation-v2.json");
    for e in Engine::all() {
        let keys = vcs!(&e, opts(), put_presentation(&vp)).unwrap();
        assert_eq!(keys.key, "urn:uuid:3978344f-8596-4c3a-a978-8fcaba3903c5");
        assert_eq!(keys.credentials, ["urn:uuid:vc-a", "urn:uuid:vc-b"]);
        let a = vcs!(&e, opts(), get_credential("urn:uuid:vc-a"))
            .unwrap()
            .unwrap();
        assert_eq!(a.meta.profile, "vc2");
        let stored_vp = vcs!(&e, opts(), get_credential(&keys.key))
            .unwrap()
            .unwrap();
        assert_eq!(stored_vp.json, vp);
        assert_eq!(stored_vp.meta.profile, "vp2");
        assert_eq!(stored_vp.meta.refs, keys.credentials);
        let f = CredentialFilter {
            issuer: Some("did:example:academy".into()),
            ..Default::default()
        };
        assert_eq!(
            vcs!(&e, opts(), find_credentials(&f)).unwrap().len(),
            2,
            "{}",
            e.name()
        );
        assert!(e.ask("ASK { GRAPH <urn:uuid:vc-b> { ?s ?p \"Go master\" } }"));
        // The separately stored credentials outlive the presentation.
        assert!(vcs!(&e, opts(), remove_credential(&keys.key)).unwrap());
        assert!(e.ask("ASK { GRAPH <urn:uuid:vc-b> { ?s ?p \"Go master\" } }"));
        // Without embedding only the presentation is stored.
        let mut o = opts();
        o.embed_presentation_credentials = false;
        vcs!(&e, o, remove_credential("urn:uuid:vc-a")).unwrap();
        let keys = vcs!(&e, o, put_presentation(&vp)).unwrap();
        assert!(keys.credentials.is_empty());
        assert!(vcs!(&e, o, get_credential("urn:uuid:vc-a"))
            .unwrap()
            .is_none());
    }
}

// @lat: [[tests#Verifiable Credentials#Metadata across data model versions]]
#[test]
fn metadata_across_data_model_versions() {
    for e in Engine::all() {
        vcs!(&e, opts(), put_credential(&fixture("alumni-v1.json"))).unwrap();
        vcs!(&e, opts(), put_credential(&fixture("degree-v2.json"))).unwrap();
        let v1 = vcs!(
            &e,
            opts(),
            get_credential("http://example.edu/credentials/1872")
        )
        .unwrap()
        .unwrap()
        .meta;
        assert_eq!(
            v1.issuer.as_deref(),
            Some("https://example.edu/issuers/565049")
        );
        assert_eq!(v1.valid_from, Some(1_262_373_804.0));
        assert_eq!(v1.valid_until, Some(1_577_906_604.0));
        let v2 = vcs!(&e, opts(), get_credential(DEGREE_ID))
            .unwrap()
            .unwrap()
            .meta;
        assert_eq!(
            v2.subject.as_deref(),
            Some("did:example:ebfeb1f712ebc6f1c276e12ec21")
        );
        assert_eq!(
            v2.types,
            ["VerifiableCredential", "ExampleDegreeCredential"]
        );
        assert_eq!(v2.valid_from, Some(1_262_304_000.0));
        assert_eq!(v2.valid_until, Some(2_208_988_800.0));
    }
}

// @lat: [[tests#Verifiable Credentials#Metadata indexes are configurable]]
#[test]
fn metadata_indexes_are_configurable() {
    for e in Engine::all() {
        let mut o = opts();
        o.jsonld.indexes = MetadataIndexes::none();
        vcs!(&e, o, put_credential(&fixture("degree-v2.json"))).unwrap();
        assert_eq!(
            e.sql_count(
                "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name LIKE 'jsonld_%'"
            ),
            0,
            "{}",
            e.name()
        );
        assert_eq!(e.sql_count("SELECT count(*) FROM jsonld_documents WHERE issuer IS NOT NULL AND valid_until IS NOT NULL"), 1);
    }
}

// @lat: [[tests#Verifiable Credentials#Find valid credentials]]
#[test]
fn find_valid_credentials() {
    let make = |id: &str, until: &str| {
        format!(
            r#"{{"@context": ["https://www.w3.org/ns/credentials/v2"], "id": "{id}", "type": ["VerifiableCredential"], "issuer": "did:example:issuer", "validFrom": "2020-01-01T00:00:00Z", "validUntil": "{until}", "credentialSubject": {{"id": "did:example:s"}}}}"#
        )
    };
    for e in Engine::all() {
        vcs!(
            &e,
            opts(),
            put_credential(&make("urn:c1", "2099-01-01T00:00:00Z"))
        )
        .unwrap();
        vcs!(
            &e,
            opts(),
            put_credential(&make("urn:c2", "2021-01-01T00:00:00Z"))
        )
        .unwrap();
        vcs!(
            &e,
            opts(),
            put_credential(&make("urn:c3", "2098-01-01T00:00:00Z"))
        )
        .unwrap();
        let now = 1_790_000_000.0;
        let f = CredentialFilter {
            issuer: Some("did:example:issuer".into()),
            valid_at: Some(now),
            ..Default::default()
        };
        let keys: Vec<String> = vcs!(&e, opts(), find_credentials(&f))
            .unwrap()
            .into_iter()
            .map(|d| d.key)
            .collect();
        assert_eq!(keys, ["urn:c1", "urn:c3"], "{}", e.name());
        let f = CredentialFilter {
            type_: Some("VerifiableCredential".into()),
            limit: 2,
            ..Default::default()
        };
        assert_eq!(vcs!(&e, opts(), find_credentials(&f)).unwrap().len(), 2);
    }
}

// @lat: [[tests#Verifiable Credentials#SPARQL selects credentials]]
#[test]
fn sparql_selects_credentials() {
    for e in Engine::all() {
        vcs!(&e, opts(), put_credential(&fixture("degree-v2.json"))).unwrap();
        vcs!(&e, opts(), put_credential(&fixture("alumni-v1.json"))).unwrap();
        let QueryResults::Solutions(sols) = e.query("SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s <https://www.w3.org/ns/credentials/examples#degree> ?d } }") else {
            panic!()
        };
        let graphs: Vec<GraphName> = sols
            .map(|s| match s.unwrap().get("g").unwrap() {
                Term::NamedNode(n) => GraphName::NamedNode(n.clone()),
                t => panic!("{t}"),
            })
            .collect();
        assert_eq!(graphs.len(), 1, "{}", e.name());
        let doc = match &e {
            Engine::Native(s) => s
                .credentials()
                .unwrap()
                .documents()
                .document_for_graph(&graphs[0])
                .unwrap(),
            Engine::Dylib(s) => s
                .credentials()
                .unwrap()
                .documents()
                .document_for_graph(&graphs[0])
                .unwrap(),
            Engine::D1(s) => futures::executor::block_on(async {
                s.credentials()
                    .await
                    .unwrap()
                    .documents()
                    .document_for_graph(&graphs[0])
                    .await
                    .unwrap()
            }),
            Engine::Miniflare(s, _) => futures::executor::block_on(async {
                s.credentials()
                    .await
                    .unwrap()
                    .documents()
                    .document_for_graph(&graphs[0])
                    .await
                    .unwrap()
            }),
        };
        assert_eq!(doc.unwrap().key, DEGREE_ID);
    }
}

// @lat: [[tests#Verifiable Credentials#Blank-node labels are stable]]
#[test]
fn blank_node_labels_are_stable() {
    let quads = oxilite::jsonld::document_quads(
        &fixture("degree-v2.json"),
        DEGREE_ID,
        &opts().jsonld,
        &oxilite::vc::bundled_contexts(),
    )
    .unwrap();
    let mut labels: Vec<String> = quads
        .iter()
        .flat_map(|q| {
            let mut v = Vec::new();
            if let oxilite::model::NamedOrBlankNode::BlankNode(b) = &q.subject {
                v.push(b.as_str().to_owned());
            }
            if let Term::BlankNode(b) = &q.object {
                v.push(b.as_str().to_owned());
            }
            if let GraphName::BlankNode(b) = &q.graph_name {
                v.push(b.as_str().to_owned());
            }
            v
        })
        .collect();
    labels.sort();
    labels.dedup();
    // A change here means json-ld relabels differently: re-putting stored credentials would
    // then replace their blank nodes instead of being a no-op.
    let prefix = oxilite::jsonld::blank_prefix(DEGREE_ID);
    assert_eq!(
        labels,
        [
            format!("{prefix}0"),
            format!("{prefix}1"),
            format!("{prefix}2")
        ],
        "{quads:#?}"
    );
    assert_eq!(quads.len(), 16);
}
