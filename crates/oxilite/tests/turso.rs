//! The store on Turso: the query surface of the bundled SQLite backend, on the Rust engine.
#![cfg(feature = "turso")]

use oxilite::io::RdfFormat;
use oxilite::sparql::QueryResults;
use oxilite::store::Store;
use oxilite::StoreOptions;

const DATA: &str = r#"@prefix ex: <http://example.com/> .
ex:a a ex:Person ; ex:name "Alice"@en ; ex:age 31 ; ex:knows ex:b ; ex:score 1.5 .
ex:b a ex:Person ; ex:name "Bob" ; ex:age 25 ; ex:knows ex:c .
ex:c a ex:Person ; ex:name "Carol" ; ex:age 40 .
"#;

fn rows<B: oxilite::SyncBackend + Send + Sync + 'static>(store: &Store<B>, q: &str) -> Vec<String> {
    let QueryResults::Solutions(s) = store.query(q).unwrap() else {
        panic!("not a SELECT")
    };
    let mut out: Vec<String> = s
        .map(|r| {
            let r = r.unwrap();
            r.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    out.sort();
    out
}

const QUERIES: &[&str] = &[
    "PREFIX ex: <http://example.com/> SELECT ?n WHERE { ?p a ex:Person ; ex:name ?n ; ex:age ?a FILTER(?a > 30) }",
    "PREFIX ex: <http://example.com/> SELECT ?p ?s WHERE { ?p a ex:Person OPTIONAL { ?p ex:score ?s } }",
    "PREFIX ex: <http://example.com/> SELECT (COUNT(*) AS ?c) (AVG(?a) AS ?avg) WHERE { ?p ex:age ?a }",
    "PREFIX ex: <http://example.com/> SELECT ?p WHERE { { ?p ex:age 25 } UNION { ?p ex:age 40 } MINUS { ?p ex:name 'Carol' } }",
    "PREFIX ex: <http://example.com/> SELECT ?x ?y WHERE { ?x ex:knows+ ?y }",
    "PREFIX ex: <http://example.com/> SELECT ?n (UCASE(?n) AS ?u) WHERE { ?p ex:name ?n FILTER(REGEX(?n, '^a', 'i')) }",
];

// @lat: [[tests#Turso#Same answers as bundled SQLite]]
#[test]
fn same_answers_as_bundled_sqlite() {
    let turso = Store::new_turso().unwrap();
    let sqlite = Store::new().unwrap();
    sqlite
        .load_from_reader(RdfFormat::Turtle, DATA.as_bytes())
        .unwrap();
    turso
        .load_from_reader(RdfFormat::Turtle, DATA.as_bytes())
        .unwrap();
    for q in QUERIES {
        assert_eq!(rows(&turso, q), rows(&sqlite, q), "{q}");
    }
    assert_eq!(rows(&turso, QUERIES[4]).len(), 3);
    turso.optimize().unwrap();
    turso
        .update("PREFIX ex: <http://example.com/> DELETE { ?p ex:age ?a } INSERT { ?p ex:age 0 } WHERE { ?p ex:age ?a FILTER(?a < 30) }")
        .unwrap();
    assert_eq!(
        rows(
            &turso,
            "PREFIX ex: <http://example.com/> SELECT ?p WHERE { ?p ex:age 0 }"
        ),
        vec!["?p=<http://example.com/b>"]
    );
    turso
        .update("DROP SILENT GRAPH <http://example.com/g>")
        .unwrap();
}

// @lat: [[tests#Turso#Cypher and Datalog on Turso]]
#[test]
fn cypher_and_datalog_on_turso() {
    let store = Store::new_turso().unwrap();
    store
        .cypher("CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {name: 'Alan'})-[:KNOWS]->(:Person {name: 'Grace'})")
        .unwrap();
    let r = store
        .cypher("MATCH (a:Person {name: 'Ada'})-[:KNOWS]->(b) RETURN b.name")
        .unwrap();
    assert_eq!(r.rows[0][0], oxilite::cypher::Value::String("Alan".into()));
    store
        .update(
            "PREFIX ex: <http://example.org/> INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
        )
        .unwrap();
    let r = store
        .datalog(
            "@prefix ex: <http://example.org/> .
             anc(?x, ?y) :- ex:parent(?x, ?y).
             anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).
             ?- anc(?x, ?y).",
        )
        .unwrap();
    assert_eq!(r.rows.len(), 3);
}

// @lat: [[tests#Turso#File store survives reopening]]
#[test]
fn file_store_survives_reopening() {
    let dir = std::env::temp_dir().join(format!("oxilite-turso-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("kg.db");
    let _ = std::fs::remove_file(&path);
    {
        let store = Store::open_turso(&path).unwrap();
        store
            .load_from_reader(RdfFormat::Turtle, DATA.as_bytes())
            .unwrap();
    }
    let store = Store::open_turso(&path).unwrap();
    assert_eq!(store.len().unwrap(), 12);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

// @lat: [[tests#Turso#Schema and refusals]]
#[test]
fn schema_and_refusals() {
    let store = Store::new_turso().unwrap();
    assert!(store.backend().capabilities_vectors());
    store
        .update("INSERT DATA { <http://e/a> <http://e/p> 1 }")
        .unwrap();
    store
        .update("INSERT DATA { <http://e/a> <http://e/p> 1 }")
        .unwrap();
    assert_eq!(store.len().unwrap(), 1);
    let err = Store::with_backend_and_options(
        oxilite::turso::TursoBackend::memory().unwrap(),
        &StoreOptions {
            text_index: true,
            ..StoreOptions::default()
        },
    )
    .err()
    .expect("the text index is refused");
    assert!(err.to_string().contains("full-text index"), "{err}");
    assert!(!Store::new().unwrap().backend().capabilities_vectors());
}

trait VectorCaps {
    fn capabilities_vectors(&self) -> bool;
}

impl<B: oxilite::SyncBackend> VectorCaps for B {
    fn capabilities_vectors(&self) -> bool {
        let c = self.capabilities();
        c.vectors && c.vector_index_methods
    }
}
