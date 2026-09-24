//! Recursion strategies: mutual recursion, the non-linear diagnostic, and materialization.

use oxilite::store::Store;
use oxilite_datalog::Options;

const PREFIX: &str = "@prefix ex: <http://example.org/> .\n";

/// A successor chain 0 -> 1 -> 2 -> 3 -> 4, with zero marked.
fn chain() -> Store {
    let s = Store::new().unwrap();
    s.update(
        "PREFIX ex: <http://example.org/>
         INSERT DATA {
           ex:n0 a ex:Zero .
           ex:n0 ex:succ ex:n1 . ex:n1 ex:succ ex:n2 .
           ex:n2 ex:succ ex:n3 . ex:n3 ex:succ ex:n4 .
         }",
    )
    .unwrap();
    s
}

fn sorted(r: &oxilite_datalog::DatalogResult) -> Vec<String> {
    let mut v: Vec<String> = r
        .rows
        .iter()
        .map(|row| row[0].as_ref().unwrap().to_string())
        .collect();
    v.sort();
    v
}

const EVEN_ODD: &str = "\
even(?x) :- ex:Zero(?x).
even(?x) :- ex:succ(?y, ?x), odd(?y).
odd(?x)  :- ex:succ(?y, ?x), even(?y).
";

// @lat: [[tests#Datalog#Even and odd]]
#[test]
fn mutual_recursion_even() {
    let s = chain();
    let r = s
        .datalog(&format!("{PREFIX}{EVEN_ODD}?- even(?x)."))
        .unwrap();
    assert_eq!(
        sorted(&r),
        vec![
            "<http://example.org/n0>",
            "<http://example.org/n2>",
            "<http://example.org/n4>",
        ]
    );
}

// @lat: [[tests#Datalog#Even and odd]]
#[test]
fn mutual_recursion_odd() {
    let s = chain();
    let r = s.datalog(&format!("{PREFIX}{EVEN_ODD}?- odd(?x).")).unwrap();
    assert_eq!(
        sorted(&r),
        vec!["<http://example.org/n1>", "<http://example.org/n3>"]
    );
}

// @lat: [[tests#Datalog#Mutual recursion uses one tagged member]]
#[test]
fn mutual_recursion_is_one_tagged_member() {
    let s = chain();
    let sql = s
        .datalog_sql(&format!("{PREFIX}{EVEN_ODD}?- even(?x)."))
        .unwrap();
    assert!(sql.contains("WITH RECURSIVE"), "{sql}");
    assert!(sql.contains("tag"), "expected a discriminant column in:\n{sql}");
    let explain = s
        .explain_datalog(&format!("{PREFIX}{EVEN_ODD}?- even(?x)."))
        .unwrap();
    assert!(explain.contains("mutual recursion"), "{explain}");
}

const NON_LINEAR: &str = "\
path(?x, ?y) :- ex:succ(?x, ?y).
path(?x, ?z) :- path(?x, ?y), path(?y, ?z).
";

// @lat: [[tests#Datalog#Non-linear recursion agrees with the linear form]]
#[test]
fn non_linear_recursion_agrees_with_the_linear_form() {
    let s = chain();
    let non_linear = s
        .datalog(&format!("{PREFIX}{NON_LINEAR}?- path(?x, ?y)."))
        .unwrap();
    let linear = s
        .datalog(&format!(
            "{PREFIX}path(?x, ?y) :- ex:succ(?x, ?y).\n\
             path(?x, ?z) :- ex:succ(?x, ?y), path(?y, ?z).\n?- path(?x, ?y)."
        ))
        .unwrap();
    let key = |r: &oxilite_datalog::DatalogResult| {
        let mut v: Vec<String> = r
            .rows
            .iter()
            .map(|row| format!("{}|{}", fmt(&row[0]), fmt(&row[1])))
            .collect();
        v.sort();
        v
    };
    assert_eq!(key(&non_linear), key(&linear));
    assert_eq!(non_linear.rows.len(), 10, "4 + 3 + 2 + 1 pairs on the chain");
}

// @lat: [[tests#Datalog#Doubling reaches the fixpoint faster]]
#[test]
fn the_non_linear_rule_doubles_each_round() {
    let s = chain();
    let r = s
        .datalog(&format!("{PREFIX}{NON_LINEAR}?- path(?x, ?y)."))
        .unwrap();
    // A non-linear closure composes the relation with itself, so the longest chain (4 hops)
    // is reached by doubling: far fewer rounds than the linear formulation would take.
    assert_eq!(r.rounds.len(), 1, "one iterated component");
    assert!(r.rounds[0] <= 4, "took {} rounds", r.rounds[0]);
}

// @lat: [[tests#Datalog#Iterated component is reported]]
#[test]
fn explain_reports_the_iterated_component() {
    let s = chain();
    let explain = s
        .explain_datalog(&format!("{PREFIX}{NON_LINEAR}?- path(?x, ?y)."))
        .unwrap();
    assert!(explain.contains("non-linear"), "{explain}");
    assert!(
        explain.contains("one request per round"),
        "the cost must be stated: {explain}"
    );
}

// @lat: [[tests#Datalog#Iteration bound]]
#[test]
fn a_divergent_component_hits_the_bound() {
    let s = chain();
    let opts = Options {
        max_iterations: 1,
        ..Options::default()
    };
    let err = s
        .datalog_with(&format!("{PREFIX}{NON_LINEAR}?- path(?x, ?y)."), &opts)
        .unwrap_err();
    assert!(err.to_string().contains("fixpoint"), "got: {err}");
}

// @lat: [[tests#Datalog#Iteration leaves no rows behind]]
#[test]
fn iteration_cleans_up_its_work_rows() {
    let s = chain();
    for _ in 0..2 {
        let r = s
            .datalog(&format!("{PREFIX}{NON_LINEAR}?- path(?x, ?y)."))
            .unwrap();
        assert_eq!(r.rows.len(), 10, "a second run must not see the first's rows");
    }
}

fn fmt(t: &Option<oxrdf::Term>) -> String {
    t.as_ref().map(ToString::to_string).unwrap_or_default()
}

// @lat: [[tests#Datalog#Linear rewrite of a non-linear closure]]
#[test]
fn the_linear_rewrite_gives_the_same_answer() {
    let s = chain();
    let r = s
        .datalog(&format!(
            "{PREFIX}path(?x, ?y) :- ex:succ(?x, ?y).\n\
             path(?x, ?z) :- ex:succ(?x, ?y), path(?y, ?z).\n?- path(?x, ?y)."
        ))
        .unwrap();
    // 4 + 3 + 2 + 1 pairs along the chain.
    assert_eq!(r.rows.len(), 10);
}

// @lat: [[tests#Datalog#Derived facts become queryable]]
#[test]
fn materialized_rules_are_visible_to_sparql() {
    let s = Store::new().unwrap();
    s.update(
        "PREFIX ex: <http://example.org/>
         INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
    )
    .unwrap();
    let program = format!(
        "{PREFIX}ex:ancestor(?x, ?y) :- ex:parent(?x, ?y).\n\
         ex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z)."
    );
    let stats = s.datalog_materialize(&program).unwrap();
    assert_eq!(stats.relations, 1);
    assert_eq!(stats.inferred, 3, "ada->bob, bob->cy, ada->cy");

    // Without inferences the derived predicate is not there.
    let plain = s
        .query("PREFIX ex: <http://example.org/> SELECT ?x ?y WHERE { ?x ex:ancestor ?y }")
        .unwrap();
    assert_eq!(count(plain), 0);

    // With inferences it is.
    let options = oxilite::sparql::QueryOptions {
        include_inferred: true,
        ..Default::default()
    };
    let inferred = s
        .query_opt(
            "PREFIX ex: <http://example.org/> SELECT ?x ?y WHERE { ?x ex:ancestor ?y }",
            options,
        )
        .unwrap();
    assert_eq!(count(inferred), 3);
}

// @lat: [[tests#Datalog#Asserted data is untouched]]
#[test]
fn materializing_does_not_touch_asserted_data() {
    let s = Store::new().unwrap();
    s.update(
        "PREFIX ex: <http://example.org/>
         INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
    )
    .unwrap();
    let before = s.len().unwrap();
    s.datalog_materialize(&format!(
        "{PREFIX}ex:ancestor(?x, ?y) :- ex:parent(?x, ?y).\n\
         ex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z)."
    ))
    .unwrap();
    assert_eq!(s.len().unwrap(), before, "asserted quads must be unchanged");
    s.clear_inferences().unwrap();
    assert_eq!(s.len().unwrap(), before);
}

// @lat: [[tests#Datalog#Re-running replaces]]
#[test]
fn re_running_materialization_replaces_the_previous_set() {
    let s = Store::new().unwrap();
    s.update(
        "PREFIX ex: <http://example.org/>
         INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
    )
    .unwrap();
    let full = format!(
        "{PREFIX}ex:ancestor(?x, ?y) :- ex:parent(?x, ?y).\n\
         ex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z)."
    );
    assert_eq!(s.datalog_materialize(&full).unwrap().inferred, 3);
    // Only the base rule now: the transitive pair must be gone.
    let base = format!("{PREFIX}ex:ancestor(?x, ?y) :- ex:parent(?x, ?y).");
    assert_eq!(s.datalog_materialize(&base).unwrap().inferred, 2);
}

// @lat: [[tests#Datalog#Non-triple head is rejected]]
#[test]
fn a_head_with_no_rdf_form_is_rejected() {
    let s = chain();
    let err = s
        .datalog_materialize(&format!("{PREFIX}p(?x, ?y) :- ex:succ(?x, ?y)."))
        .unwrap_err();
    assert!(err.to_string().contains("materialize"), "got: {err}");
}

// @lat: [[tests#Datalog#Unary head materializes as rdf:type]]
#[test]
fn a_unary_head_materializes_as_rdf_type() {
    let s = Store::new().unwrap();
    s.update(
        "PREFIX ex: <http://example.org/>
         INSERT DATA { ex:ada ex:age 42 . ex:bob ex:age 12 }",
    )
    .unwrap();
    s.datalog_materialize(&format!(
        "{PREFIX}ex:Adult(?p) :- ex:age(?p, ?a), ?a >= 18."
    ))
    .unwrap();
    let options = oxilite::sparql::QueryOptions {
        include_inferred: true,
        ..Default::default()
    };
    let r = s
        .query_opt(
            "PREFIX ex: <http://example.org/> SELECT ?p WHERE { ?p a ex:Adult }",
            options,
        )
        .unwrap();
    assert_eq!(count(r), 1);
}

// @lat: [[tests#Datalog#Backend without compound recursive CTEs]]
#[test]
fn mutual_recursion_needs_the_capability() {
    use oxilite_core::sql::Capabilities;
    let mut caps = Capabilities::native();
    caps.compound_recursive_cte = false;
    let err = oxilite_datalog::compile(
        &format!("{PREFIX}{EVEN_ODD}?- even(?x)."),
        &caps,
        &Options::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("3.34.0"), "got: {err}");
}

fn count(r: oxilite::sparql::QueryResults) -> usize {
    match r {
        oxilite::sparql::QueryResults::Solutions(s) => s.count(),
        _ => panic!("expected solutions"),
    }
}

// @lat: [[tests#Datalog#Materializing an iterated component]]
#[test]
fn materializing_a_non_linear_component_works() {
    let s = chain();
    let stats = s
        .datalog_materialize(&format!(
            "{PREFIX}ex:reaches(?x, ?y) :- ex:succ(?x, ?y).\n\
             ex:reaches(?x, ?z) :- ex:reaches(?x, ?y), ex:reaches(?y, ?z)."
        ))
        .unwrap();
    assert_eq!(stats.inferred, 10, "the full closure over a 5-node chain");

    let options = oxilite::sparql::QueryOptions {
        include_inferred: true,
        ..Default::default()
    };
    let r = s
        .query_opt(
            "PREFIX ex: <http://example.org/> SELECT ?y WHERE { ex:n0 ex:reaches ?y }",
            options,
        )
        .unwrap();
    assert_eq!(count(r), 4, "n0 reaches n1..n4");
}

// @lat: [[tests#Datalog#Iterated materialization cleans up]]
#[test]
fn materializing_an_iterated_component_cleans_up() {
    let s = chain();
    let program = format!(
        "{PREFIX}ex:reaches(?x, ?y) :- ex:succ(?x, ?y).\n\
         ex:reaches(?x, ?z) :- ex:reaches(?x, ?y), ex:reaches(?y, ?z)."
    );
    // Running twice must give the same answer: the work rows of the first run are gone, and
    // the second run does not read the inferences the first one wrote.
    assert_eq!(s.datalog_materialize(&program).unwrap().inferred, 10);
    assert_eq!(s.datalog_materialize(&program).unwrap().inferred, 10);
}
