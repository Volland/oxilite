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
    load(&store, "GRAPH ex:onto { ex:Dog rdfs:subClassOf ex:Animal }");
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
    // Visible: the data, the ontology, and the registry graph's description of it (type,
    // active flag, registration time).
    assert_eq!(
        values(&store, q, &visible),
        ["\"5\"^^<http://www.w3.org/2001/XMLSchema#integer>"]
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
        .get(
            &person,
            &NamedNode::new("http://example.com/status").unwrap(),
        )
        .expect("status shape");
    let mut got: Vec<String> = status.values_in.iter().map(Term::to_string).collect();
    got.sort();
    assert_eq!(got, ["\"off\"", "\"on\""]);

    let name = index
        .get(&person, &NamedNode::new("http://example.com/name").unwrap())
        .expect("name shape");
    assert_eq!(name.pattern.as_deref(), Some("^[A-Z]"));

    let employer = index
        .get(
            &person,
            &NamedNode::new("http://example.com/employer").unwrap(),
        )
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

fn with_graphs(store: &Store) {
    load(
        store,
        "GRAPH ex:o1 { ex:Dog rdfs:subClassOf ex:Animal } \
         GRAPH ex:o2 { ex:Dog rdfs:subClassOf ex:Plant } \
         GRAPH ex:base { ex:Animal rdfs:subClassOf ex:Being . ex:Plant rdfs:subClassOf ex:Being } \
         GRAPH ex:a { ex:rex a ex:Dog } \
         GRAPH ex:b { ex:fido a ex:Dog }",
    );
}

// @lat: [[tests#Schema registry#Ontologies apply to the graphs they are mapped to]]
#[test]
fn ontologies_apply_to_mapped_graphs() {
    let store = store();
    with_graphs(&store);
    let register = |o: &str, to: &[&str]| {
        store
            .register_schema_graph(
                graph(o).as_ref(),
                SchemaRole::Ontology,
                &Registration::new().applies_to(to.iter().map(|g| graph(g))),
            )
            .unwrap();
    };
    register("http://example.com/o1", &["http://example.com/a"]);
    register("http://example.com/o2", &["http://example.com/b"]);
    let animals = "SELECT ?x WHERE { ?x a ex:Animal }";
    let plants = "SELECT ?x WHERE { ?x a ex:Plant }";
    assert_eq!(
        values(&store, animals, &rdfs()),
        ["<http://example.com/rex>"]
    );
    assert_eq!(
        values(&store, plants, &rdfs()),
        ["<http://example.com/fido>"]
    );
    // The same through GRAPH ?g, and through the fallback evaluator's quad source.
    assert_eq!(
        values(
            &store,
            "SELECT ?x WHERE { GRAPH ?g { ?x a ex:Plant } }",
            &rdfs()
        ),
        ["<http://example.com/fido>"]
    );

    // A global ontology applies everywhere, on top of the mapped ones.
    register("http://example.com/base", &[]);
    assert_eq!(
        values(&store, "SELECT ?x WHERE { ?x a ex:Being }", &rdfs()),
        ["<http://example.com/fido>", "<http://example.com/rex>"]
    );
    // Remapping o1 to every graph makes rex and fido animals.
    register("http://example.com/o1", &[]);
    assert_eq!(
        values(&store, animals, &rdfs()),
        ["<http://example.com/fido>", "<http://example.com/rex>"]
    );
    assert_eq!(
        store
            .schema_graphs_for(graph("http://example.com/b").as_ref(), SchemaRole::Ontology)
            .unwrap(),
        [
            graph("http://example.com/base"),
            graph("http://example.com/o1"),
            graph("http://example.com/o2")
        ]
    );
    assert_eq!(
        store
            .schema_graphs_for(graph("http://example.com/a").as_ref(), SchemaRole::Ontology)
            .unwrap(),
        [
            graph("http://example.com/base"),
            graph("http://example.com/o1")
        ]
    );
}

// @lat: [[tests#Schema registry#The registry is RDF in the schema graph]]
#[test]
fn registry_is_rdf_in_the_schema_graph() {
    let store = store();
    with_graphs(&store);
    // Registered by a plain SPARQL update, as on any SPARQL store.
    store
        .update(
            "PREFIX oxl: <https://oxilite.dev/ns#> INSERT DATA { GRAPH <oxilite:schema> { \
             <http://example.com/o2> a oxl:OntologyGraph ; oxl:appliesTo <http://example.com/b> ; oxl:version \"7\" } }",
        )
        .unwrap();
    let listed = store.schema_graphs().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].registration.version.as_deref(), Some("7"));
    assert_eq!(
        listed[0].registration.applies_to,
        [graph("http://example.com/b")]
    );
    assert!(listed[0].registration.active);
    assert_eq!(
        values(&store, "SELECT ?x WHERE { ?x a ex:Plant }", &rdfs()),
        ["<http://example.com/fido>"],
        "a SPARQL registration scopes reasoning like the API"
    );
    // What the API writes reads back with SPARQL.
    store
        .register_schema_graph(
            graph("http://example.com/o1").as_ref(),
            SchemaRole::Ontology,
            &Registration::new()
                .with_version("v1")
                .applies_to([GraphName::DefaultGraph]),
        )
        .unwrap();
    let ask = |q: &str| {
        matches!(
            store
                .query_output(
                    format!("PREFIX oxl: <https://oxilite.dev/ns#> ASK {{ GRAPH <oxilite:schema> {{ {q} }} }}").as_str(),
                    &QueryOptions::default()
                )
                .unwrap(),
            QueryOutput::Boolean(true)
        )
    };
    assert!(ask("<http://example.com/o1> a oxl:OntologyGraph ; oxl:version \"v1\" ; oxl:active true ; oxl:appliesTo oxl:DefaultGraph ; oxl:loadedAt ?t"));
    // Deactivating is a registry change too.
    assert!(store
        .set_schema_graph_active(graph("http://example.com/o2").as_ref(), false)
        .unwrap());
    assert!(ask("<http://example.com/o2> oxl:active false"));
    assert!(values(&store, "SELECT ?x WHERE { ?x a ex:Plant }", &rdfs()).is_empty());
    // The registry graph is hidden with the schema graphs.
    let hidden = QueryOptions {
        include_schema_graphs: false,
        ..QueryOptions::default()
    };
    assert!(
        values(&store, "SELECT ?g WHERE { GRAPH ?g { ?s ?p ?o } }", &hidden)
            .iter()
            .all(|g| g != "<oxilite:schema>" && g != "<http://example.com/o1>")
    );
    assert!(oxilite::schema::VOCABULARY.contains("oxl:appliesTo"));
}

// @lat: [[tests#Schema registry#A version 1 registry is migrated]]
#[test]
fn version_one_registry_is_migrated() {
    use oxilite_core::sql::Request;
    let path = std::env::temp_dir().join(format!("oxilite-registry-v1-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    {
        let store = Store::open(&path).unwrap();
        load(
            &store,
            "GRAPH ex:onto { ex:Dog rdfs:subClassOf ex:Animal } \
             GRAPH ex:other { ex:Rock rdfs:subClassOf ex:Animal } ex:rex a ex:Dog . ex:granite a ex:Rock .",
        );
    }
    // Turn the file into a schema version 1 store holding one registration.
    let onto = oxilite_core::encoding::named_node_id("http://example.com/onto");
    let backend = oxilite::rusqlite::RusqliteBackend::open(&path).unwrap();
    backend
        .execute(&Request::atomic(vec![
            "DROP TABLE tbox_closure".into(),
            "CREATE TABLE tbox_closure (kind INTEGER NOT NULL, sub INTEGER NOT NULL, sup INTEGER NOT NULL, PRIMARY KEY (kind, sup, sub)) WITHOUT ROWID, STRICT".into(),
            "CREATE INDEX tbox_closure_sub ON tbox_closure(kind, sub, sup)".into(),
            "CREATE TABLE schema_graphs (g INTEGER PRIMARY KEY, role INTEGER NOT NULL, iri TEXT, version TEXT, sha256 TEXT, imports TEXT, active INTEGER NOT NULL DEFAULT 1, loaded_at REAL NOT NULL) STRICT".into(),
            format!("INSERT INTO schema_graphs VALUES ({onto}, 1, NULL, 'v1', NULL, 'http://example.com/base', 1, 0)").into(),
            "UPDATE oxilite_meta SET value = '1' WHERE key = 'schema_version'".into(),
        ]))
        .unwrap();
    drop(backend);

    let store = Store::open(&path).unwrap();
    let listed = store.schema_graphs().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].graph, graph("http://example.com/onto"));
    assert_eq!(listed[0].role, SchemaRole::Ontology);
    assert_eq!(listed[0].registration.version.as_deref(), Some("v1"));
    assert_eq!(
        listed[0].registration.imports,
        [NamedNode::new("http://example.com/base").unwrap()]
    );
    assert_eq!(
        values(&store, "SELECT ?x WHERE { ?x a ex:Animal }", &rdfs()),
        ["<http://example.com/rex>"],
        "the migrated registration scopes reasoning"
    );
    drop(store);
    let backend = oxilite::rusqlite::RusqliteBackend::open(&path).unwrap();
    let r = backend
        .execute(&Request::read(vec![
            "SELECT count(*) FROM sqlite_master WHERE name = 'schema_graphs'".into(),
            "SELECT value FROM oxilite_meta WHERE key = 'schema_version'".into(),
        ]))
        .unwrap();
    assert_eq!(r[0].rows[0][0].as_i64(), Some(0));
    assert_eq!(r[1].rows[0][0].as_str(), Some("2"));
    drop(backend);
    let _ = std::fs::remove_file(&path);
}

// @lat: [[tests#Schema registry#A blank store starts with the system graphs]]
#[test]
fn blank_store_starts_with_system_graphs() {
    let options = oxilite::StoreOptions {
        system_graphs: true,
        ..Default::default()
    };
    let store = Store::with_backend_and_options(
        oxilite::rusqlite::RusqliteBackend::memory().unwrap(),
        &options,
    )
    .unwrap();
    let all = QueryOptions {
        union_default_graph: true,
        ..QueryOptions::default()
    };
    let graphs = values(
        &store,
        "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } }",
        &all,
    );
    assert_eq!(graphs, ["<oxilite:schema>", "<oxilite:vocabulary>"]);
    assert!(store.system_graphs_installed().unwrap());
    assert!(!store.install_system_graphs().unwrap(), "already current");
    // The vocabulary is there, typed and documented.
    assert_eq!(
        values(
            &store,
            "SELECT ?c WHERE { GRAPH <oxilite:vocabulary> { ?c rdfs:subClassOf <https://oxilite.dev/ns#SchemaGraph> } }",
            &all
        )
        .len(),
        4
    );
    // System graphs are not registrations, do not narrow reasoning, and hide with the schema.
    assert!(store.schema_graphs().unwrap().is_empty());
    load(
        &store,
        "GRAPH ex:onto { ex:Dog rdfs:subClassOf ex:Animal } ex:rex a ex:Dog .",
    );
    assert_eq!(
        values(&store, "SELECT ?x WHERE { ?x a ex:Animal }", &rdfs()),
        ["<http://example.com/rex>"]
    );
    let hidden = QueryOptions {
        include_schema_graphs: false,
        ..all.clone()
    };
    assert_eq!(
        values(
            &store,
            "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } }",
            &hidden
        ),
        ["<http://example.com/onto>"]
    );
    // An existing store gets them on request; a store that has data is never bootstrapped.
    let plain = Store::new().unwrap();
    load(&plain, "ex:a ex:b ex:c .");
    assert!(!plain.system_graphs_installed().unwrap());
    assert!(plain.install_system_graphs().unwrap());
    assert!(plain.system_graphs_installed().unwrap());
    assert!(plain.schema_graphs().unwrap().is_empty());
}
