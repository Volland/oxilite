//! Datalog dialect: parsing, compilation and evaluation against a real store.

use oxilite::store::Store;
use oxilite_datalog::{DatalogResult, Options};

const DATA: &str = r#"
PREFIX ex: <http://example.org/>
INSERT DATA {
  ex:ada  ex:parent ex:bob .
  ex:bob  ex:parent ex:cy .
  ex:cy   ex:parent ex:dee .
  ex:ada  a ex:Person . ex:bob a ex:Person .
  ex:cy   a ex:Person . ex:dee a ex:Person .
  ex:ada  ex:age 42 . ex:bob ex:age 17 .
  ex:cy   ex:age 71 . ex:dee ex:age 8 .
  ex:ada  ex:name "Ada" . ex:bob ex:name "Bob" .
}
"#;

fn store() -> Store {
    let store = Store::new().expect("in-memory store");
    store.update(DATA).expect("load");
    store
}

/// Renders solutions as sorted `a|b` strings, for order-independent comparison.
fn rows(r: &DatalogResult) -> Vec<String> {
    let mut out: Vec<String> = r
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| match c {
                    Some(t) => t.to_string(),
                    None => "UNBOUND".to_owned(),
                })
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    out.sort();
    out
}

const PREFIX: &str = "@prefix ex: <http://example.org/> .\n";

// @lat: [[tests#Datalog#Derived relation from a triple pattern]]
#[test]
fn derived_relation_from_a_triple_pattern() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} parent(?x, ?y) :- ex:parent(?x, ?y).\n?- parent(?x, ?y)."
        ))
        .unwrap();
    assert_eq!(r.variables, vec!["x".to_owned(), "y".to_owned()]);
    assert_eq!(
        rows(&r),
        vec![
            "<http://example.org/ada>|<http://example.org/bob>",
            "<http://example.org/bob>|<http://example.org/cy>",
            "<http://example.org/cy>|<http://example.org/dee>",
        ]
    );
}

// @lat: [[tests#Datalog#Join of two atoms]]
#[test]
fn join_of_two_atoms() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} gp(?x, ?z) :- ex:parent(?x, ?y), ex:parent(?y, ?z).\n?- gp(?x, ?z)."
        ))
        .unwrap();
    assert_eq!(
        rows(&r),
        vec![
            "<http://example.org/ada>|<http://example.org/cy>",
            "<http://example.org/bob>|<http://example.org/dee>",
        ]
    );
}

// @lat: [[tests#Datalog#Transitive closure agrees with a property path]]
#[test]
fn linear_recursion_matches_a_property_path() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} anc(?x, ?y) :- ex:parent(?x, ?y).\n\
             anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).\n?- anc(?x, ?y)."
        ))
        .unwrap();
    let datalog = rows(&r);

    // The same question in SPARQL, which is the oracle.
    let mut sparql: Vec<String> = match s
        .query("PREFIX ex: <http://example.org/> SELECT ?x ?y WHERE { ?x ex:parent+ ?y }")
        .unwrap()
    {
        oxilite::sparql::QueryResults::Solutions(sol) => sol
            .map(|s| {
                let s = s.unwrap();
                format!("{}|{}", s.get("x").unwrap(), s.get("y").unwrap())
            })
            .collect(),
        _ => panic!("expected solutions"),
    };
    sparql.sort();
    assert_eq!(datalog, sparql, "datalog recursion must equal ex:parent+");
    assert_eq!(datalog.len(), 6);
}

// @lat: [[tests#Datalog#Numeric comparison]]
#[test]
fn numeric_constraint() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} adult(?p) :- ex:age(?p, ?a), ?a >= 18.\n?- adult(?p)."
        ))
        .unwrap();
    assert_eq!(
        rows(&r),
        vec!["<http://example.org/ada>", "<http://example.org/cy>"]
    );
}

// @lat: [[tests#Datalog#Arithmetic in a constraint]]
#[test]
fn arithmetic_in_a_constraint() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} young(?p) :- ex:age(?p, ?a), ?a * 2 < 40.\n?- young(?p)."
        ))
        .unwrap();
    assert_eq!(
        rows(&r),
        vec!["<http://example.org/bob>", "<http://example.org/dee>"]
    );
}

// @lat: [[tests#Datalog#Negation over a derived relation]]
#[test]
fn stratified_negation() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} anc(?x, ?y) :- ex:parent(?x, ?y).\n\
             anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).\n\
             root(?x) :- ex:Person(?x), not anc(_, ?x).\n?- root(?x)."
        ))
        .unwrap();
    // Only Ada has no ancestor above her.
    assert_eq!(rows(&r), vec!["<http://example.org/ada>"]);
}

// @lat: [[tests#Datalog#Negation compiles to NOT EXISTS]]
#[test]
fn negation_compiles_to_not_exists() {
    let s = store();
    let sql = s
        .datalog_sql(&format!(
            "{PREFIX} childless(?x) :- ex:Person(?x), not ex:parent(?x, _).\n?- childless(?x)."
        ))
        .unwrap();
    assert!(sql.contains("NOT EXISTS"), "expected NOT EXISTS in:\n{sql}");
}

// @lat: [[tests#Datalog#Count grouped by key]]
#[test]
fn aggregation_counts_per_group() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} anc(?x, ?y) :- ex:parent(?x, ?y).\n\
             anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).\n\
             n(?x, COUNT(?y)) :- anc(?x, ?y).\n?- n(?x, ?c)."
        ))
        .unwrap();
    assert_eq!(
        rows(&r),
        [
            "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>".to_owned(),
            "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>".to_owned(),
            "\"3\"^^<http://www.w3.org/2001/XMLSchema#integer>".to_owned()
        ]
        .iter()
        .zip(["cy", "bob", "ada"])
        .map(|(c, p)| format!("<http://example.org/{p}>|{c}"))
        .collect::<Vec<_>>()
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
}

// @lat: [[tests#Datalog#Unstratified negation is rejected]]
#[test]
fn unstratified_negation_is_rejected() {
    let s = store();
    let err = s
        .datalog(&format!(
            "{PREFIX} p(?x) :- ex:Person(?x), not p(?x).\n?- p(?x)."
        ))
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("not stratified"), "got: {msg}");
    assert!(msg.contains('p'), "the cycle must be named: {msg}");
}

// @lat: [[tests#Datalog#Unbound head variable]]
#[test]
fn unsafe_head_variable_is_rejected() {
    let s = store();
    let err = s
        .datalog(&format!(
            "{PREFIX} p(?x, ?y) :- ex:Person(?x).\n?- p(?x, ?y)."
        ))
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("?y"), "got: {msg}");
}

// @lat: [[tests#Datalog#Unbound negated variable]]
#[test]
fn unsafe_negated_variable_is_rejected() {
    let s = store();
    let err = s
        .datalog(&format!(
            "{PREFIX} p(?x) :- ex:Person(?x), not ex:parent(?y, ?y).\n?- p(?x)."
        ))
        .unwrap_err();
    assert!(err.to_string().contains("?y"), "got: {err}");
}

// @lat: [[tests#Datalog#Quantifying over the predicate]]
#[test]
fn triple_atom_binds_the_predicate() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} uses(?p) :- triple(ex:ada, ?p, _).\n?- uses(?p)."
        ))
        .unwrap();
    let got = rows(&r);
    assert!(
        got.contains(&"<http://example.org/parent>".to_owned()),
        "{got:?}"
    );
    assert!(
        got.contains(&"<http://example.org/age>".to_owned()),
        "{got:?}"
    );
}

// @lat: [[tests#Datalog#Cycle terminates]]
#[test]
fn recursion_over_a_cycle_terminates() {
    let s = Store::new().unwrap();
    s.update(
        "PREFIX ex: <http://example.org/>
         INSERT DATA { ex:a ex:next ex:b . ex:b ex:next ex:c . ex:c ex:next ex:a }",
    )
    .unwrap();
    let r = s
        .datalog(&format!(
            "{PREFIX} reach(?x, ?y) :- ex:next(?x, ?y).\n\
             reach(?x, ?z) :- ex:next(?x, ?y), reach(?y, ?z).\n?- reach(?x, ?y)."
        ))
        .unwrap();
    // Every node reaches every node, including itself: 3 x 3.
    assert_eq!(r.rows.len(), 9);
}

// @lat: [[tests#Datalog#Goal with a constant]]
#[test]
fn goal_with_a_constant_projects_one_column() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} anc(?x, ?y) :- ex:parent(?x, ?y).\n\
             anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).\n?- anc(ex:ada, ?who)."
        ))
        .unwrap();
    assert_eq!(r.variables, vec!["who".to_owned()]);
    assert_eq!(
        rows(&r),
        vec![
            "<http://example.org/bob>",
            "<http://example.org/cy>",
            "<http://example.org/dee>",
        ]
    );
}

// @lat: [[tests#Datalog#Request count]]
#[test]
fn a_non_recursive_program_is_one_statement() {
    let s = store();
    let sql = s
        .datalog_sql(&format!(
            "{PREFIX} gp(?x, ?z) :- ex:parent(?x, ?y), ex:parent(?y, ?z).\n?- gp(?x, ?z)."
        ))
        .unwrap();
    assert!(sql.matches("SELECT").count() >= 1);
    assert!(!sql.contains(';'), "one statement only:\n{sql}");
}

// @lat: [[tests#Datalog#Options scope graphs]]
#[test]
fn union_default_graph_option_widens_the_scope() {
    let s = Store::new().unwrap();
    s.update(
        "PREFIX ex: <http://example.org/>
         INSERT DATA { GRAPH ex:g { ex:a ex:p ex:b } }",
    )
    .unwrap();
    let program = format!("{PREFIX} r(?x, ?y) :- ex:p(?x, ?y).\n?- r(?x, ?y).");
    assert!(s.datalog(&program).unwrap().rows.is_empty());
    let opts = Options {
        union_default_graph: true,
        ..Options::default()
    };
    assert_eq!(s.datalog_with(&program, &opts).unwrap().rows.len(), 1);
}

// @lat: [[tests#Datalog#A unary atom is a class]]
#[test]
fn a_unary_atom_is_a_class() {
    let s = store();
    let r = s
        .datalog(&format!("{PREFIX} p(?x) :- ex:Person(?x).\n?- p(?x)."))
        .unwrap();
    assert_eq!(r.rows.len(), 4, "every ex:Person, via rdf:type");
}

// @lat: [[tests#Datalog#An aggregate with no inline form is rejected]]
#[test]
fn an_aggregate_with_no_inline_form_is_rejected() {
    let s = store();
    for func in ["AVG", "GROUP_CONCAT"] {
        let err = s
            .datalog(&format!(
                "{PREFIX} m(?x, {func}(?a)) :- ex:age(?x, ?a).\n?- m(?x, ?v)."
            ))
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("inline term id"), "{func}: {msg}");
    }
}

// @lat: [[tests#Datalog#Sum and min over a group]]
#[test]
fn sum_and_min_aggregate_to_terms() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} total(SUM(?a)) :- ex:age(_, ?a).\n?- total(?t)."
        ))
        .unwrap();
    assert_eq!(
        r.rows[0][0].as_ref().unwrap().to_string(),
        "\"138\"^^<http://www.w3.org/2001/XMLSchema#integer>",
        "42 + 17 + 71 + 8"
    );
}

// @lat: [[tests#Datalog#String constraint]]
#[test]
fn string_constraints_work_on_lexical_forms() {
    let s = store();
    let r = s
        .datalog(&format!(
            "{PREFIX} a(?p) :- ex:name(?p, ?n), STRSTARTS(?n, \"A\").\n?- a(?p)."
        ))
        .unwrap();
    assert_eq!(rows(&r), vec!["<http://example.org/ada>"]);
}

/// The program the README and the website print, run end to end, so the documentation cannot
/// drift away from what the dialect actually accepts.
// @lat: [[tests#Datalog#The documented program runs]]
#[test]
fn the_documented_program_runs() {
    let s = store();
    let r = s
        .datalog(
            r#"
  @prefix ex: <http://example.org/> .

  ancestor(?x, ?y) :- ex:parent(?x, ?y).
  ancestor(?x, ?z) :- ex:parent(?x, ?y), ancestor(?y, ?z).

  adult(?x, ?y)    :- ancestor(?x, ?y), ex:age(?y, ?a), ?a >= 18.
  orphan(?x)       :- ex:Person(?x), not ancestor(_, ?x).
  lines(?x, COUNT(?y)) :- ancestor(?x, ?y).

  ?- adult(?x, ?y).
"#,
        )
        .unwrap();
    // Of ada's descendants, cy (71) is an adult; bob (17) and dee (8) are not.
    assert_eq!(
        rows(&r),
        [
            "<http://example.org/ada>|<http://example.org/cy>",
            "<http://example.org/bob>|<http://example.org/cy>"
        ]
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    );
}
