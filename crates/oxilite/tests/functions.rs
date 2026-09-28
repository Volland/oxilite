//! Host functions: application code called from SPARQL, Cypher and Datalog.

use oxilite::functions::HostFunction;
use oxilite::model::{Literal, Term};
use oxilite::sparql::QueryResults;
use oxilite::store::Store;

const FN: &str = "http://example.com/fn#";
const PREFIXES: &str = "PREFIX ex: <http://example.com/> PREFIX fn: <http://example.com/fn#> ";

fn text(t: &Term) -> Option<&str> {
    match t {
        Term::Literal(l) => Some(l.value()),
        _ => None,
    }
}

fn slugify() -> HostFunction {
    HostFunction::new(format!("{FN}slugify"), |args| {
        let s = text(args.first()?)?;
        Some(
            Literal::new_simple_literal(
                s.to_lowercase()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join("-"),
            )
            .into(),
        )
    })
    .cypher_name("ex.slugify")
    .arity(1, 1)
    .description("URL-safe slug of a string")
}

fn is_long() -> HostFunction {
    HostFunction::new(format!("{FN}isLong"), |args| {
        Some(Literal::from(text(args.first()?)?.chars().count() > 4).into())
    })
    .arity(1, 1)
}

fn people<B: oxilite::SyncBackend + Send + Sync + 'static>(store: &Store<B>) {
    store.register_function(slugify()).unwrap();
    store.register_function(is_long()).unwrap();
    store
        .update(format!(
            r#"{PREFIXES} INSERT DATA {{
                ex:ada a ex:Person ; ex:name "Ada Lovelace" .
                ex:al a ex:Person ; ex:name "Al" .
                ex:grace a ex:Person ; ex:name "Grace Hopper" .
            }}"#
        ))
        .unwrap();
}

fn column<B: oxilite::SyncBackend + Send + Sync + 'static>(
    store: &Store<B>,
    q: &str,
    var: &str,
) -> Vec<Option<String>> {
    let QueryResults::Solutions(s) = store.query(format!("{PREFIXES}{q}").as_str()).unwrap() else {
        panic!("not a SELECT")
    };
    let mut out: Vec<Option<String>> = s
        .map(|r| r.unwrap().get(var).and_then(text).map(str::to_owned))
        .collect();
    out.sort();
    out
}

// @lat: [[tests#Host functions#Registry]]
#[test]
fn registry() {
    let store = Store::new().unwrap();
    people(&store);
    let fs = store.functions();
    assert_eq!(fs.len(), 2);
    assert!(fs.iter().any(|f| f.cypher() == "ex.slugify"
        && f.description_text() == "URL-safe slug of a string"
        && f.max_arity() == Some(1)));
    // A clone shares the registry.
    assert_eq!(store.clone().functions().len(), 2);
    for clash in [
        HostFunction::new("https://oxilite.dev/ns#textMatch", |_| None),
        HostFunction::new("http://www.w3.org/2001/XMLSchema#string", |_| None),
        HostFunction::new("http://example.com/x", |_| None).cypher_name("toUpper"),
    ] {
        assert!(store.register_function(clash).is_err());
    }
    assert!(store.unregister_function(&format!("{FN}isLong")));
    assert_eq!(store.functions().len(), 1);
}

// @lat: [[tests#Host functions#SPARQL calls host functions]]
#[test]
fn sparql_calls_host_functions() {
    let store = Store::new().unwrap();
    people(&store);
    assert_eq!(
        column(
            &store,
            "SELECT ?slug WHERE { ?s ex:name ?n BIND(fn:slugify(?n) AS ?slug) FILTER(fn:isLong(?n)) }",
            "slug"
        ),
        [Some("ada-lovelace".into()), Some("grace-hopper".into())]
    );
    // The rest of the query still runs as SQL.
    let explain = store
        .explain(format!("{PREFIXES} SELECT ?slug WHERE {{ ?s a ex:Person ; ex:name ?n BIND(fn:slugify(?n) AS ?slug) }}").as_str())
        .unwrap();
    assert!(explain.contains("SELECT"), "{explain}");
    // In an update's WHERE.
    store
        .update(format!(
            "{PREFIXES} INSERT {{ ?s ex:slug ?slug }} WHERE {{ ?s ex:name ?n BIND(fn:slugify(?n) AS ?slug) }}"
        ))
        .unwrap();
    assert_eq!(
        column(&store, "SELECT ?x WHERE { ex:al ex:slug ?x }", "x"),
        [Some("al".into())]
    );
    // An unregistered function fails the query, naming it.
    let err = store
        .query(
            format!("{PREFIXES} SELECT ?x WHERE {{ ex:al ex:name ?n BIND(fn:missing(?n) AS ?x) }}")
                .as_str(),
        )
        .err()
        .expect("unknown function");
    assert!(err.to_string().contains("fn#missing"), "{err}");
    // A wrong number of arguments is an evaluation error too.
    assert_eq!(
        column(
            &store,
            "SELECT ?x WHERE { ex:al ex:name ?n BIND(fn:slugify(?n, ?n) AS ?x) }",
            "x"
        ),
        [None]
    );
}

// @lat: [[tests#Host functions#Host functions on Turso with vector search]]
#[cfg(feature = "turso")]
#[test]
fn host_functions_with_vector_search() {
    let store = Store::new_turso().unwrap();
    people(&store);
    store
        .update(format!(
            r#"{PREFIXES} INSERT DATA {{ ex:ada ex:e "[1, 0]" . ex:al ex:e "[0, 1]" . ex:grace ex:e "[0.9, 0.2]" }}"#
        ))
        .unwrap();
    store
        .create_vector_index(&oxilite::vector::VectorIndex::new(
            "people",
            oxilite::model::NamedNode::new("http://example.com/e").unwrap(),
            2,
        ))
        .unwrap();
    assert_eq!(
        column(
            &store,
            "PREFIX oxl: <https://oxilite.dev/ns#> SELECT ?slug WHERE {
                SERVICE <oxilite:vector/people> { [] oxl:query \"[1, 0]\" ; oxl:k 2 ; oxl:node ?p }
                ?p ex:name ?n BIND(fn:slugify(?n) AS ?slug)
            }",
            "slug"
        ),
        [Some("ada-lovelace".into()), Some("grace-hopper".into())]
    );
}

// @lat: [[tests#Host functions#Cypher calls host functions]]
#[test]
fn cypher_calls_host_functions() {
    use oxilite::cypher::{CypherOptions, Value, Vocabulary};
    let store = Store::new().unwrap();
    people(&store);
    let options = CypherOptions {
        vocabulary: Vocabulary::new("http://example.com/"),
        ..Default::default()
    };
    let run = |q: &str| {
        store
            .cypher_with(q, &Default::default(), &options)
            .unwrap_or_else(|e| panic!("{q}: {e}"))
    };
    // In WHERE and RETURN (SQL part, host calls in the fallback).
    let r = run(
        "MATCH (p:Person) WHERE isLong(p.name) RETURN ex.slugify(p.name) AS slug ORDER BY slug",
    );
    assert_eq!(
        r.rows,
        [
            vec![Value::String("ada-lovelace".into())],
            vec![Value::String("grace-hopper".into())]
        ]
    );
    // In a clause evaluated in Rust.
    let r =
        run("MATCH (p:Person) WITH p ORDER BY p.name RETURN collect(ex.slugify(p.name)) AS slugs");
    assert_eq!(
        r.rows[0][0],
        Value::List(vec![
            Value::String("ada-lovelace".into()),
            Value::String("al".into()),
            Value::String("grace-hopper".into())
        ])
    );
    let r = run("RETURN EX.SLUGIFY('Hello World') AS s, ex.slugify(null) AS n");
    assert_eq!(
        r.rows[0],
        [Value::String("hello-world".into()), Value::Null]
    );
    let err = store
        .cypher_with("RETURN nosuch.fn('x')", &Default::default(), &options)
        .unwrap_err();
    assert!(err.to_string().contains("nosuch.fn"), "{err}");
}

fn datalog_column<B: oxilite::SyncBackend + Send + Sync + 'static>(
    store: &Store<B>,
    program: &str,
    var: &str,
) -> Vec<String> {
    let r = store
        .datalog(&format!(
            "@prefix ex: <http://example.com/> .\n@prefix fn: <http://example.com/fn#> .\n{program}"
        ))
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    let mut out: Vec<String> = (0..r.rows.len())
        .map(|i| match r.get(i, var) {
            Some(Term::Literal(l)) => l.value().to_owned(),
            Some(t) => t.to_string(),
            None => "unbound".into(),
        })
        .collect();
    out.sort();
    out
}

// @lat: [[tests#Host functions#Datalog calls host functions]]
#[test]
fn datalog_calls_host_functions() {
    let store = Store::new().unwrap();
    people(&store);
    // Expression form, binding a value.
    assert_eq!(
        datalog_column(
            &store,
            "slug(?p, ?s) :- ex:name(?p, ?n), ?s = fn:slugify(?n). ?- slug(?p, ?s).",
            "s"
        ),
        ["ada-lovelace", "al", "grace-hopper"]
    );
    // Atom forms: a filter, and a result argument.
    assert_eq!(
        datalog_column(
            &store,
            "long(?p, ?s) :- ex:name(?p, ?n), fn:isLong(?n), fn:slugify(?n, ?s). ?- long(?p, ?s).",
            "s"
        ),
        ["ada-lovelace", "grace-hopper"]
    );
    // Results feed later rules, joins and negation.
    let program = "slug(?p, ?s) :- ex:name(?p, ?n), ?s = fn:slugify(?n).
         short(?p) :- ex:Person(?p), not long(?p).
         long(?p) :- slug(?p, ?s), fn:isLong(?s).
         both(?p, ?s) :- short(?p), slug(?p, ?s).
         ?- both(?p, ?s).";
    assert_eq!(datalog_column(&store, program, "s"), ["al"]);
    // A goal constraint calling a host function filters the rows.
    assert_eq!(
        datalog_column(
            &store,
            "named(?p, ?n) :- ex:name(?p, ?n). ?- named(?p, ?n), fn:isLong(?n) = true.",
            "n"
        ),
        ["Ada Lovelace", "Grace Hopper"]
    );
    // A host rule that derives nothing still defines its relation.
    assert!(datalog_column(
        &store,
        "none(?p) :- ex:name(?p, ?n), fn:slugify(?n) = \"nobody\". ?- none(?p).",
        "p"
    )
    .is_empty());
}

// @lat: [[tests#Host functions#Datalog rejects recursive host rules]]
#[test]
fn datalog_rejects_recursive_host_rules() {
    let store = Store::new().unwrap();
    people(&store);
    let err = store
        .datalog(
            "@prefix ex: <http://example.com/> . @prefix fn: <http://example.com/fn#> .
             r(?x, ?y) :- ex:name(?x, ?y).
             r(?x, ?y) :- r(?x, ?z), ?y = fn:slugify(?z).
             ?- r(?x, ?y).",
        )
        .unwrap_err();
    assert!(err.to_string().contains("cannot be recursive"), "{err}");
    let err = store
        .datalog(
            "@prefix ex: <http://example.com/> . @prefix fn: <http://example.com/fn#> .
             r(?x, ?y) :- ex:name(?x, ?n), ?y = fn:unknown(?n). ?- r(?x, ?y).",
        )
        .unwrap_err();
    assert!(err.to_string().contains("fn#unknown"), "{err}");
}
