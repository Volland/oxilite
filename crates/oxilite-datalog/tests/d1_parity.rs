//! What the compiler emits must fit D1's limits, and must never use a SQLite feature D1 has
//! not declared. These checks need no D1 binding: they read the SQL a program compiles to
//! against `Capabilities::d1()`.

use oxilite_core::sql::Capabilities;
use oxilite_datalog::{compile, materialize_plan, Options};

const PREFIX: &str = "@prefix ex: <http://example.org/> .\n";

fn d1() -> Capabilities {
    Capabilities::d1()
}

/// Every statement a plan would send, so limits can be checked on all of them at once.
fn statements(program: &str) -> Vec<String> {
    let caps = d1();
    let options = Options::default();
    let compiled = compile(program, &caps, &options).expect("compiles");
    let mut out = vec![compiled.sql.clone()];
    for phase in &compiled.fixpoint.phases {
        out.extend(phase.seed.iter().cloned());
        out.extend(phase.step.iter().cloned());
    }
    out
}

/// Counts the terms of the longest compound SELECT, which D1 caps at five.
fn longest_compound(sql: &str) -> usize {
    // Terms are separated by UNION / UNION ALL / EXCEPT / INTERSECT; count per parenthesis
    // depth so that a nested compound is measured on its own.
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut counts = vec![1usize];
    let mut max = 1usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                depth += 1;
                counts.push(1);
            }
            b')' => {
                if let Some(c) = counts.pop() {
                    max = max.max(c);
                }
                depth = depth.saturating_sub(1);
            }
            b'U' | b'u' if sql[i..].len() >= 5 && sql[i..5 + i].eq_ignore_ascii_case("UNION") => {
                let n = counts.last_mut().expect("a level is always open");
                *n += 1;
                max = max.max(*n);
                i += 4;
            }
            _ => {}
        }
        i += 1;
    }
    let _ = depth;
    for c in counts {
        max = max.max(c);
    }
    max
}

// @lat: [[tests#Datalog#D1 statement length]]
#[test]
fn no_statement_exceeds_the_d1_length_limit() {
    let caps = d1();
    let programs = [
        format!("{PREFIX}p(?x, ?y) :- ex:parent(?x, ?y).\n?- p(?x, ?y)."),
        format!(
            "{PREFIX}anc(?x, ?y) :- ex:parent(?x, ?y).\n\
             anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).\n\
             adult(?x, ?y) :- anc(?x, ?y), ex:age(?y, ?a), ?a >= 18.\n\
             root(?x) :- ex:Person(?x), not anc(_, ?x).\n\
             n(?x, COUNT(?y)) :- anc(?x, ?y).\n?- adult(?x, ?y)."
        ),
        format!(
            "{PREFIX}p(?x, ?y) :- ex:parent(?x, ?y).\n\
             p(?x, ?z) :- p(?x, ?y), p(?y, ?z).\n?- p(?x, ?y)."
        ),
    ];
    for program in &programs {
        for sql in statements(program) {
            assert!(
                sql.len() <= caps.max_sql_len,
                "statement of {} bytes exceeds D1's {} byte limit:\n{sql}",
                sql.len(),
                caps.max_sql_len
            );
        }
    }
}

// @lat: [[tests#Datalog#D1 compound limit]]
#[test]
fn no_compound_exceeds_the_d1_term_limit() {
    let caps = d1();
    // Five rules for one predicate is five compound terms, which is exactly D1's limit.
    let program = format!(
        "{PREFIX}p(?x) :- ex:A(?x).\np(?x) :- ex:B(?x).\np(?x) :- ex:C(?x).\n\
         p(?x) :- ex:D(?x).\np(?x) :- ex:E(?x).\n?- p(?x)."
    );
    for sql in statements(&program) {
        assert!(
            longest_compound(&sql) <= caps.max_compound_select,
            "compound of {} terms exceeds D1's {}:\n{sql}",
            longest_compound(&sql),
            caps.max_compound_select
        );
    }
}

// @lat: [[tests#Datalog#D1 never gets a compound recursive term]]
#[test]
fn d1_never_gets_a_compound_recursive_term() {
    // D1's SQLite version is not ours to assume, so mutual recursion must not compile to a
    // member with several recursive terms there.
    assert!(!d1().compound_recursive_cte);
    let program = format!(
        "{PREFIX}even(?x) :- ex:Zero(?x).\n\
         even(?x) :- ex:succ(?y, ?x), odd(?y).\n\
         odd(?x) :- ex:succ(?y, ?x), even(?y).\n?- even(?x)."
    );
    let err = compile(&program, &d1(), &Options::default()).unwrap_err();
    assert!(err.to_string().contains("3.34.0"), "got: {err}");
}

// @lat: [[tests#Datalog#D1 never needs user-defined functions]]
#[test]
fn a_function_needing_udfs_is_refused_on_d1() {
    // D1 provides no user-defined functions, so REGEX must be refused rather than emitted.
    assert!(!d1().udf);
    let program = format!(
        "{PREFIX}a(?p) :- ex:name(?p, ?n), REGEX(?n, \"^A\").\n?- a(?p)."
    );
    let err = compile(&program, &d1(), &Options::default()).unwrap_err();
    assert!(err.to_string().contains("user-defined functions"), "got: {err}");
}

// @lat: [[tests#Datalog#D1 materialization stays within the statement budget]]
#[test]
fn materialization_fits_d1s_statement_budget() {
    let caps = d1();
    let program = format!(
        "{PREFIX}ex:ancestor(?x, ?y) :- ex:parent(?x, ?y).\n\
         ex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z)."
    );
    let (_, statements, _) = materialize_plan(&program, &caps, &Options::default()).unwrap();
    assert!(
        statements.len() <= caps.max_statements,
        "{} statements exceeds D1's {} per request",
        statements.len(),
        caps.max_statements
    );
    for s in &statements {
        assert!(
            s.sql.len() <= caps.max_sql_len,
            "statement of {} bytes exceeds D1's limit:\n{}",
            s.sql.len(),
            s.sql
        );
    }
}

// @lat: [[tests#Datalog#Bound parameters are never needed]]
#[test]
fn statements_are_self_contained() {
    // The backend contract is that statements carry their own constants; a `?` placeholder
    // would mean a bound parameter D1 is never given.
    let program = format!(
        "{PREFIX}p(?x) :- ex:age(?x, ?a), ?a >= 18, ?a < 65.\n?- p(?x)."
    );
    for sql in statements(&program) {
        assert!(!sql.contains('?'), "placeholder in:\n{sql}");
    }
}
