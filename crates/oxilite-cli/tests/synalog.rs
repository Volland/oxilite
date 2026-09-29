//! `oxilite synalog`, as a user runs it.

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

// @lat: [[tests#Synalog#Run from the command line]]
#[test]
fn run_from_the_command_line() {
    let dir = std::env::temp_dir().join(format!("oxilite-synalog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("kg.sqlite");
    let db = db.to_str().unwrap();
    ok(&[
        "update",
        "-l",
        db,
        "-u",
        "PREFIX ex: <http://example.org/> INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
    ]);
    let program = "# @table parent <http://example.org/parent>
@OrderBy(Parent, \"child\");
Parent(child:, parent:) :- parent(subject: child, object: parent);";
    let out = ok(&["synalog", "Parent", "-l", db, "-p", program]);
    assert_eq!(
        out,
        "child\tparent\n\
         http://example.org/ada\thttp://example.org/bob\n\
         http://example.org/bob\thttp://example.org/cy\n"
    );
    let sql = ok(&["synalog", "Parent", "-l", db, "-p", program, "--sql"]);
    assert!(sql.contains("parent AS NOT MATERIALIZED"), "{sql}");
    let duck = ok(&["synalog", "Parent", "-p", program, "--engine", "duckdb"]);
    assert!(
        !duck.contains("NOT MATERIALIZED") && duck.contains("parent"),
        "{duck}"
    );
    let bad = oxilite(&["synalog", "Nope", "-l", db, "-p", program]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("does not define `Nope`"));
    let _ = std::fs::remove_dir_all(&dir);
}
