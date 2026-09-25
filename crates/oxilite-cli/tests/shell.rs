//! The shell as a user runs it: the `oxilite` binary with a script on standard input.

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn oxilite(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxilite"))
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("oxilite-shell-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// @lat: [[tests#Shell#A new file gets the schema]]
#[test]
fn a_new_file_gets_the_schema() {
    let dir = scratch("file");
    let file = dir.join("new.sqlite");
    let f = file.to_str().unwrap();
    let out = oxilite(&[f], "INSERT DATA { <a:s> <a:p> <a:o> }\n");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(file.exists());
    let out = oxilite(
        &["query", "-l", f, "-q", "SELECT ?o { <a:s> <a:p> ?o }"],
        "",
    );
    assert!(text(&out.stdout).contains("a:o"));
    // `-l` names the same store.
    let out = oxilite(&["-l", f], "SELECT ?o { <a:s> <a:p> ?o }\n");
    assert!(text(&out.stdout).contains("<a:o>"));
    let _ = std::fs::remove_dir_all(&dir);
}

// @lat: [[tests#Shell#In memory by default]]
#[test]
fn in_memory_by_default() {
    let dir = scratch("memory");
    let out = Command::new(env!("CARGO_BIN_EXE_oxilite"))
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            c.stdin
                .take()
                .unwrap()
                .write_all(b"INSERT DATA { <a:s> <a:p> <a:o> }\nSELECT * { ?s ?p ?o }\n")?;
            c.wait_with_output()
        })
        .unwrap();
    assert!(out.status.success());
    assert!(text(&out.stdout).contains("1 row"), "{}", text(&out.stdout));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    // Subcommands are unchanged.
    let out = oxilite(&["query", "-q", "ASK {}"], "");
    assert!(text(&out.stdout).contains("\"boolean\":true"));
    let _ = std::fs::remove_dir_all(&dir);
}

// @lat: [[tests#Shell#Scripts report failures]]
#[test]
fn scripts_report_failures() {
    let out = oxilite(
        &[],
        "ASK {}\nSELEC nothing\n\n.exit\nINSERT DATA { <a:s> <a:p> <a:o> }\n",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stdout).contains("true"));
    assert!(text(&out.stderr).contains("Error (line 2)"));
    let out = oxilite(&[], ".exit\nSELEC nothing\n");
    assert_eq!(out.status.code(), Some(0));
}
