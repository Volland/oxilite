//! `oxilite n3` and `oxilite elpi`, as a user runs them.

use std::process::{Command, Output};

fn oxilite(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxilite"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> String {
    let out = oxilite(args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A scratch directory with a family graph loaded into `kg.sqlite`.
fn family(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("oxilite-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("family.ttl");
    std::fs::write(
        &data,
        "@prefix ex: <http://ex.org/> .
         ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy .
         ex:ada a ex:Person ; ex:name \"Ada\" . ex:bob a ex:Person .",
    )
    .unwrap();
    let db = dir.join("kg.sqlite").to_str().unwrap().to_owned();
    ok(&["load", "-l", &db, "-f", data.to_str().unwrap()]);
    (dir, db)
}

// @tg: verifies:: [[openspec:n3-rules#Command line#Materialize from the command line]] {since: "2026-11-03", level: e2e}
// @tg: verifies:: [[tests#Command line#N3 rules from the command line]] {since: "2026-11-03", level: e2e}
#[test]
fn n3_rules_from_the_command_line() {
    let (dir, db) = family("n3");
    let rules = dir.join("rules.n3");
    std::fs::write(
        &rules,
        "@prefix ex: <http://ex.org/> .
         { ?x ex:parent ?y . ?y ex:parent ?z } => { ?x ex:grandparent ?z } .",
    )
    .unwrap();
    let rules = rules.to_str().unwrap();
    let out = ok(&["n3", "-l", &db, "-f", rules]);
    assert_eq!(
        out.trim(),
        "<http://ex.org/ada> <http://ex.org/grandparent> <http://ex.org/cy> ."
    );
    let out = ok(&["n3", "-l", &db, "-f", rules, "--materialize"]);
    assert!(out.starts_with("1 inferred triple(s)"), "{out}");
    let q = "SELECT ?z WHERE { <http://ex.org/ada> <http://ex.org/grandparent> ?z }";
    assert!(ok(&["query", "-l", &db, "--inferred", "-q", q]).contains("http://ex.org/cy"));
    assert!(!ok(&["query", "-l", &db, "-q", q]).contains("http://ex.org/cy"));
    std::fs::remove_dir_all(&dir).unwrap();
}

// @tg: verifies:: [[openspec:elpi-programs#Command line#Check from the command line]] {since: "2026-11-03", level: e2e}
// @tg: verifies:: [[tests#Command line#ELPI programs from the command line]] {since: "2026-11-03", level: e2e}
#[test]
fn elpi_programs_from_the_command_line() {
    let (dir, db) = family("elpi");
    let program = dir.join("rules.elpi");
    std::fs::write(
        &program,
        r#"
        pred anc o:term, o:term.
        anc X Y :- ox.triple X (iri "http://ex.org/parent") Y.
        anc X Z :- ox.triple X (iri "http://ex.org/parent") Y, anc Y Z.
        ox.violation error "missing-name" "a person needs a name" [P] :-
          ox.triple P (iri "http://www.w3.org/1999/02/22-rdf-syntax-ns#type") (iri "http://ex.org/Person"),
          not (ox.triple P (iri "http://ex.org/name") _).
        "#,
    )
    .unwrap();
    let program = program.to_str().unwrap();
    let goal = r#"anc (iri "http://ex.org/ada") Y"#;
    if cfg!(feature = "elpi") {
        let out = ok(&["elpi", "-l", &db, "-f", program, "--goal", goal]);
        assert_eq!(out.lines().next(), Some("Y"));
        assert_eq!(out.lines().count(), 3, "{out}");
        let check = oxilite(&["elpi", "-l", &db, "-f", program, "--check"]);
        assert_eq!(check.status.code(), Some(1));
        let out = String::from_utf8_lossy(&check.stdout);
        assert!(out.starts_with("error\tmissing-name\t"), "{out}");
        assert!(out.contains("<http://ex.org/bob>"), "{out}");
    } else {
        let out = oxilite(&["elpi", "-l", &db, "-f", program, "--goal", goal]);
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("--features elpi"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
