//! `oxilite registry`, `oxilite materialize` and the query flags, as a user runs them.

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

// @lat: [[tests#Command line#Registry and reasoning flags]]
#[test]
fn registry_and_reasoning_flags() {
    let dir = std::env::temp_dir().join(format!("oxilite-registry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("kg.sqlite");
    let db = db.to_str().unwrap();
    let write = |name: &str, text: &str| {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p.to_str().unwrap().to_owned()
    };
    let rdfs =
        "@prefix ex: <http://ex.org/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .";
    let good = write(
        "good.ttl",
        &format!("{rdfs} ex:Dog rdfs:subClassOf ex:Animal ."),
    );
    let bad = write(
        "bad.ttl",
        &format!("{rdfs} ex:Dog rdfs:subClassOf ex:Plant ."),
    );
    ok(&[
        "update",
        "-l",
        db,
        "-u",
        "INSERT DATA { <http://ex.org/rex> a <http://ex.org/Dog> }",
    ]);
    // A new store starts with the system graphs; `registry init` finds them current.
    let vocab = ok(&[
        "query",
        "-l",
        db,
        "-q",
        "ASK { GRAPH <oxilite:vocabulary> { <https://oxilite.dev/ns#appliesTo> ?p ?o } }",
    ]);
    assert!(vocab.contains("true"), "{vocab}");
    let init = oxilite(&["registry", "init", "-l", db]);
    assert!(String::from_utf8_lossy(&init.stderr).contains("already current"));
    let bare = dir.join("bare.sqlite");
    let bare = bare.to_str().unwrap();
    ok(&[
        "update",
        "--no-system-graphs",
        "-l",
        bare,
        "-u",
        "INSERT DATA { <a:s> <a:p> <a:o> }",
    ]);
    let ask = "ASK { GRAPH <oxilite:vocabulary> { ?s ?p ?o } }";
    assert!(ok(&["query", "-l", bare, "-q", ask]).contains("false"));
    ok(&["registry", "init", "-l", bare]);
    assert!(ok(&["query", "-l", bare, "-q", ask]).contains("true"));

    ok(&[
        "registry",
        "register",
        "http://ex.org/good",
        "--role",
        "ontology",
        "--file",
        &good,
        "--version",
        "v1",
        "-l",
        db,
    ]);
    // Loaded but not registered: with one ontology registered, it no longer counts.
    ok(&[
        "registry",
        "register",
        "<http://ex.org/bad>",
        "--role",
        "ontology",
        "--file",
        &bad,
        "-l",
        db,
    ]);
    ok(&["registry", "unregister", "http://ex.org/bad", "-l", db]);

    let list: serde_json::Value =
        serde_json::from_str(&ok(&["registry", "list", "--json", "-l", db])).unwrap();
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["graph"]["value"], "http://ex.org/good");
    assert_eq!(list[0]["role"], "ontology");
    assert_eq!(list[0]["version"], "v1");
    assert_eq!(list[0]["sha256"].as_str().unwrap().len(), 64);

    let plants = "SELECT ?x WHERE { ?x a <http://ex.org/Plant> }";
    let animals = "SELECT ?x WHERE { ?x a <http://ex.org/Animal> }";
    assert!(
        ok(&["query", "-l", db, "-q", animals, "--results-format", "csv"])
            .lines()
            .count()
            == 1
    );
    let rdfs_animals = ok(&[
        "query",
        "-l",
        db,
        "-q",
        animals,
        "--reasoning",
        "rdfs",
        "--results-format",
        "csv",
    ]);
    assert!(rdfs_animals.contains("http://ex.org/rex"), "{rdfs_animals}");
    let rdfs_plants = ok(&[
        "query",
        "-l",
        db,
        "-q",
        plants,
        "--reasoning",
        "rdfs",
        "--results-format",
        "csv",
    ]);
    assert!(
        !rdfs_plants.contains("rex"),
        "the unregistered ontology is out of scope"
    );
    assert!(
        ok(&["explain", "-l", db, "-q", animals, "--reasoning", "rdfs"]).contains("tbox_closure")
    );
    assert!(
        !oxilite(&["query", "-l", db, "-q", animals, "--reasoning", "maybe"])
            .status
            .success()
    );

    // Schema graphs hidden: only the (unregistered) bad ontology is left in the named graphs.
    let count = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";
    let n = |extra: &[&str]| {
        let mut a = vec!["query", "-l", db, "-q", count, "--results-format", "csv"];
        a.extend_from_slice(extra);
        ok(&a).lines().nth(1).unwrap().trim().to_owned()
    };
    // Visible: both ontologies, the registry graph's five triples about the good one (role,
    // active, version, sha256, registration time), and the system graphs the new store started
    // with (the vocabulary and the registry's own description: 74 triples).
    assert_eq!(n(&[]), "81");
    assert_eq!(n(&["--no-schema-graphs"]), "1");

    // Mapping: the good ontology applies to another graph only, then to every graph again.
    ok(&[
        "registry",
        "map",
        "http://ex.org/good",
        "--to",
        "http://ex.org/elsewhere",
        "-l",
        db,
    ]);
    let listed = ok(&["registry", "list", "-l", db]);
    assert!(
        listed.contains("applies to <http://ex.org/elsewhere>"),
        "{listed}"
    );
    let mapped = ok(&[
        "query",
        "-l",
        db,
        "-q",
        animals,
        "--reasoning",
        "rdfs",
        "--results-format",
        "csv",
    ]);
    assert!(
        !mapped.contains("rex"),
        "rex is in the default graph: {mapped}"
    );
    ok(&[
        "registry",
        "map",
        "http://ex.org/good",
        "--to",
        "DEFAULT",
        "-l",
        db,
    ]);
    let back = ok(&[
        "query",
        "-l",
        db,
        "-q",
        animals,
        "--reasoning",
        "rdfs",
        "--results-format",
        "csv",
    ]);
    assert!(back.contains("rex"), "{back}");
    ok(&[
        "registry",
        "map",
        "http://ex.org/good",
        "--to",
        "ALL",
        "-l",
        db,
    ]);
    assert!(ok(&["registry", "list", "-l", db]).contains("applies to all graphs"));

    ok(&["registry", "deactivate", "http://ex.org/good", "-l", db]);
    assert!(ok(&["registry", "list", "-l", db]).contains("inactive"));
    assert!(
        !oxilite(&["registry", "activate", "http://ex.org/nope", "-l", db])
            .status
            .success()
    );

    // Materialization and --inferred.
    ok(&["update", "-l", db, "-u", "INSERT DATA { <http://ex.org/a> <http://www.w3.org/2002/07/owl#sameAs> <http://ex.org/b> . <http://ex.org/a> <http://ex.org/name> \"A\" }"]);
    ok(&["materialize", "-l", db]);
    let name = "SELECT ?n WHERE { <http://ex.org/b> <http://ex.org/name> ?n }";
    assert!(ok(&[
        "query",
        "-l",
        db,
        "-q",
        name,
        "--inferred",
        "--results-format",
        "csv"
    ])
    .contains('A'));
    ok(&["materialize", "--clear", "-l", db]);
    assert!(!ok(&[
        "query",
        "-l",
        db,
        "-q",
        name,
        "--inferred",
        "--results-format",
        "csv"
    ])
    .contains('A'));

    // Shapes.
    let shapes = write(
        "shapes.ttl",
        "@prefix ex: <http://ex.org/> . @prefix sh: <http://www.w3.org/ns/shacl#> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
         ex:S a sh:NodeShape ; sh:targetClass ex:Person ; sh:property [ sh:path ex:age ; sh:datatype xsd:integer ; sh:minCount 1 ] .",
    );
    ok(&[
        "registry",
        "register",
        "http://ex.org/shapes",
        "--role",
        "shacl",
        "--file",
        &shapes,
        "-l",
        db,
    ]);
    let text = ok(&["registry", "shapes", "-l", db]);
    assert!(
        text.contains("http://ex.org/age") && text.contains("minCount 1"),
        "{text}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&ok(&["registry", "shapes", "--json", "-l", db])).unwrap();
    assert_eq!(
        json[0]["datatype"],
        "http://www.w3.org/2001/XMLSchema#integer"
    );
    ok(&["registry", "drop", "http://ex.org/shapes", "-l", db]);
    assert_eq!(ok(&["registry", "shapes", "--json", "-l", db]).trim(), "[]");
    std::fs::remove_dir_all(dir).ok();
}
