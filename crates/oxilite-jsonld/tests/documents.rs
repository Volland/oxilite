//! JSON-LD document storage: the `jsonld-documents` specification scenarios, on the native
//! store, the platform SQLite (dylib), the D1 code path and, with `OXILITE_D1_URL`, Miniflare.

use futures::executor::block_on;
use oxilite::jsonld::{
    DocumentFilter, DocumentInput, GraphStrategy, JsonLdError, JsonLdOptions, KeyStrategy,
    MissingKey, WriteJob,
};
use oxilite::model::{GraphName, NamedNode};
use oxilite::sparql::QueryResults;
use oxilite::Capabilities;

#[path = "engine/mod.rs"]
mod engine;

use engine::{d1_like, Engine};

const PERSON: &str = r#"{
  "@context": {"name": "http://schema.org/name", "knows": {"@id": "http://schema.org/knows", "@type": "@id"}},
  "@id":   "urn:uuid:1234",
  "name":  "Ada é Lovelace",
  "knows": "https://example.org/alan"
}"#;

fn defaults() -> JsonLdOptions {
    JsonLdOptions::default()
}

// @lat: [[tests#JSON-LD documents#Documents round-trip verbatim]]
#[test]
fn documents_round_trip_verbatim() {
    for e in Engine::all() {
        let key = docs!(&e, defaults(), put_document(PERSON)).unwrap();
        assert_eq!(key, "urn:uuid:1234", "{}", e.name());
        let d = docs!(&e, defaults(), get_document(&key)).unwrap().unwrap();
        assert_eq!(d.json, PERSON, "{}", e.name());
        assert_eq!(d.sha256.len(), 64);
        assert_eq!(
            d.graph,
            GraphName::NamedNode(NamedNode::new_unchecked("urn:uuid:1234"))
        );
        assert_eq!(d.meta.profile, "jsonld");
        assert!(d.stored_at > 1.7e9, "{}", e.name());
        assert!(docs!(&e, defaults(), get_document("urn:nope"))
            .unwrap()
            .is_none());
    }
}

// @lat: [[tests#JSON-LD documents#Key strategies]]
#[test]
fn key_strategies() {
    let no_id = r#"{"@context": {"name": "http://schema.org/name"}, "name": "anonymous"}"#;
    let pointed = r#"{"@context": {"name": "http://schema.org/name"}, "@id": "urn:x", "credentialSubject": {"id": "did:example:subject"}, "name": "x"}"#;
    for e in Engine::all() {
        // Reject (the generic default) writes nothing.
        let err = docs!(&e, defaults(), put_document(no_id)).unwrap_err();
        assert!(
            matches!(err, JsonLdError::MissingKey(_)),
            "{}: {err}",
            e.name()
        );
        assert_eq!(e.quads(), 0);
        // Content-hash fallback is stable: storing twice keeps one document.
        let o = JsonLdOptions {
            on_missing_key: MissingKey::ContentHash,
            ..defaults()
        };
        let k1 = docs!(&e, o, put_document(no_id)).unwrap();
        let k2 = docs!(&e, o, put_document(no_id)).unwrap();
        assert_eq!(k1, k2);
        assert!(k1.starts_with("urn:oxilite:doc:sha256:"));
        assert_eq!(docs!(&e, o, list_documents(None, 10)).unwrap().len(), 1);
        // JSON Pointer.
        let o = JsonLdOptions {
            key: KeyStrategy::Pointer("/credentialSubject/id".into()),
            ..defaults()
        };
        assert_eq!(
            docs!(&e, o, put_document(pointed)).unwrap(),
            "did:example:subject"
        );
        // Explicit keys.
        let o = JsonLdOptions {
            key: KeyStrategy::Explicit,
            ..defaults()
        };
        assert!(matches!(
            docs!(&e, o, put_document(pointed)),
            Err(JsonLdError::MissingKey(_))
        ));
        assert_eq!(
            docs!(&e, o, put_document_with_key("urn:mine", pointed)).unwrap(),
            "urn:mine"
        );
    }
}

// @lat: [[tests#JSON-LD documents#Graph strategies]]
#[test]
fn graph_strategies() {
    let doc = r#"{"@context": {"name": "http://schema.org/name"}, "@id": "thing", "name": "x"}"#;
    for e in Engine::all() {
        let o = JsonLdOptions {
            graph: GraphStrategy::Template("https://example.org/g/{key}".into()),
            base_iri: Some("https://example.org/".into()),
            ..defaults()
        };
        docs!(&e, o, put_document_with_key("a b", doc)).unwrap();
        assert!(e.ask("ASK { GRAPH <https://example.org/g/a%20b> { <https://example.org/thing> <http://schema.org/name> \"x\" } }"), "{}", e.name());
        // The key is not an IRI: the key-as-graph strategy refuses it.
        let err = docs!(&e, defaults(), put_document_with_key("a b", doc)).unwrap_err();
        assert!(matches!(err, JsonLdError::InvalidGraphName(_)), "{err}");
        // Default graph.
        let o = JsonLdOptions {
            graph: GraphStrategy::DefaultGraph,
            ..defaults()
        };
        docs!(&e, o, put_document(PERSON)).unwrap();
        assert!(e.ask("ASK { <urn:uuid:1234> <http://schema.org/name> ?n }"));
        let d = docs!(&e, o, get_document("urn:uuid:1234"))
            .unwrap()
            .unwrap();
        assert_eq!(d.graph, GraphName::DefaultGraph);
    }
}

// @lat: [[tests#JSON-LD documents#Invalid JSON-LD is rejected atomically]]
#[test]
fn invalid_json_ld_is_rejected_atomically() {
    for e in Engine::all() {
        let err = docs!(
            &e,
            defaults(),
            put_document(r#"{"@context": 42, "@id": "urn:x"}"#)
        )
        .unwrap_err();
        // The JSON-LD API error code is reported (json-ld 0.21 classifies a number as an
        // invalid context entry).
        assert_eq!(
            err.code(),
            Some("invalid context entry"),
            "{}: {err}",
            e.name()
        );
        let err = docs!(&e, defaults(), put_document("{not json")).unwrap_err();
        assert!(matches!(err, JsonLdError::Json(_)));
        assert_eq!(e.quads(), 0);
        assert!(docs!(&e, defaults(), get_document("urn:x"))
            .unwrap()
            .is_none());
    }
}

const WITH_PROOF: &str = r#"{
  "@context": {
    "name": "http://schema.org/name",
    "proof": {"@id": "https://w3id.org/security#proof", "@type": "@id", "@container": "@graph"},
    "proofValue": "https://w3id.org/security#proofValue"
  },
  "@id": "urn:doc:proofed",
  "name": "claims",
  "proof": {"proofValue": "z3abc"}
}"#;

// @lat: [[tests#JSON-LD documents#Graph containers are owned by the document]]
#[test]
fn graph_containers_are_owned_by_the_document() {
    for e in Engine::all() {
        docs!(&e, defaults(), put_document(WITH_PROOF)).unwrap();
        let graphs = docs!(&e, defaults(), document_graphs("urn:doc:proofed")).unwrap();
        assert_eq!(graphs.len(), 2, "{}: {graphs:?}", e.name());
        assert!(graphs.iter().any(|g| matches!(g, GraphName::BlankNode(_))));
        // Claims and proof live in different graphs.
        assert!(!e.ask(
            "ASK { GRAPH <urn:doc:proofed> { ?s <https://w3id.org/security#proofValue> ?v } }"
        ));
        assert!(e.ask("ASK { GRAPH <urn:doc:proofed> { <urn:doc:proofed> <https://w3id.org/security#proof> ?g } GRAPH ?g { ?p <https://w3id.org/security#proofValue> \"z3abc\" } }"));
        // Both graphs go with the document.
        assert!(docs!(&e, defaults(), remove_document("urn:doc:proofed")).unwrap());
        assert_eq!(e.quads(), 0, "{}", e.name());
        assert_eq!(e.sql_count("SELECT count(*) FROM graphs"), 0);
    }
}

// @lat: [[tests#JSON-LD documents#Blank nodes are document-scoped]]
#[test]
fn blank_nodes_are_document_scoped() {
    let a = r#"{"@context": {"name": "http://schema.org/name", "has": "http://schema.org/has"}, "@id": "urn:a", "has": {"name": "x"}}"#;
    let b = r#"{"@context": {"name": "http://schema.org/name", "has": "http://schema.org/has"}, "@id": "urn:b", "has": {"name": "x"}}"#;
    for e in Engine::all() {
        docs!(&e, defaults(), put_document(a)).unwrap();
        docs!(&e, defaults(), put_document(b)).unwrap();
        assert_eq!(
            e.count("SELECT DISTINCT ?n WHERE { GRAPH ?g { ?n <http://schema.org/name> \"x\" } FILTER isBlank(?n) }"),
            2,
            "{}",
            e.name()
        );
        // Re-storing yields the same labels: nothing changes.
        let before = e.quads();
        docs!(&e, defaults(), put_document(a)).unwrap();
        assert_eq!(e.quads(), before);
        assert_eq!(e.sql_count("SELECT count(*) FROM quads"), before as i64);
    }
}

// @lat: [[tests#JSON-LD documents#Replace is atomic]]
#[test]
fn replace_is_atomic() {
    let v = |n: &str| {
        format!(
            r#"{{"@context": {{"p": "http://example.org/p"}}, "@id": "http://example.org/a", "p": "{n}"}}"#
        )
    };
    // A document defining the named graph <http://example.org/a> (owned by another document).
    let thief = r#"{"@context": {"p": "http://example.org/p"}, "@id": "urn:thief", "@graph": [{"@id": "http://example.org/a", "@graph": {"@id": "urn:s", "p": "stolen"}}]}"#;
    let thief_v1 = r#"{"@context": {"p": "http://example.org/p"}, "@id": "urn:thief", "p": "v1"}"#;
    for e in Engine::all() {
        docs!(&e, defaults(), put_document(&v("1"))).unwrap();
        docs!(&e, defaults(), put_document(&v("2"))).unwrap();
        assert!(e.ask("ASK { GRAPH <http://example.org/a> { ?s <http://example.org/p> \"2\" } }"));
        assert!(
            !e.ask("ASK { GRAPH ?g { ?s <http://example.org/p> \"1\" } }"),
            "{}",
            e.name()
        );
        // A failing replace leaves the previous version whole.
        docs!(&e, defaults(), put_document(thief_v1)).unwrap();
        let err = docs!(&e, defaults(), put_document(thief)).unwrap_err();
        assert!(
            matches!(err, JsonLdError::GraphOwned(_)),
            "{}: {err}",
            e.name()
        );
        let d = docs!(&e, defaults(), get_document("urn:thief"))
            .unwrap()
            .unwrap();
        assert_eq!(d.json, thief_v1);
        assert!(e.ask("ASK { GRAPH <urn:thief> { <urn:thief> <http://example.org/p> \"v1\" } }"));
        assert!(!e.ask("ASK { GRAPH ?g { ?s <http://example.org/p> \"stolen\" } }"));
    }
}

// @lat: [[tests#JSON-LD documents#Remove clears owned graphs only]]
#[test]
fn remove_clears_owned_graphs_only() {
    for e in Engine::all() {
        e.update("INSERT DATA { <urn:x> <urn:p> <urn:y> . GRAPH <urn:other> { <urn:x> <urn:p> <urn:z> } }");
        docs!(&e, defaults(), put_document(PERSON)).unwrap();
        assert!(docs!(&e, defaults(), remove_document("urn:uuid:1234")).unwrap());
        assert!(docs!(&e, defaults(), get_document("urn:uuid:1234"))
            .unwrap()
            .is_none());
        assert_eq!(e.quads(), 2, "{}", e.name());
        assert!(!e.ask("ASK { GRAPH <urn:uuid:1234> { ?s ?p ?o } }"));
        assert_eq!(e.count("SELECT DISTINCT ?g WHERE { GRAPH ?g {} }"), 1);
        // Removing again reports absence.
        assert!(!docs!(&e, defaults(), remove_document("urn:uuid:1234")).unwrap());
    }
}

// @lat: [[tests#JSON-LD documents#Shared graphs delete exact triples]]
#[test]
fn shared_graphs_delete_exact_triples() {
    let doc = |id: &str, v: &str| {
        format!(r#"{{"@context": {{"p": "http://example.org/p"}}, "@id": "{id}", "p": "{v}"}}"#)
    };
    for e in Engine::all() {
        for graph in [
            GraphStrategy::Fixed(NamedNode::new_unchecked("urn:shared")),
            GraphStrategy::DefaultGraph,
        ] {
            let o = JsonLdOptions {
                graph: graph.clone(),
                ..defaults()
            };
            docs!(&e, o, put_document(&doc("urn:a", "1"))).unwrap();
            docs!(&e, o, put_document(&doc("urn:b", "1"))).unwrap();
            docs!(&e, o, put_document(&doc("urn:a", "2"))).unwrap();
            assert_eq!(e.quads(), 2, "{} {graph:?}", e.name());
            assert!(e.ask("ASK { { <urn:a> <http://example.org/p> \"2\" } UNION { GRAPH ?g { <urn:a> <http://example.org/p> \"2\" } } }"));
            assert!(docs!(&e, o, remove_document("urn:b")).unwrap());
            assert!(docs!(&e, o, remove_document("urn:a")).unwrap());
            assert_eq!(e.quads(), 0, "{} {graph:?}", e.name());
        }
    }
}

// @lat: [[tests#JSON-LD documents#Contexts load offline]]
#[test]
fn contexts_load_offline() {
    let ctx = r#"{"@context": {"name": "http://schema.org/name"}}"#;
    let doc =
        r#"{"@context": "https://example.org/ctx.jsonld", "@id": "urn:c", "name": "offline"}"#;
    for e in Engine::all() {
        // Unknown and no network: fails naming the IRI, writes nothing.
        let err = docs!(&e, defaults(), put_document(doc)).unwrap_err();
        assert!(
            matches!(&err, JsonLdError::ContextNotFound(i) if i == "https://example.org/ctx.jsonld"),
            "{}: {err}",
            e.name()
        );
        assert_eq!(e.quads(), 0);
        // Registered in memory.
        let o = defaults().with_context("https://example.org/ctx.jsonld", ctx);
        docs!(&e, o, put_document(doc)).unwrap();
        // Persisted: any later handle (another process, D1) converts it.
        docs!(
            &e,
            defaults(),
            put_context("https://example.org/ctx.jsonld", ctx)
        )
        .unwrap();
        assert_eq!(
            docs!(&e, defaults(), contexts()).unwrap(),
            vec!["https://example.org/ctx.jsonld"]
        );
        docs!(&e, defaults(), remove_document("urn:c")).unwrap();
        docs!(&e, defaults(), put_document(doc)).unwrap();
        assert!(
            e.ask("ASK { GRAPH <urn:c> { <urn:c> <http://schema.org/name> \"offline\" } }"),
            "{}",
            e.name()
        );
    }
}

// @lat: [[tests#JSON-LD documents#Nested contexts load in rounds]]
#[test]
fn nested_contexts_load_in_rounds() {
    let outer =
        r#"{"@context": ["https://example.org/inner.jsonld", {"age": "http://schema.org/age"}]}"#;
    let inner = r#"{"@context": {"name": "http://schema.org/name"}}"#;
    let doc = r#"{"@context": "https://example.org/outer.jsonld", "@id": "urn:n", "name": "n", "age": 3}"#;
    for e in Engine::all() {
        docs!(
            &e,
            defaults(),
            put_context("https://example.org/outer.jsonld", outer)
        )
        .unwrap();
        docs!(
            &e,
            defaults(),
            put_context("https://example.org/inner.jsonld", inner)
        )
        .unwrap();
        docs!(&e, defaults(), put_document(doc)).unwrap();
        assert!(e.ask("ASK { GRAPH <urn:n> { <urn:n> <http://schema.org/name> \"n\" ; <http://schema.org/age> 3 } }"), "{}", e.name());
    }
}

// @lat: [[tests#JSON-LD documents#SPARQL finds documents]]
#[test]
fn sparql_finds_documents() {
    for e in Engine::all() {
        docs!(&e, defaults(), put_document(PERSON)).unwrap();
        e.update("INSERT DATA { GRAPH <urn:plain> { <urn:x> <urn:p> 1 } }");
        assert_eq!(
            e.count("SELECT ?p ?o WHERE { GRAPH <urn:uuid:1234> { ?s ?p ?o } }"),
            2
        );
        let QueryResults::Solutions(sols) =
            e.query("SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } } ORDER BY ?g")
        else {
            panic!()
        };
        let mut found = Vec::new();
        for s in sols {
            let s = s.unwrap();
            let g = match s.get("g").unwrap() {
                oxilite::model::Term::NamedNode(n) => GraphName::NamedNode(n.clone()),
                _ => panic!(),
            };
            found.push(
                docs!(&e, defaults(), document_for_graph(&g))
                    .unwrap()
                    .map(|d| d.key),
            );
        }
        assert_eq!(
            found,
            vec![None, Some("urn:uuid:1234".to_owned())],
            "{}",
            e.name()
        );
    }
}

// @lat: [[tests#JSON-LD documents#Check and rebuild repair drift]]
#[test]
fn check_and_rebuild_repair_drift() {
    for e in Engine::all() {
        docs!(&e, defaults(), put_document(PERSON)).unwrap();
        docs!(&e, defaults(), put_document(WITH_PROOF)).unwrap();
        assert!(
            docs!(&e, defaults(), check_documents()).unwrap().is_empty(),
            "{}",
            e.name()
        );
        e.update("DELETE WHERE { GRAPH <urn:uuid:1234> { ?s <http://schema.org/name> ?n } }; INSERT DATA { GRAPH <urn:uuid:1234> { <urn:x> <urn:p> <urn:y> } }");
        let drift = docs!(&e, defaults(), check_documents()).unwrap();
        assert_eq!(drift.len(), 1, "{}: {drift:?}", e.name());
        assert_eq!(
            (drift[0].key.as_str(), drift[0].missing, drift[0].extra),
            ("urn:uuid:1234", 1, 1)
        );
        assert!(docs!(&e, defaults(), rebuild_graph("urn:uuid:1234")).unwrap());
        assert!(docs!(&e, defaults(), check_documents()).unwrap().is_empty());
        assert!(e.ask("ASK { GRAPH <urn:uuid:1234> { ?s <http://schema.org/name> ?n } }"));
        assert!(!docs!(&e, defaults(), rebuild_graph("urn:none")).unwrap());
    }
}

// @lat: [[tests#JSON-LD documents#Metadata lookup]]
#[test]
fn metadata_lookup() {
    for e in Engine::all() {
        let mut inputs = Vec::new();
        for (i, (issuer, until)) in [("did:a", 100.0), ("did:a", 300.0), ("did:b", 300.0)]
            .iter()
            .enumerate()
        {
            let mut d = DocumentInput::new(format!(
                r#"{{"@context": {{"p": "http://example.org/p"}}, "@id": "urn:m{i}", "p": {i}}}"#
            ));
            d.meta.issuer = Some((*issuer).into());
            d.meta.types = vec!["Thing".into(), format!("T{i}")];
            d.meta.valid_until = Some(*until);
            inputs.push(d);
        }
        docs!(&e, defaults(), put_documents(inputs)).unwrap();
        let f = DocumentFilter {
            issuer: Some("did:a".into()),
            valid_at: Some(200.0),
            ..Default::default()
        };
        let found = docs!(&e, defaults(), find_documents(&f)).unwrap();
        assert_eq!(
            found.iter().map(|d| d.key.as_str()).collect::<Vec<_>>(),
            ["urn:m1"],
            "{}",
            e.name()
        );
        let f = DocumentFilter {
            type_: Some("T2".into()),
            ..Default::default()
        };
        assert_eq!(
            docs!(&e, defaults(), find_documents(&f)).unwrap()[0].key,
            "urn:m2"
        );
        let f = DocumentFilter {
            type_: Some("T".into()),
            ..Default::default()
        };
        assert!(docs!(&e, defaults(), find_documents(&f))
            .unwrap()
            .is_empty());
        // Keyset paging.
        let page = docs!(&e, defaults(), list_documents(Some("urn:m0"), 1)).unwrap();
        assert_eq!(page[0].key, "urn:m1");
        assert_eq!(
            page[0].meta.types,
            vec!["Thing".to_owned(), "T1".to_owned()]
        );
    }
}

// @lat: [[tests#JSON-LD documents#Tables are created on demand]]
#[test]
fn tables_are_created_on_demand() {
    for e in Engine::all() {
        let tables =
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'jsonld_%' AND type = 'table'";
        e.update("INSERT DATA { <urn:s> <urn:p> <urn:o> }");
        assert_eq!(e.sql_count(tables), 0, "{}", e.name());
        let version =
            "SELECT CAST(value AS INTEGER) FROM oxilite_meta WHERE key = 'schema_version'";
        let before = e.sql_count(version);
        docs!(&e, defaults(), list_documents(None, 1)).unwrap();
        assert_eq!(e.sql_count(tables), 3);
        assert_eq!(e.sql_count(version), before);
        assert_eq!(e.quads(), 1);
        // Index choice.
        let idx =
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name LIKE 'jsonld_%'";
        assert_eq!(e.sql_count(idx), 3);
    }
}

// @lat: [[tests#JSON-LD documents#Large documents stay atomic]]
#[test]
fn large_documents_stay_atomic() {
    // A document far above the (shrunken) SQL-length limit is appended in chunks.
    let big: String = (0..400)
        .map(|i| format!("\"it's {i}\""))
        .collect::<Vec<_>>()
        .join(",");
    let doc = format!(
        r#"{{"@context": {{"p": "http://example.org/p"}}, "@id": "urn:big", "p": [{big}]}}"#
    );
    let caps = Capabilities {
        max_sql_len: 3_000,
        ..Capabilities::d1()
    };
    let store = d1_like(caps.clone());
    let docs = block_on(store.jsonld()).unwrap();
    block_on(docs.put_document(&doc)).unwrap();
    assert_eq!(
        block_on(docs.get_document("urn:big"))
            .unwrap()
            .unwrap()
            .json,
        doc
    );
    // Too many statements for one batch: refused, not split.
    let caps = Capabilities {
        max_statements: 5,
        ..caps
    };
    let job = WriteJob::new(
        vec![DocumentInput::new(doc)],
        vec![],
        defaults(),
        oxilite::jsonld::NoContexts,
        caps,
    );
    let mut job = job.unwrap();
    let err = job.step_jsonld(None).unwrap_err();
    assert!(
        matches!(err, JsonLdError::DocumentTooLarge { limit: 5, .. }),
        "{err}"
    );
}
