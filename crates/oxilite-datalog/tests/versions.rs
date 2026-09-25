//! Datalog over a past version of a versioned store (`@version`, `Options::as_of`).

use oxilite::store::Store;
use oxilite::version::Versioning;
use oxilite::StoreOptions;
use oxilite_datalog::{DatalogError, Options};

fn store() -> Store {
    let s = Store::with_backend_and_options(
        oxilite::rusqlite::RusqliteBackend::memory().unwrap(),
        &StoreOptions {
            versioning: Versioning::Log,
            ..Default::default()
        },
    )
    .unwrap();
    s.update("PREFIX ex: <http://example.org/> INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }")
        .unwrap();
    s.update("PREFIX ex: <http://example.org/> INSERT DATA { ex:cy ex:parent ex:dee }")
        .unwrap();
    s
}

const ANCESTORS: &str = "@prefix ex: <http://example.org/> .
anc(?x, ?y) :- ex:parent(?x, ?y).
anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).
?- anc(ex:ada, ?y).";

// @lat: [[tests#Versioning#Datalog reads a past version]]
#[test]
fn datalog_reads_a_past_version() {
    let s = store();
    assert_eq!(s.datalog(ANCESTORS).unwrap().rows.len(), 3);
    let past = format!("@version \"HEAD~1\" .\n{ANCESTORS}");
    assert_eq!(s.datalog(&past).unwrap().rows.len(), 2);
    let opts = Options {
        as_of: Some("HEAD~1".into()),
        ..Default::default()
    };
    assert_eq!(s.datalog_with(ANCESTORS, &opts).unwrap().rows.len(), 2);
    // Materialization derives from the current state only.
    assert!(matches!(
        s.datalog_materialize(&past),
        Err(DatalogError::Unsupported(_))
    ));
    // An unversioned store keeps no history.
    let plain = Store::new().unwrap();
    let err = plain.datalog(&past).unwrap_err().to_string();
    assert!(err.contains("keeps no history"), "{err}");
}

fn tickets() -> Store {
    let s = Store::with_backend_and_options(
        oxilite::rusqlite::RusqliteBackend::memory().unwrap(),
        &StoreOptions {
            versioning: Versioning::Log,
            ..Default::default()
        },
    )
    .unwrap();
    use oxilite::version::CommitInfo;
    let step = |author: &str, u: &str| {
        s.with_commit(
            CommitInfo {
                author: Some(author.into()),
                message: None,
            },
            |st| st.update(format!("PREFIX ex: <http://example.org/> {u}")),
        )
        .unwrap()
    };
    step(
        "ada",
        "INSERT DATA { ex:t1 ex:status \"open\" . ex:t2 ex:status \"open\" }",
    );
    step(
        "bob",
        "DELETE DATA { ex:t1 ex:status \"open\" } ; INSERT DATA { ex:t1 ex:status \"done\" }",
    );
    step(
        "cy",
        "DELETE DATA { ex:t2 ex:status \"open\" } ; INSERT DATA { ex:t2 ex:status \"blocked\" }",
    );
    s
}

fn strings(r: &oxilite_datalog::DatalogResult) -> Vec<String> {
    let mut v: Vec<String> = r
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| c.as_ref().map(ToString::to_string).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    v.sort();
    v
}

// @lat: [[tests#Versioning#Datalog compares versions per atom]]
#[test]
fn datalog_compares_versions_per_atom() {
    let s = tickets();
    let r = s
        .datalog(
            "@prefix ex: <http://example.org/> .
             changed(?t, ?old, ?new) :- ex:status(?t, ?new), ex:status(?t, ?old) at \"HEAD~1\", ?old != ?new.
             ?- changed(?t, ?old, ?new).",
        )
        .unwrap();
    assert_eq!(
        strings(&r),
        vec!["<http://example.org/t2> \"open\" \"blocked\""]
    );
    // `at ?c`: the status of t1 at every commit, bound by `commit`.
    let r = s
        .datalog(
            "@prefix ex: <http://example.org/> .
             status_at(?c, ?v) :- commit(?c, _, _, _), ex:status(ex:t1, ?v) at ?c.
             ?- status_at(?c, ?v).",
        )
        .unwrap();
    let values: Vec<String> = strings(&r)
        .into_iter()
        .map(|l| l.split(' ').nth(1).unwrap().to_owned())
        .collect();
    assert!(values.contains(&"\"open\"".to_owned()) && values.contains(&"\"done\"".to_owned()));
    // An `at` variable nothing binds is unsafe.
    let err = s
        .datalog("@prefix ex: <http://example.org/> . ?- ex:status(ex:t1, ?v) at ?c.")
        .unwrap_err();
    assert!(matches!(err, DatalogError::Unsafe { .. }), "{err}");
}

// @lat: [[tests#Versioning#Datalog reads the history relations]]
#[test]
fn datalog_reads_the_history_relations() {
    let s = tickets();
    // Who removed each status, and ancestry by recursion over `commit`.
    let r = s
        .datalog(
            "@prefix ex: <http://example.org/> .
             removed_by(?t, ?v, ?who) :- removed(?t, ex:status, ?v, _, ?c), commit(?c, _, _, ?who).
             ?- removed_by(?t, ?v, ?who).",
        )
        .unwrap();
    assert_eq!(
        strings(&r),
        vec![
            "<http://example.org/t1> \"open\" \"bob\"",
            "<http://example.org/t2> \"open\" \"cy\""
        ]
    );
    let r = s
        .datalog(
            "anc(?c, ?p) :- commit(?c, ?p, _, _), commit(?p, _, _, _).
             anc(?c, ?q) :- anc(?c, ?p), commit(?p, ?q, _, _), commit(?q, _, _, _).
             head(?c) :- branch(\"main\", ?c).
             ?- head(?c).",
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    // The graph column of a default-graph change reads as unbound (id 0 is not a term).
    let r = s.datalog("?- removed(?s, ?p, ?o, ?g, ?c).").unwrap();
    assert_eq!(r.rows.len(), 2);
    assert!(r.rows.iter().all(|row| row[3].is_none()));
    // A program's own relation named `commit` keeps working.
    let r = s
        .datalog("commit(?x) :- triple(?x, _, _). ?- commit(?x).")
        .unwrap();
    assert_eq!(r.rows.len(), 2);
    // Without versioning, the built-ins are refused.
    let plain = Store::new().unwrap();
    assert!(matches!(
        plain.datalog("?- commit(?c, ?p, ?t, ?a)."),
        Err(DatalogError::Unsupported(_))
    ));
}
