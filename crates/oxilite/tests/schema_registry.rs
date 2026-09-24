//! The schema registry: ontology and shapes graphs, scoped reasoning, schema-graph hiding and
//! the compiled shape index.

use oxilite::io::RdfFormat;
use oxilite::model::{GraphName, NamedNode, Term};
use oxilite::schema::{Registration, SchemaRole};
use oxilite::sparql::{QueryOptions, Reasoning};
use oxilite::store::Store;
use oxilite_core::{QueryOutput, SyncBackend};

const PREFIXES: &str = "@prefix ex: <http://example.com/> . \
     @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
     @prefix owl: <http://www.w3.org/2002/07/owl#> . \
     @prefix sh: <http://www.w3.org/ns/shacl#> . \
     @prefix xsd: <http://www.w3.org/2001/XMLSchema#> . ";

fn store() -> Store {
    Store::new().unwrap()
}

fn load<B: SyncBackend + Send + Sync + 'static>(store: &Store<B>, trig: &str) {
    store
        .load_from_slice(RdfFormat::TriG, format!("{PREFIXES}{trig}").as_bytes())
        .unwrap();
}

fn graph(iri: &str) -> GraphName {
    NamedNode::new(iri).unwrap().into()
}

/// Values of the first variable, as N-Triples strings, sorted.
fn values<B: SyncBackend + Send + Sync + 'static>(
    store: &Store<B>,
    query: &str,
    options: &QueryOptions,
) -> Vec<String> {
    let q = format!(
        "PREFIX ex: <http://example.com/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
         PREFIX owl: <http://www.w3.org/2002/07/owl#> {query}"
    );
    match store.query_output(q.as_str(), options).unwrap() {
        QueryOutput::Solutions { rows, .. } => {
            let mut v: Vec<String> = rows
                .iter()
                .filter_map(|r| r.first().cloned().flatten())
                .map(|t: Term| t.to_string())
                .collect();
            v.sort();
            v
        }
        _ => panic!("expected solutions"),
    }
}

fn rdfs() -> QueryOptions {
    QueryOptions {
        reasoning: Reasoning::Rdfs,
        union_default_graph: true,
        ..QueryOptions::default()
    }
}

// @lat: [[tests#Schema registry#Registration round-trip]]
#[test]
fn registers_lists_and_unregisters() {
    let store = store();
    load(
        &store,
        "GRAPH ex:onto { ex:Dog rdfs:subClassOf ex:Animal }",
    );
    let g = graph("http://example.com/onto");
    store
        .register_schema_graph(
            g.as_ref(),
            SchemaRole::Ontology,
            &Registration::new()
                .with_iri(NamedNode::new("http://example.com/onto").unwrap())
                .with_version("v1")
                .with_imports([NamedNode::new("http://example.com/base").unwrap()]),
        )
        .unwrap();

    let listed = store.schema_graphs().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].graph, g);
    assert_eq!(listed[0].role, SchemaRole::Ontology);
    assert_eq!(listed[0].registration.version.as_deref(), Some("v1"));
    assert_eq!(listed[0].registration.imports.len(), 1);
    assert!(listed[0].registration.active);

    // Registering does not move triples: the graph is still ordinary RDF.
    assert_eq!(
        values(
            &store,
            "SELECT ?o WHERE { GRAPH <http://example.com/onto> { ex:Dog rdfs:subClassOf ?o } }",
            &QueryOptions::default()
        ),
        ["<http://example.com/Animal>"]
    );

    assert!(store.unregister_schema_graph(g.as_ref()).unwrap());
    assert!(store.schema_graphs().unwrap().is_empty());
    // …and unregistering does not remove them either.
    assert_eq!(store.len().unwrap(), 1);
}

// @lat: [[tests#Schema registry#Reasoning scoped to registered ontologies]]
#[test]
fn reasoning_is_scoped_to_active_ontologies() {
    let store = store();
    load(
        &store,
        "GRAPH ex:good { ex:Dog rdfs:subClassOf ex:Animal } \
         GRAPH ex:bad { ex:Rock rdfs:subClassOf ex:Animal } \
         ex:rex a ex:Dog . ex:granite a ex:Rock .",
    );
    let q = "SELECT ?x WHERE { ?x a ex:Animal }";

    // Nothing registered: every graph contributes, as before the registry existed.
    assert_eq!(
        values(&store, q, &rdfs()),
        ["<http://example.com/granite>", "<http://example.com/rex>"]
    );

    let good = graph("http://example.com/good");
    let bad = graph("http://example.com/bad");
    store
        .register_schema_graph(good.as_ref(), SchemaRole::Ontology, &Registration::new())
        .unwrap();
    store
        .register_schema_graph(bad.as_ref(), SchemaRole::Ontology, &Registration::new())
        .unwrap();
    assert_eq!(
        values(&store, q, &rdfs()),
        ["<http://example.com/granite>", "<http://example.com/rex>"]
    );

    // Deactivating one ontology withdraws its entailments.
    assert!(store.set_schema_graph_active(bad.as_ref(), false).unwrap());
    assert_eq!(values(&store, q, &rdfs()), ["<http://example.com/rex>"]);

    // Reactivating restores them, with no reload of data.
    assert!(store.set_schema_graph_active(bad.as_ref(), true).unwrap());
    assert_eq!(
        values(&store, q, &rdfs()),
        ["<http://example.com/granite>", "<http://example.com/rex>"]
    );
}

// @lat: [[tests#Schema registry#Axioms outside registered ontologies]]
#[test]
fn axioms_outside_registered_ontologies_are_ignored() {
    let store = store();
    load(
        &store,
        "GRAPH ex:onto { ex:Dog rdfs:subClassOf ex:Animal } \
         GRAPH ex:notes { ex:Rock rdfs:subClassOf ex:Animal } \
         ex:rex a ex:Dog . ex:granite a ex:Rock .",
    );
    store
        .register_schema_graph(
            graph("http://example.com/onto").as_ref(),
            SchemaRole::Ontology,
            &Registration::new(),
        )
        .unwrap();
    assert_eq!(
        values(&store, "SELECT ?x WHERE { ?x a ex:Animal }", &rdfs()),
        ["<http://example.com/rex>"]
    );
}

// @lat: [[tests#Schema registry#Hiding schema graphs]]
#[test]
fn schema_graphs_can_be_hidden() {
    let store = store();
    load(
        &store,
        "GRAPH ex:onto { ex:Dog rdfs:subClassOf ex:Animal } ex:rex a ex:Dog .",
    );
    let visible = QueryOptions {
        union_default_graph: true,
        ..QueryOptions::default()
    };
    let hidden = QueryOptions {
        include_schema_graphs: false,
        ..visible.clone()
    };
    let q = "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }";

    // Not registered yet: hiding changes nothing.
    assert_eq!(values(&store, q, &visible), values(&store, q, &hidden));

    store
        .register_schema_graph(
            graph("http://example.com/onto").as_ref(),
            SchemaRole::Ontology,
            &Registration::new(),
        )
        .unwrap();
    assert_eq!(
        values(&store, q, &visible),
        ["\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"]
    );
    assert_eq!(
        values(&store, q, &hidden),
        ["\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>"]
    );

    // Hiding ignores the active flag: an inactive ontology graph is still schema.
    store
        .set_schema_graph_active(graph("http://example.com/onto").as_ref(), false)
        .unwrap();
    assert_eq!(
        values(&store, q, &hidden),
        ["\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>"]
    );
}

// @lat: [[tests#Schema registry#Dropping a schema graph]]
#[test]
fn drop_removes_registration_and_quads() {
    let store = store();
    load(
        &store,
        "GRAPH ex:onto { ex:Dog rdfs:subClassOf ex:Animal } ex:rex a ex:Dog .",
    );
    let g = graph("http://example.com/onto");
    store
        .register_schema_graph(g.as_ref(), SchemaRole::Ontology, &Registration::new())
        .unwrap();
    assert_eq!(store.drop_schema_graph(g.as_ref()).unwrap(), 1);
    assert!(store.schema_graphs().unwrap().is_empty());
    assert_eq!(store.len().unwrap(), 1);
    assert!(values(&store, "SELECT ?x WHERE { ?x a ex:Animal }", &rdfs()).is_empty());
}

const SHAPES: &str = "GRAPH ex:shapes { \
     ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; \
       sh:property [ sh:path ex:age ; sh:datatype xsd:integer ; sh:minCount 1 ; sh:maxCount 1 ] ; \
       sh:property [ sh:path ex:status ; sh:in ( \"on\" \"off\" ) ] ; \
       sh:property [ sh:path ex:name ; sh:pattern \"^[A-Z]\" ] ; \
       sh:property [ sh:path ex:employer ; sh:class ex:Company ] . }";

// @lat: [[tests#Schema registry#Compiled shape index]]
#[test]
fn shape_index_is_compiled_on_write() {
    let store = store();
    load(&store, SHAPES);
    // No `optimize()`: the index is refreshed inside the write that stored the shapes.
    let index = store.shape_index().unwrap();
    let person = NamedNode::new("http://example.com/Person").unwrap();

    let age = index
        .get(&person, &NamedNode::new("http://example.com/age").unwrap())
        .expect("age shape");
    assert_eq!(
        age.datatype.as_ref().map(|d| d.as_str()),
        Some("http://www.w3.org/2001/XMLSchema#integer")
    );
    assert_eq!(age.min, Some(1));
    assert_eq!(age.max, Some(1));
    assert!(!age.relationship);

    let status = index
        .get(&person, &NamedNode::new("http://example.com/status").unwrap())
        .expect("status shape");
    let mut got: Vec<String> = status.values_in.iter().map(Term::to_string).collect();
    got.sort();
    assert_eq!(got, ["\"off\"", "\"on\""]);

    let name = index
        .get(&person, &NamedNode::new("http://example.com/name").unwrap())
        .expect("name shape");
    assert_eq!(name.pattern.as_deref(), Some("^[A-Z]"));

    let employer = index
        .get(&person, &NamedNode::new("http://example.com/employer").unwrap())
        .expect("employer shape");
    assert!(employer.relationship);
}

// @lat: [[tests#Schema registry#Shape index follows deletions]]
#[test]
fn shape_index_follows_deletions_and_scope() {
    let store = store();
    load(&store, SHAPES);
    load(
        &store,
        "GRAPH ex:other { ex:OtherShape sh:targetClass ex:Thing ; \
           sh:property [ sh:path ex:label ; sh:minCount 1 ] . }",
    );
    let thing = NamedNode::new("http://example.com/Thing").unwrap();
    let label = NamedNode::new("http://example.com/label").unwrap();
    assert!(store.shape_index().unwrap().get(&thing, &label).is_some());

    // Registering one shapes graph narrows the index to it.
    store
        .register_schema_graph(
            graph("http://example.com/shapes").as_ref(),
            SchemaRole::Shacl,
            &Registration::new(),
        )
        .unwrap();
    assert!(store.shape_index().unwrap().get(&thing, &label).is_none());

    // Deleting the shape triples empties the index.
    store
        .update(
            "PREFIX sh: <http://www.w3.org/ns/shacl#> \
             DELETE WHERE { GRAPH <http://example.com/shapes> { ?s sh:targetClass ?o } }",
        )
        .unwrap();
    assert!(store.shape_index().unwrap().is_empty());
}

// @lat: [[tests#Schema registry#Shapes merged across shapes]]
#[test]
fn shapes_targeting_the_same_path_merge() {
    let store = store();
    load(
        &store,
        "ex:A sh:targetClass ex:Person ; sh:property [ sh:path ex:age ; sh:minCount 1 ] . \
         ex:B sh:targetClass ex:Person ; sh:property [ sh:path ex:age ; sh:datatype xsd:integer ] .",
    );
    let shape = store
        .shape_index()
        .unwrap()
        .get(
            &NamedNode::new("http://example.com/Person").unwrap(),
            &NamedNode::new("http://example.com/age").unwrap(),
        )
        .cloned()
        .expect("merged shape");
    assert_eq!(shape.min, Some(1));
    assert_eq!(
        shape.datatype.as_ref().map(|d| d.as_str()),
        Some("http://www.w3.org/2001/XMLSchema#integer")
    );
}

// @lat: [[tests#Schema registry#Registry survives reopening]]
#[test]
fn registry_survives_reopening() {
    let path = std::env::temp_dir().join(format!("oxilite-registry-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let g = graph("http://example.com/onto");
    {
        let store = Store::open(&path).unwrap();
        load(
            &store,
            "GRAPH ex:onto { ex:Dog rdfs:subClassOf ex:Animal } ex:rex a ex:Dog .",
        );
        store
            .register_schema_graph(
                g.as_ref(),
                SchemaRole::Ontology,
                &Registration::new().with_version("v2"),
            )
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let listed = store.schema_graphs().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].graph, g);
    assert_eq!(listed[0].registration.version.as_deref(), Some("v2"));
    assert_eq!(
        values(&store, "SELECT ?x WHERE { ?x a ex:Animal }", &rdfs()),
        ["<http://example.com/rex>"]
    );
    drop(store);
    let _ = std::fs::remove_file(&path);
}
