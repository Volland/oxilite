//! Synalog over the triple store: the store as tables, scope options, recursion, negation,
//! and what the store refuses to run.

use oxilite::core::encoding::named_node_id;
use oxilite::sparql::QueryResults;
use oxilite::store::Store;
use oxilite::version::Versioning;
use oxilite::{Capabilities, StoreOptions};
use oxilite_synalog::{
    check, compile, compile_for_engine, Options, SqlValue, SynalogError, SynalogResult,
};

const EX: &str = "http://example.org/";

const DATA: &str = r#"
PREFIX ex: <http://example.org/>
INSERT DATA {
  ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy . ex:cy ex:parent ex:dee .
  ex:ada a ex:Person . ex:bob a ex:Person . ex:cy a ex:Person . ex:dee a ex:Person .
  ex:ada ex:age 42 . ex:bob ex:age 17 . ex:cy ex:age 71 . ex:dee ex:age 8 .
  ex:ada ex:name "Ada" . ex:bob ex:name "Bob"@en . ex:cy ex:knows _:someone .
  GRAPH ex:g { ex:eve ex:parent ex:ada }
}
"#;

const PARENT: &str = "# @table parent <http://example.org/parent>\n";

fn store() -> Store {
    let store = Store::new().expect("in-memory store");
    store.update(DATA).expect("load");
    store
}

fn text(v: &SqlValue) -> String {
    match v {
        SqlValue::Null => "NULL".into(),
        SqlValue::Integer(i) => i.to_string(),
        SqlValue::Real(r) => r.to_string(),
        SqlValue::Text(s) => s.strip_prefix(EX).unwrap_or(s).to_owned(),
    }
}

/// Rows as sorted `a|b` strings, with the example namespace dropped.
fn rows(r: &SynalogResult) -> Vec<String> {
    let mut out: Vec<String> = r
        .rows
        .iter()
        .map(|row| row.iter().map(text).collect::<Vec<_>>().join("|"))
        .collect();
    out.sort();
    out
}

// @lat: [[tests#Synalog#Rule over triples]]
#[test]
fn rule_over_triples() {
    let r = store()
        .synalog(
            r#"Parent(child:, parent:) :-
                 triples(subject: child, predicate: "http://example.org/parent", object: parent);"#,
            "Parent",
        )
        .unwrap();
    assert_eq!(r.columns, ["child", "parent"]);
    assert_eq!(rows(&r), ["ada|bob", "bob|cy", "cy|dee"]);
}

// @lat: [[tests#Synalog#Numbers compare as numbers]]
#[test]
fn numbers_compare_as_numbers() {
    let s = store();
    let program = r#"
        Age(p:, age:) :- triples(subject: p, predicate: "http://example.org/age", object: age);
        Adult(p:) :- Age(p:, age:), age > 18;
        Total(t? += age) distinct :- Age(age:);
    "#;
    assert_eq!(rows(&s.synalog(program, "Adult").unwrap()), ["ada", "cy"]);
    let total = s.synalog(program, "Total").unwrap();
    assert_eq!(total.rows, [[SqlValue::Integer(138)]]);
}

// @lat: [[tests#Synalog#Literal details are kept]]
#[test]
fn literal_details_are_kept() {
    let r = store()
        .synalog(
            r#"Detail(s:, o:, kind:, datatype:, lang:) :-
                 triples(subject: s, object: o, kind:, datatype:, lang:),
                 s in ["http://example.org/bob", "http://example.org/cy"];"#,
            "Detail",
        )
        .unwrap();
    let rs = rows(&r);
    assert!(
        rs.contains(
            &"bob|Bob|literal|http://www.w3.org/1999/02/22-rdf-syntax-ns#langString|en".to_owned()
        ),
        "{rs:?}"
    );
    assert!(
        rs.contains(&"bob|17|literal|http://www.w3.org/2001/XMLSchema#integer|NULL".to_owned()),
        "{rs:?}"
    );
    let blank = r
        .rows
        .iter()
        .find(|row| row[2] == SqlValue::Text("blank".into()))
        .expect("the blank node row");
    assert!(
        matches!(&blank[1], SqlValue::Text(l) if l.starts_with("_:")),
        "{blank:?}"
    );
    assert_eq!(blank[3], SqlValue::Null);
}

// @lat: [[tests#Synalog#Predicate table selects by term id]]
#[test]
fn predicate_table_selects_by_term_id() {
    let s = store();
    let program =
        format!("{PARENT}Parent(child:, parent:) :- parent(subject: child, object: parent);");
    assert_eq!(
        rows(&s.synalog(&program, "Parent").unwrap()),
        ["ada|bob", "bob|cy", "cy|dee"]
    );
    let sql = s.synalog_sql(&program, "Parent").unwrap();
    let id = named_node_id(&format!("{EX}parent"));
    assert!(sql.contains(&format!("q.p = {id}")), "{sql}");
    assert!(!sql.contains("triples AS NOT MATERIALIZED"), "{sql}");
}

// @lat: [[tests#Synalog#Class table]]
#[test]
fn class_table() {
    let r = store()
        .synalog(
            "# @class person <http://example.org/Person>\nPerson(p:) :- person(subject: p);",
            "Person",
        )
        .unwrap();
    assert_eq!(rows(&r), ["ada", "bob", "cy", "dee"]);
}

// @lat: [[tests#Synalog#Named graphs need the union option]]
#[test]
fn named_graphs_need_the_union_option() {
    let s = store();
    let program = format!("{PARENT}P(child:, graph:) :- parent(subject: child, graph:);");
    assert_eq!(s.synalog(&program, "P").unwrap().rows.len(), 3);
    let all = Options {
        union_default_graph: true,
        ..Default::default()
    };
    let r = s.synalog_with(&program, "P", &all).unwrap();
    assert_eq!(rows(&r), ["ada|NULL", "bob|NULL", "cy|NULL", "eve|g"]);
}

// @lat: [[tests#Synalog#Inferences are opt-in]]
#[test]
fn inferences_are_opt_in() {
    let s = store();
    s.datalog_materialize(
        "@prefix ex: <http://example.org/> .
         ex:grandparent(?x, ?z) :- ex:parent(?x, ?y), ex:parent(?y, ?z).",
    )
    .unwrap();
    let program = "# @table grandparent <http://example.org/grandparent>\n\
                   G(x:, y:) :- grandparent(subject: x, object: y);";
    assert!(s.synalog(program, "G").unwrap().rows.is_empty());
    let inferred = Options {
        include_inferred: true,
        ..Default::default()
    };
    assert_eq!(
        rows(&s.synalog_with(program, "G", &inferred).unwrap()),
        ["ada|cy", "bob|dee"]
    );
}

// @lat: [[tests#Synalog#Inferences merge with asserted triples]]
#[test]
fn an_inference_that_is_also_asserted_reads_once() {
    let s = store();
    // Re-derives every asserted `ex:parent` triple of the default graph.
    s.datalog_materialize(
        "@prefix ex: <http://example.org/> .
         ex:parent(?x, ?y) :- triple(?x, ex:parent, ?y).",
    )
    .unwrap();
    let inferred = Options {
        include_inferred: true,
        ..Default::default()
    };
    for program in [
        format!("{PARENT}P(x:, y:) :- parent(subject: x, object: y);"),
        "P(x:, y:) :- triples(subject: x, predicate: \"http://example.org/parent\", object: y);"
            .to_owned(),
    ] {
        assert_eq!(
            rows(&s.synalog_with(&program, "P", &inferred).unwrap()),
            ["ada|bob", "bob|cy", "cy|dee"],
            "{program}"
        );
    }
}

// @lat: [[tests#Synalog#Time travel]]
#[test]
fn time_travel() {
    let s = Store::with_backend_and_options(
        oxilite::rusqlite::RusqliteBackend::memory().unwrap(),
        &StoreOptions {
            versioning: Versioning::Log,
            ..Default::default()
        },
    )
    .unwrap();
    s.update("PREFIX ex: <http://example.org/> INSERT DATA { ex:ada ex:parent ex:bob }")
        .unwrap();
    s.update("PREFIX ex: <http://example.org/> INSERT DATA { ex:bob ex:parent ex:cy }")
        .unwrap();
    let program = format!("{PARENT}P(x:, y:) :- parent(subject: x, object: y);");
    assert_eq!(s.synalog(&program, "P").unwrap().rows.len(), 2);
    let past = Options {
        as_of: Some("HEAD~1".into()),
        ..Default::default()
    };
    assert_eq!(
        rows(&s.synalog_with(&program, "P", &past).unwrap()),
        ["ada|bob"]
    );
    let both = Options {
        include_inferred: true,
        ..past
    };
    assert!(matches!(
        s.synalog_with(&program, "P", &both),
        Err(SynalogError::Unsupported(_))
    ));
}

// @lat: [[tests#Synalog#Recursion agrees with a property path]]
#[test]
fn recursion_agrees_with_a_property_path() {
    let s = store();
    let program = format!(
        "{PARENT}@Recursive(Ancestor, 10);
         Ancestor(x:, y:) distinct :- parent(subject: x, object: y);
         Ancestor(x:, y:) distinct :- Ancestor(x:, y: m), parent(subject: m, object: y);"
    );
    let got = rows(&s.synalog(&program, "Ancestor").unwrap());
    let QueryResults::Solutions(solutions) = s
        .query("PREFIX ex: <http://example.org/> SELECT ?x ?y WHERE { ?x ex:parent+ ?y }")
        .unwrap()
    else {
        panic!("expected solutions");
    };
    let mut want: Vec<String> = solutions
        .map(|sol| {
            let sol = sol.unwrap();
            let name = |v: &str| match sol.get(v) {
                Some(oxilite::model::Term::NamedNode(n)) => n.as_str()[EX.len()..].to_owned(),
                other => panic!("unexpected {other:?}"),
            };
            format!("{}|{}", name("x"), name("y"))
        })
        .collect();
    want.sort();
    assert_eq!(got, want);
    assert_eq!(got.len(), 6);
}

// @lat: [[tests#Synalog#Negation needs no runtime function]]
#[test]
fn negation_needs_no_runtime_function() {
    let s = store();
    let program = format!(
        "{PARENT}# @class person <http://example.org/Person>
         Childless(p:) distinct :- person(subject: p), ~parent(subject: p);"
    );
    assert_eq!(rows(&s.synalog(&program, "Childless").unwrap()), ["dee"]);
    let sql = s.synalog_sql(&program, "Childless").unwrap();
    assert!(
        !sql.to_ascii_lowercase().contains("magicalentangle"),
        "{sql}"
    );
}

// @lat: [[tests#Synalog#Runtime-only functions are rejected]]
#[test]
fn runtime_only_functions_are_rejected() {
    let err = store()
        .synalog(
            r#"Oldest(p? ArgMax= p -> age) distinct :-
                 triples(subject: p, predicate: "http://example.org/age", object: age);"#,
            "Oldest",
        )
        .unwrap_err();
    assert!(
        matches!(&err, SynalogError::Unsupported(m) if m.contains("ArgMax")),
        "{err}"
    );
}

// @lat: [[tests#Synalog#Ground is rejected]]
#[test]
fn ground_is_rejected() {
    let err = store()
        .synalog(
            "@Ground(S);\nS(s:) distinct :- triples(subject: s);\nT(s:) :- S(s:);",
            "T",
        )
        .unwrap_err();
    assert!(
        matches!(&err, SynalogError::Unsupported(m) if m.contains("create tables")),
        "{err}"
    );
}

// @lat: [[tests#Synalog#Verifier errors are reported]]
#[test]
fn verifier_errors_are_reported() {
    let program = format!(
        "{PARENT}Anc(x:, y:) :- parent(subject: x, object: y);
         Anc(x:, y:) :- Anc(x:, y: m), parent(subject: m, object: y);"
    );
    let err = check(&program).unwrap_err();
    assert!(
        matches!(&err, SynalogError::Verify(m) if m.iter().any(|e| e.contains("Anc"))),
        "{err}"
    );
    assert!(matches!(
        store().synalog(&program, "Anc"),
        Err(SynalogError::Verify(_))
    ));
    assert!(matches!(
        store().synalog("P(x:) :- triples(subject: x);", "Q"),
        Err(SynalogError::UnknownPredicate { .. })
    ));
}

// @lat: [[tests#Synalog#Compile for another engine]]
#[test]
fn compile_for_another_engine() {
    let program = "@OrderBy(Big, \"n\");\nBig(n:) :- orders(amount: n), n > 100;";
    let duck = compile_for_engine(program, "Big", "duckdb", Some(5), None).unwrap();
    assert!(
        duck.contains("orders") && duck.contains("LIMIT 5"),
        "{duck}"
    );
    assert!(compile_for_engine(program, "Big", "oracle", None, None).is_err());
}

// @lat: [[tests#Synalog#Pagination]]
#[test]
fn pagination() {
    let program = r#"
        @OrderBy(Ages, "p");
        Ages(p:, age:) :- triples(subject: p, predicate: "http://example.org/age", object: age);
    "#;
    let page = Options {
        limit: Some(1),
        offset: Some(1),
        ..Default::default()
    };
    let r = store().synalog_with(program, "Ages", &page).unwrap();
    assert_eq!(rows(&r), ["bob|17"]);
}

// @lat: [[tests#Synalog#D1 limits are checked before sending]]
#[test]
fn d1_limits_are_checked_before_sending() {
    let program = |bound: usize| {
        format!(
            "{PARENT}@Recursive(Ancestor, {bound});
             Ancestor(x:, y:) distinct :- parent(subject: x, object: y);
             Ancestor(x:, y:) distinct :- Ancestor(x:, y: m), parent(subject: m, object: y);"
        )
    };
    let d1 = Capabilities::d1();
    let small = compile(&program(5), "Ancestor", &d1, &Options::default()).unwrap();
    assert!(small.sql.len() <= d1.max_sql_len);
    assert_eq!(small.tables, ["parent"]);
    // Above 20, Synalog iterates through tables it creates: not a read.
    let err = compile(&program(21), "Ancestor", &d1, &Options::default()).unwrap_err();
    assert!(
        matches!(&err, SynalogError::Unsupported(m) if m.contains("@Recursive")),
        "{err}"
    );
    // Six rules for one predicate are a six-term UNION ALL; D1 allows five.
    let six: String = (0..6)
        .map(|i| format!("P(x:) :- triples(subject: x, object: {i});\n"))
        .collect();
    let err = compile(&six, "P", &d1, &Options::default()).unwrap_err();
    assert!(
        matches!(&err, SynalogError::Unsupported(m) if m.contains("compound")),
        "{err}"
    );
    assert!(compile(&six, "P", &Capabilities::native(), &Options::default()).is_ok());
}
