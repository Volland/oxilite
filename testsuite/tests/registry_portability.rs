//! The schema registry is portable RDF: the SPARQL oxilite generates for it runs unchanged on
//! Oxigraph, reads back the same registrations, and leaves the same `<oxilite:schema>` graph as
//! on oxilite.
#![allow(deprecated)]

use oxigraph::sparql::QueryResults;
use oxilite_core::registry::{self, SchemaGraph, SchemaRole};
use oxrdf::{GraphName, GraphNameRef, NamedNode, Quad, Term};

fn node(iri: &str) -> NamedNode {
    NamedNode::new(iri).unwrap()
}

fn entry() -> SchemaGraph {
    let mut e = SchemaGraph::new(node("http://ex.org/onto").into(), SchemaRole::Ontology);
    e.version = Some("2.1".into());
    e.sha256 = Some("9f2c".into());
    e.iri = Some(node("http://ex.org/hr#"));
    e.imports = vec![node("http://xmlns.com/foaf/0.1/")];
    e.applies_to = vec![GraphName::DefaultGraph, node("http://ex.org/staff").into()];
    e.loaded_at = Some("2026-09-26T10:00:00Z".into());
    e
}

fn oxigraph_entries(store: &oxigraph::store::Store) -> Vec<SchemaGraph> {
    let QueryResults::Solutions(solutions) =
        store.query(registry::entries_query().as_str()).unwrap()
    else {
        panic!("solutions expected")
    };
    let rows: Vec<Vec<Option<Term>>> = solutions
        .map(|s| {
            let s = s.unwrap();
            ["g", "p", "o"].iter().map(|v| s.get(*v).cloned()).collect()
        })
        .collect();
    registry::entries_from_rows(&rows)
}

fn schema_quads<I: IntoIterator<Item = Quad>>(quads: I) -> Vec<String> {
    let mut v: Vec<String> = quads
        .into_iter()
        .filter(|q| matches!(&q.graph_name, GraphName::NamedNode(g) if g.as_str() == registry::SCHEMA_GRAPH))
        .map(|q| q.to_string())
        .collect();
    v.sort();
    v
}

// @lat: [[tests#Schema registry#Registry SPARQL runs on Oxigraph]]
#[test]
fn registry_sparql_runs_on_oxigraph() {
    let e = entry();
    let graph = GraphNameRef::from(e.graph.as_ref());
    let oxigraph = oxigraph::store::Store::new().unwrap();
    oxigraph
        .update(registry::register_update(&e).unwrap().as_str())
        .unwrap();
    let mut want = e.clone();
    want.applies_to.sort_by_key(ToString::to_string);
    assert_eq!(oxigraph_entries(&oxigraph), vec![want.clone()]);

    // oxilite writes the very same registry graph for the same update.
    let oxilite = oxilite::store::Store::new().unwrap();
    oxilite
        .update(registry::register_update(&e).unwrap().as_str())
        .unwrap();
    assert_eq!(
        schema_quads(oxigraph.iter().map(Result::unwrap)),
        schema_quads(oxilite.iter().map(Result::unwrap))
    );
    assert_eq!(oxilite.schema_graphs().unwrap()[0].to_entry(), want);

    // Every other operation is plain SPARQL too.
    let ask = |q: String| {
        matches!(
            oxigraph.query(q.as_str()).unwrap(),
            QueryResults::Boolean(true)
        )
    };
    assert!(ask(registry::registered_query(graph).unwrap()));
    oxigraph
        .update(registry::set_active_update(graph, false).unwrap().as_str())
        .unwrap();
    assert!(!oxigraph_entries(&oxigraph)[0].active);
    oxigraph
        .update(registry::unregister_update(graph).unwrap().as_str())
        .unwrap();
    assert!(oxigraph_entries(&oxigraph).is_empty());
    assert!(!ask(registry::registered_query(graph).unwrap()));

    oxigraph
        .update(registry::register_update(&e).unwrap().as_str())
        .unwrap();
    oxigraph
        .update(registry::drop_update(graph).unwrap().as_str())
        .unwrap();
    assert!(oxigraph_entries(&oxigraph).is_empty());
    assert!(!oxigraph
        .contains_named_graph(node("http://ex.org/onto").as_ref())
        .unwrap());
    // The system graphs install by SPARQL, and match what a blank oxilite store starts with.
    let fresh = oxigraph::store::Store::new().unwrap();
    fresh
        .update(registry::system_graphs_update().as_str())
        .unwrap();
    let blank = oxilite::store::Store::with_backend_and_options(
        oxilite::rusqlite::RusqliteBackend::memory().unwrap(),
        &oxilite::StoreOptions {
            system_graphs: true,
            ..Default::default()
        },
    )
    .unwrap();
    let all = |quads: Vec<Quad>| {
        let mut v: Vec<String> = quads.iter().map(ToString::to_string).collect();
        v.sort();
        v
    };
    assert_eq!(
        all(fresh.iter().map(Result::unwrap).collect()),
        all(blank.iter().map(Result::unwrap).collect())
    );
    assert!(matches!(
        fresh
            .query(registry::system_graphs_ready_query().as_str())
            .unwrap(),
        QueryResults::Boolean(true)
    ));
    // The website publishes the same vocabulary.
    let published = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../site/ns/oxl.ttl"),
    )
    .unwrap();
    assert_eq!(
        published,
        registry::VOCABULARY,
        "site/ns/oxl.ttl is out of date"
    );
    // The vocabulary itself is valid Turtle.
    oxigraph
        .load_from_slice(
            oxigraph::io::RdfFormat::Turtle,
            registry::VOCABULARY.as_bytes(),
        )
        .unwrap();
}
