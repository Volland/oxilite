//! Optional full-text search (FTS5) and `oxl:textMatch`.

use oxilite::io::RdfFormat;
use oxilite::model::Term;
use oxilite::sparql::QueryOptions;
use oxilite::store::Store;
use oxilite_core::{QueryOutput, StoreOptions, SyncBackend};

const DATA: &str = r#"@prefix ex: <http://example.com/> .
ex:a ex:label "graph database" . ex:b ex:label "relational store" .
ex:c ex:label "Graphes et bases"@fr . ex:d ex:label "graphs everywhere" . ex:e ex:count 3 ."#;

const Q: &str = "PREFIX oxl: <https://oxilite.dev/ns#> SELECT ?s WHERE { ?s <http://example.com/label> ?l FILTER(oxl:textMatch(?l, \"QUERY\")) }";

fn subjects<B: SyncBackend + Send + Sync + 'static>(s: &Store<B>, text: &str) -> Vec<String> {
    let QueryOutput::Solutions { rows, .. } = s
        .query_output(Q.replace("QUERY", text).as_str(), &QueryOptions::default())
        .unwrap()
    else {
        panic!()
    };
    let mut v: Vec<String> = rows
        .into_iter()
        .map(|r| r[0].as_ref().map(Term::to_string).unwrap())
        .collect();
    v.sort();
    v
}

fn ex(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|n| format!("<http://example.com/{n}>"))
        .collect()
}

fn check<B: SyncBackend + Send + Sync + 'static>(s: Store<B>, indexed: bool) {
    s.load_from_slice(RdfFormat::Turtle, DATA.as_bytes())
        .unwrap();
    assert_eq!(subjects(&s, "graph"), ex(&["a"]));
    assert_eq!(subjects(&s, "GRAPH database"), ex(&["a"]));
    assert_eq!(subjects(&s, "graph*"), ex(&["a", "c", "d"]));
    assert_eq!(subjects(&s, "store"), ex(&["b"]));
    let plan = s.explain(Q.replace("QUERY", "graph").as_str()).unwrap();
    assert_eq!(plan.contains("terms_fts MATCH"), indexed, "{plan}");
    // The index follows the terms table through clear and re-insertion.
    s.clear().unwrap();
    assert!(subjects(&s, "graph").is_empty());
    s.load_from_slice(RdfFormat::Turtle, DATA.as_bytes())
        .unwrap();
    assert_eq!(subjects(&s, "graph"), ex(&["a"]));
}

fn dylib(options: &StoreOptions) -> Store<oxilite::dylib::DylibBackend> {
    let lib = std::env::var("OXILITE_SQLITE_LIBRARY").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/usr/lib/libsqlite3.dylib".into()
        } else {
            "libsqlite3.so.0".into()
        }
    });
    Store::with_backend_and_options(
        oxilite::dylib::DylibBackend::open(lib, ":memory:").unwrap(),
        options,
    )
    .unwrap()
}

fn with_text() -> StoreOptions {
    StoreOptions {
        text_index: true,
        ..StoreOptions::default()
    }
}

// @lat: [[tests#Text search#Word match]]
#[test]
fn word_match_with_the_index() {
    check(
        Store::with_backend_and_options(
            oxilite::rusqlite::RusqliteBackend::memory().unwrap(),
            &with_text(),
        )
        .unwrap(),
        true,
    );
    check(dylib(&with_text()), true);
}

// @lat: [[tests#Text search#Same answers without the index]]
#[test]
fn same_answers_without_the_index() {
    check(Store::new().unwrap(), false);
}

// @lat: [[tests#Text search#Disabled by default]]
#[test]
fn disabled_by_default() {
    let s = Store::new().unwrap();
    let QueryOutput::Solutions { rows, .. } = s
        .query_output("SELECT * WHERE { ?s ?p ?o }", &QueryOptions::default())
        .unwrap()
    else {
        panic!()
    };
    assert!(rows.is_empty());
    let sql = oxilite_core::schema::schema_sql(&StoreOptions::default());
    assert!(!sql.contains("terms_fts"));
    assert!(oxilite_core::schema::schema_sql(&with_text()).contains("USING fts5"));
}

// Enabling the index on an existing store back-fills it.
#[test]
fn enabling_back_fills() {
    let dir = std::env::temp_dir().join(format!("oxilite-text-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    {
        let s = Store::open(&dir).unwrap();
        s.load_from_slice(RdfFormat::Turtle, DATA.as_bytes())
            .unwrap();
    }
    let s = Store::open_with_options(&dir, with_text()).unwrap();
    assert_eq!(subjects(&s, "graph"), ex(&["a"]));
    assert!(s
        .explain(Q.replace("QUERY", "graph").as_str())
        .unwrap()
        .contains("terms_fts MATCH"));
    drop(s);
    let _ = std::fs::remove_file(&dir);
}
