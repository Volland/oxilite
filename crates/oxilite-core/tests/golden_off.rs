//! An unversioned store generates exactly the SQL it generated before versioning existed.
//!
//! `golden/off-sql.tsv` was written by the `golden_sql` example from the commit before
//! versioning (0.3.1): one hash of the compiled SQL and planner notes per W3C query or update
//! and backend, and of the batch writer's statements.

mod golden {
    pub mod dump;
}

// @lat: [[tests#Versioning#Off store SQL matches the pre-versioning snapshot]]
#[test]
fn off_store_sql_matches_the_pre_versioning_snapshot() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testsuite/rdf-tests/sparql");
    if !root.join("sparql11").exists() {
        eprintln!("skipped: the rdf-tests submodule is not checked out");
        return;
    }
    let expected = include_str!("golden/off-sql.tsv");
    // Keyed by file and backend: a build with more parser features (unified from other crates)
    // parses a few more files, which the snapshot does not constrain.
    let actual: std::collections::HashMap<String, String> = golden::dump::dump(&root)
        .into_iter()
        .map(|line| {
            let (key, hash) = line.rsplit_once('\t').expect("key and hash");
            (key.to_owned(), hash.to_owned())
        })
        .collect();
    let mut changed = Vec::new();
    for line in expected.lines() {
        let (key, hash) = line.rsplit_once('\t').expect("key and hash");
        match actual.get(key) {
            Some(h) if h == hash => {}
            Some(h) => changed.push(format!("{key}: expected {hash}, got {h}")),
            None => changed.push(format!("{key}: no longer rendered")),
        }
    }
    assert!(
        changed.is_empty(),
        "{} of {} renderings changed for an unversioned store:\n{}",
        changed.len(),
        expected.lines().count(),
        changed.join("\n")
    );
}
