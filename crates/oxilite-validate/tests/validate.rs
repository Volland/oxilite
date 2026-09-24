//! SHACL / ShEx over stores, and the bounded prefetch used for D1.

use oxilite::io::RdfFormat;
use oxilite::model::Term;
use oxilite::rusqlite::RusqliteBackend;
use oxilite::store::Store;
use oxilite::AsyncStore;
use oxilite_core::{AsyncBackend, Capabilities, Request, Response, SyncBackend};
use oxilite_validate::prefetch::{validate_shacl_async, validate_shex_async, PrefetchOptions};
use oxilite_validate::{validate_shacl, validate_shex, Error, ShaclValidationMode};

const DATA: &str = r#"
@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
ex:Student rdfs:subClassOf ex:Person .
ex:alice a ex:Person ; ex:name "Alice" ; ex:address [ ex:city "Paris" ] ; ex:knows ex:bob .
ex:bob a ex:Person , ex:Student ; ex:address [ ex:zip "123" ] .
ex:carol a ex:Person ; ex:name "Carol" .
ex:dave ex:worksFor ex:acme .
"#;

const SHAPES: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.com/> .
ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:address ; sh:node ex:AddressShape ] ;
  sh:property [ sh:path [ sh:inversePath ex:knows ] ; sh:maxCount 1 ] .
ex:AddressShape a sh:NodeShape ; sh:property [ sh:path ex:city ; sh:minCount 1 ] .
ex:EmployerShape a sh:NodeShape ; sh:targetObjectsOf ex:worksFor ; sh:property [ sh:path ex:name ; sh:minCount 1 ] .
"#;

fn store() -> Store {
    let s = Store::new().unwrap();
    s.load_from_slice(RdfFormat::Turtle, DATA.as_bytes())
        .unwrap();
    s
}

fn focus_nodes(r: &oxilite_validate::ValidationReport) -> Vec<String> {
    let mut v: Vec<String> = r
        .results()
        .iter()
        .map(|r| r.focus_node().to_string())
        .collect();
    v.sort();
    v
}

// @lat: [[tests#Validation#SHACL violation reported]]
#[test]
fn shacl_violation_reported() {
    for mode in [ShaclValidationMode::Native, ShaclValidationMode::Sparql] {
        let report = validate_shacl(&store(), SHAPES, &mode).unwrap();
        assert!(!report.conforms());
        let nodes = focus_nodes(&report);
        // bob has no name and his address has no city; acme (a worksFor object) has no name.
        assert!(nodes.iter().any(|n| n.contains("bob")), "{nodes:?}");
        assert!(nodes.iter().any(|n| n.contains("acme")), "{nodes:?}");
        assert!(
            !nodes
                .iter()
                .any(|n| n.contains("alice") || n.contains("carol")),
            "{nodes:?}"
        );
    }
}

// @lat: [[tests#Validation#ShEx conforming node]]
#[test]
fn shex_conforming_node() {
    let schema =
        "PREFIX ex: <http://example.com/> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> \
                  ex:PersonShape { ex:name xsd:string ; ex:knows @ex:PersonShape * }";
    let r = validate_shex(
        &store(),
        schema,
        "http://example.com/",
        "<http://example.com/carol>@<http://example.com/PersonShape>",
    )
    .unwrap();
    assert!(r.iter().all(|(_, _, s)| s.is_conformant()));
    let r = validate_shex(
        &store(),
        schema,
        "http://example.com/",
        "<http://example.com/alice>@<http://example.com/PersonShape>",
    )
    .unwrap();
    assert!(
        r.iter().any(|(_, _, s)| s.is_non_conformant()),
        "alice knows bob, who has no name"
    );
}

/// A rusqlite database behind the async interface, with D1's capabilities (TEXT ids, 50
/// statements per batch, …): the code path of a D1 store.
struct D1Like(RusqliteBackend, Capabilities);

impl AsyncBackend for D1Like {
    async fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        self.0.execute(request)
    }
    fn capabilities(&self) -> &Capabilities {
        &self.1
    }
}

/// The Miniflare D1 sidecar (`testsuite/d1-sidecar`), when `OXILITE_D1_URL` is set.
struct HttpD1(String, Capabilities);

impl AsyncBackend for HttpD1 {
    async fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        let body = serde_json::to_value(request).map_err(oxilite_core::Error::backend)?;
        match ureq::post(&format!("{}/execute", self.0)).send_json(body) {
            Ok(r) => r.into_json().map_err(oxilite_core::Error::backend),
            Err(ureq::Error::Status(_, r)) => {
                let v: serde_json::Value = r.into_json().unwrap_or_default();
                Err(oxilite_core::Error::backend(
                    v["error"].as_str().unwrap_or("D1 error"),
                ))
            }
            Err(e) => Err(oxilite_core::Error::backend(e)),
        }
    }
    fn capabilities(&self) -> &Capabilities {
        &self.1
    }
}

async fn check_prefetch<B: AsyncBackend>(store: AsyncStore<B>) {
    store
        .load_from_slice(RdfFormat::Turtle, DATA.as_bytes())
        .await
        .unwrap();
    for mode in [ShaclValidationMode::Native, ShaclValidationMode::Sparql] {
        let full = validate_shacl(&self::store(), SHAPES, &mode).unwrap();
        let prefetched = validate_shacl_async(&store, SHAPES, &mode, &PrefetchOptions::default())
            .await
            .unwrap();
        assert_eq!(full.conforms(), prefetched.conforms());
        assert_eq!(focus_nodes(&full), focus_nodes(&prefetched));
        assert_eq!(full.results().len(), prefetched.results().len());
    }
    // The bound is enforced, not silently truncated.
    let small = PrefetchOptions {
        max_triples: 5,
        ..PrefetchOptions::default()
    };
    match validate_shacl_async(&store, SHAPES, &ShaclValidationMode::Native, &small).await {
        Err(Error::TooLarge { limit: 5, found }) => assert!(found > 5),
        other => panic!("expected TooLarge, got {:?}", other.map(|r| r.conforms())),
    }
    let schema = "PREFIX ex: <http://example.com/> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> ex:S { ex:name xsd:string }";
    let carol = Term::from(oxilite::model::NamedNode::new_unchecked(
        "http://example.com/carol",
    ));
    let r = validate_shex_async(
        &store,
        schema,
        "http://example.com/",
        "<http://example.com/carol>@<http://example.com/S>",
        &[carol],
        &PrefetchOptions::default(),
    )
    .await
    .unwrap();
    assert!(r.iter().all(|(_, _, s)| s.is_conformant()));
}

// @lat: [[tests#Validation#Bounded prefetch matches full validation]]
#[test]
fn bounded_prefetch_matches_full_validation() {
    futures::executor::block_on(async {
        let backend = D1Like(RusqliteBackend::memory().unwrap(), Capabilities::d1());
        check_prefetch(AsyncStore::open(backend).await.unwrap()).await;
        if let Ok(url) = std::env::var("OXILITE_D1_URL") {
            ureq::post(&format!("{url}/reset")).call().unwrap();
            check_prefetch(
                AsyncStore::open(HttpD1(url, Capabilities::d1()))
                    .await
                    .unwrap(),
            )
            .await;
        }
    });
}

// ----- shapes held in the store -----

/// Loads the shapes into `graph` of a store that already holds the data.
fn store_with_shapes(graph: &str, register: bool) -> Store {
    let s = store();
    let body = SHAPES
        .replace("@prefix sh: <http://www.w3.org/ns/shacl#> .", "")
        .replace("@prefix ex: <http://example.com/> .", "");
    s.load_from_slice(
        RdfFormat::TriG,
        format!(
            "@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.com/> . \
             GRAPH <{graph}> {{ {body} }}"
        )
        .as_bytes(),
    )
    .unwrap();
    if register {
        register_shapes(&s, graph);
    }
    s
}

fn register_shapes(s: &Store, graph: &str) {
    s.register_schema_graph(
        oxilite::model::NamedNodeRef::new(graph).unwrap(),
        oxilite::schema::SchemaRole::Shacl,
        &oxilite::schema::Registration::new(),
    )
    .unwrap();
}

// @lat: [[tests#Validation#Shapes read from the store]]
#[test]
fn stored_shapes_give_the_same_report() {
    let graph = "http://example.com/shapes";
    let s = store_with_shapes(graph, true);
    let from_text = validate_shacl(&s, SHAPES, &ShaclValidationMode::Native).unwrap();
    let from_store =
        oxilite_validate::validate_shacl_stored(&s, None, &ShaclValidationMode::Native).unwrap();
    assert_eq!(focus_nodes(&from_store), focus_nodes(&from_text));
    assert_eq!(from_store.conforms(), from_text.conforms());

    // Naming the graph explicitly gives the same result.
    let named = oxilite_validate::validate_shacl_stored(
        &s,
        Some(oxilite::model::NamedNodeRef::new(graph).unwrap().into()),
        &ShaclValidationMode::Native,
    )
    .unwrap();
    assert_eq!(focus_nodes(&named), focus_nodes(&from_text));
}

// @lat: [[tests#Validation#Stored shapes need an unambiguous graph]]
#[test]
fn stored_shapes_need_an_unambiguous_graph() {
    // Nothing registered.
    let s = store_with_shapes("http://example.com/shapes", false);
    assert!(matches!(
        oxilite_validate::validate_shacl_stored(&s, None, &ShaclValidationMode::Native),
        Err(Error::NoShapesGraph)
    ));

    // Two registered shapes graphs: refuse to guess.
    let s = store_with_shapes("http://example.com/shapes", true);
    register_shapes(&s, "http://example.com/other");
    let err = oxilite_validate::validate_shacl_stored(&s, None, &ShaclValidationMode::Native)
        .expect_err("two shapes graphs must be ambiguous");
    assert!(matches!(err, Error::AmbiguousShapesGraph(2, _)), "{err}");
}
