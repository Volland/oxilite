use super::editor::{complete_command, complete_sparql};
use super::input::{balanced, complete};
use super::prefixes::Prefixes;
use super::render::{fit_widths, table, Cell, Paint};
use super::{Flow, Session};
use crate::db::Db;
use crate::Location;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Default)]
struct Buf(Rc<RefCell<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.borrow_mut())).unwrap()
    }
}

fn memory() -> Location {
    Location::default()
}

fn session() -> (Session, Buf, Buf) {
    let (out, err) = (Buf::default(), Buf::default());
    let db = Db::open(&memory()).unwrap();
    let s = Session::new(db, memory(), Box::new(out.clone()), Box::new(err.clone()));
    (s, out, err)
}

fn feed(s: &mut Session, text: &str) {
    for line in text.lines() {
        assert!(matches!(s.feed_line(line), Flow::Continue));
    }
}

// @lat: [[tests#Shell#Statements run when complete]]
#[test]
fn statements_run_when_complete() {
    assert!(balanced("SELECT * { ?s ?p \"}\" } # {"));
    assert!(!balanced("SELECT * { ?s ?p ?o ;"));
    assert!(!balanced("INSERT DATA { <a:s> <a:p> \"\"\"open"));
    assert!(balanced("SELECT * { ?s ?p ?o FILTER(?o < 3) }"));
    let parses = |t: &str| t.starts_with("SELECT") && t.ends_with('}');
    assert!(complete("SELECT * { ?s ?p ?o }", true, false, parses));
    // A multi-line statement waits for `;` or an empty line even when it parses.
    assert!(!complete("SELECT * {\n?s ?p ?o }", false, false, parses));
    assert!(complete(
        "SELECT * {\n?s ?p ?o }\nLIMIT 2;",
        false,
        false,
        parses
    ));
    assert!(complete("SELECT * {\n?s ?p ?o }\n", false, true, parses));
    assert!(!complete("SELECT * {\n?s ?p ?o ;", false, false, parses));

    let (mut s, out, err) = session();
    feed(&mut s, "INSERT DATA { <a:s> <a:p> <a:o> }");
    feed(&mut s, "SELECT * WHERE {\n  ?s ?p ?o ;\n     ?q ?r }");
    assert!(s.is_pending() && !out.take().contains("a:o"));
    feed(&mut s, "LIMIT 5;");
    assert!(!s.is_pending());
    let text = out.take();
    assert_eq!(text.matches("1 row").count(), 1, "{text}");
    feed(&mut s, "SELEC * { ?s ?p ?o }\n\n");
    assert!(err.take().contains("Error"));
    assert!(!s.is_pending());
}

// @lat: [[tests#Shell#Exit stops at once]]
#[test]
fn exit_stops_at_once() {
    let (mut s, _, _) = session();
    assert!(matches!(s.feed_line(".exit"), Flow::Exit(0)));
    assert!(matches!(s.feed_line(".quit"), Flow::Exit(0)));
    assert!(matches!(s.feed_line("  .exit 4"), Flow::Exit(4)));
}

// @lat: [[tests#Shell#Session prefixes]]
#[test]
fn session_prefixes() {
    let mut p = Prefixes::default();
    let text = p.declare_missing("SELECT * { ?s foaf:name ?n }");
    assert!(text.starts_with("PREFIX foaf: <http://xmlns.com/foaf/0.1/> SELECT"));
    // A declared prefix is not declared twice, and an unknown one is left to the parser.
    let text = p.declare_missing("PREFIX foaf: <http://x/> SELECT * { ?s foaf:name ?n ; ex:p ?o }");
    assert!(text.starts_with("PREFIX foaf: <http://x/>"), "{text}");
    p.learn("PREFIX ex: <http://example.com/> ASK {}");
    assert_eq!(
        p.compact("http://example.com/alice").as_deref(),
        Some("ex:alice")
    );
    assert_eq!(p.compact("http://example.com/a b"), None);

    let (mut s, out, err) = session();
    feed(&mut s, "PREFIX ex: <http://example.com/>");
    feed(&mut s, "INSERT DATA { ex:alice ex:knows ex:bob }");
    feed(&mut s, "SELECT ?o { ex:alice ex:knows ?o }");
    assert!(out.take().contains("ex:bob"));
    assert!(err.take().is_empty());
}

// @lat: [[tests#Shell#Tables fit the terminal]]
#[test]
fn tables_fit_the_terminal() {
    assert_eq!(fit_widths(&[5, 10], Some(80)), vec![5, 10]);
    let w = fit_widths(&[5, 300, 40], Some(60));
    assert!(w.iter().sum::<usize>() + 3 * 3 < 60, "{w:?}");
    assert_eq!(w[0], 5);
    let rows = vec![vec![Cell::plain("ex:alice"), Cell::plain("x".repeat(500))]];
    let t = table(
        &["s".into(), "o".into()],
        &rows,
        Some(40),
        Paint { color: false },
    );
    assert!(t.lines().all(|l| l.width() <= 40), "{t}");
    assert!(t.contains('…'));
}

// @lat: [[tests#Shell#Completion knows the store]]
#[test]
fn completion_knows_the_store() {
    let (mut s, _, _) = session();
    feed(&mut s, "PREFIX ex: <http://example.com/>");
    feed(
        &mut s,
        "INSERT DATA { ex:alice ex:knows ex:bob . ex:alice a ex:Person }",
    );
    let line = "SELECT * { ?s ex:kn";
    let (start, items) = complete_sparql(&mut s, line, line.len());
    assert_eq!(&line[start..], "ex:kn");
    assert!(
        items.iter().any(|p| p.replacement == "ex:knows"),
        "{:?}",
        items.iter().map(|p| &p.replacement).collect::<Vec<_>>()
    );
    let line = "SELECT * { ?s a ex:";
    let (_, items) = complete_sparql(&mut s, line, line.len());
    assert!(items.iter().any(|p| p.replacement == "ex:Person"));
    let line = "sel";
    let (start, items) = complete_sparql(&mut s, line, line.len());
    assert_eq!(start, 0);
    assert!(items.iter().any(|p| p.replacement == "SELECT"));
    // Earlier lines of the statement count.
    feed(&mut s, "SELECT ?who WHERE {");
    let line = "  ?w";
    let (_, items) = complete_sparql(&mut s, line, line.len());
    assert!(items.iter().any(|p| p.replacement == "?who"));
    s.cancel();

    let (start, items) = complete_command(&s, ".ex", 3).unwrap();
    assert_eq!(start, 0);
    let names: Vec<_> = items.iter().map(|p| p.replacement.as_str()).collect();
    assert_eq!(names, [".exit", ".explain"]);
    let (_, items) = complete_command(&s, ".mode ts", 8).unwrap();
    assert_eq!(items[0].replacement, "tsv");
    assert!(complete_command(&s, ".load da", 8).is_none());
}

// @lat: [[tests#Shell#Save copies the store]]
#[test]
fn save_copies_the_store() {
    let dir = std::env::temp_dir().join(format!("oxilite-shell-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("out.sqlite");
    let (mut s, _, err) = session();
    feed(
        &mut s,
        "INSERT DATA { <a:s> <a:p> \"x\" . GRAPH <a:g> { <a:s> <a:p> 1 } }",
    );
    feed(&mut s, &format!(".save {}", file.display()));
    assert!(err.take().is_empty());
    feed(&mut s, &format!(".save {}", file.display()));
    assert!(err.take().contains("already exists"));
    let saved = Db::open(&Location {
        location: Some(file.display().to_string()),
        ..memory()
    })
    .unwrap();
    let out = saved
        .query(
            // The data; the system graphs (vocabulary, registry) are copied too.
            "SELECT * { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } FILTER(!STRSTARTS(STR(?g), \"oxilite:\")) } }",
            &[],
            &[],
        )
        .unwrap();
    let oxilite_core::QueryOutput::Solutions { rows, .. } = out else {
        panic!()
    };
    assert_eq!(rows.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

// @lat: [[tests#Shell#Schema registry commands]]
#[test]
fn schema_registry_commands() {
    let dir = std::env::temp_dir().join(format!("oxilite-shell-registry-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let onto = dir.join("onto.ttl");
    std::fs::write(
        &onto,
        "@prefix ex: <http://ex.org/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         ex:Dog rdfs:subClassOf ex:Animal .\n",
    )
    .unwrap();
    let (mut s, out, err) = session();
    feed(&mut s, "PREFIX ex: <http://ex.org/>");
    out.take();
    feed(
        &mut s,
        &format!(".register ontology ex:onto {}", onto.display()),
    );
    assert!(err.take().is_empty());
    assert!(out.take().contains("Registered ex:onto as ontology"));
    feed(&mut s, ".registry");
    let table = out.take();
    assert!(
        table.contains("ex:onto") && table.contains("ontology"),
        "{table}"
    );
    assert!(table.contains("yes"), "active: {table}");
    // A role the registry does not know is refused.
    feed(&mut s, ".register taxonomy ex:other");
    assert!(err.take().contains("one of ontology, shacl, shex"));

    feed(
        &mut s,
        "INSERT DATA { GRAPH ex:shapes { ex:S a <http://www.w3.org/ns/shacl#NodeShape> ; \
         <http://www.w3.org/ns/shacl#targetClass> ex:Person ; \
         <http://www.w3.org/ns/shacl#property> [ <http://www.w3.org/ns/shacl#path> ex:age ; \
         <http://www.w3.org/ns/shacl#maxCount> 1 ] } }",
    );
    feed(&mut s, ".register shacl ex:shapes");
    out.take();
    feed(&mut s, ".shapes");
    let shapes = out.take();
    assert!(
        shapes.contains("ex:Person") && shapes.contains("maxCount 1"),
        "{shapes}"
    );

    feed(&mut s, ".map ex:onto ex:data DEFAULT");
    feed(&mut s, ".registry");
    let mapped = out.take();
    assert!(
        mapped.contains("ex:data") && mapped.contains("DEFAULT"),
        "{mapped}"
    );
    feed(&mut s, ".map ex:onto ALL");
    feed(&mut s, ".registry");
    assert!(out.take().contains("all graphs"));
    feed(&mut s, ".map ex:nothing ex:data");
    assert!(err.take().contains("is not registered"));
    feed(&mut s, ".deactivate ex:onto");
    feed(&mut s, ".registry");
    assert!(out.take().contains("no"));
    feed(&mut s, ".activate ex:onto");
    feed(&mut s, ".unregister ex:shapes --drop");
    assert!(out.take().contains("Dropped ex:shapes"));
    feed(&mut s, ".unregister ex:onto");
    feed(&mut s, ".unregister ex:onto");
    assert!(err.take().contains("is not registered"));
    feed(&mut s, ".registry");
    assert!(out.take().contains("No schema graph registered"));
    std::fs::remove_dir_all(dir).ok();
}

// @lat: [[tests#Shell#Session query options]]
#[test]
fn session_query_options() {
    let (mut s, out, err) = session();
    feed(
        &mut s,
        "INSERT DATA { <http://ex.org/Dog> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex.org/Animal> . \
         <http://ex.org/rex> a <http://ex.org/Dog> }",
    );
    let animals = "SELECT ?x WHERE { ?x a <http://ex.org/Animal> }";
    out.take();
    feed(&mut s, animals);
    assert!(out.take().contains("0 rows"));
    feed(&mut s, ".reasoning");
    assert!(out.take().contains("none"));
    feed(&mut s, ".reasoning rdfs");
    feed(&mut s, animals);
    assert!(out.take().contains("rex"));
    feed(&mut s, ".explain");
    assert!(
        out.take().contains("tbox_closure"),
        "explain uses the session options"
    );
    // The shell's own reads keep the default options: a dump holds asserted quads only.
    feed(&mut s, ".dump");
    let dump = out.take();
    assert!(
        !dump.contains("<http://ex.org/rex> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex.org/Animal>"),
        "{dump}"
    );
    feed(&mut s, ".reasoning none");
    feed(&mut s, animals);
    assert!(out.take().contains("0 rows"));
    feed(&mut s, ".reasoning maybe");
    assert!(err.take().contains("one of none, rdfs, owl-ql"));

    feed(
        &mut s,
        "INSERT DATA { <http://ex.org/a> <http://www.w3.org/2002/07/owl#sameAs> <http://ex.org/b> . \
         <http://ex.org/a> <http://ex.org/name> \"A\" }",
    );
    feed(&mut s, ".materialize");
    assert!(out.take().contains("inferred triple"));
    let b = "SELECT ?n WHERE { <http://ex.org/b> <http://ex.org/name> ?n }";
    feed(&mut s, b);
    assert!(out.take().contains("0 rows"));
    feed(&mut s, ".inferred on");
    feed(&mut s, b);
    let r = out.take();
    assert!(r.contains("1 row"), "{r}");
    feed(&mut s, ".inferred");
    assert!(out.take().contains("on"));
    feed(&mut s, ".materialize clear");
    feed(&mut s, b);
    assert!(out.take().contains("0 rows"));

    feed(&mut s, ".schemagraphs off");
    feed(&mut s, ".schemagraphs");
    assert!(out.take().contains("off"));
    assert!(err.take().is_empty());
}
