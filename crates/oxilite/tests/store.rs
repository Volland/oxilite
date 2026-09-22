//! Integration tests of M1 behaviour (see `openspec/changes/m1-storage-core/specs`).

use oxilite::core::sql::{Capabilities, Request, Response};
use oxilite::core::SyncBackend;
use oxilite::model::*;
use oxilite::rusqlite::RusqliteBackend;
use oxilite::sparql::QueryResults;
use oxilite::store::Store;
use oxilite::{Result, StoreOptions};
use std::cell::Cell;

/// Counts requests sent to the backend.
struct Counting {
    inner: RusqliteBackend,
    requests: std::sync::atomic::AtomicUsize,
}

impl SyncBackend for Counting {
    fn execute(&self, request: &Request) -> Result<Response> {
        self.requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.execute(request)
    }
    fn capabilities(&self) -> &Capabilities {
        self.inner.capabilities()
    }
}

fn ex(s: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("http://example.com/{s}"))
}

fn solutions(r: QueryResults<'_>) -> Vec<oxilite::sparql::QuerySolution> {
    let QueryResults::Solutions(s) = r else {
        panic!("expected solutions")
    };
    s.collect::<Result<Vec<_>, _>>().unwrap()
}

// @lat: [[tests#Store#Queries are single statements]]
#[test]
fn queries_are_single_statements() -> Result<()> {
    let backend = Counting {
        inner: RusqliteBackend::memory()?,
        requests: 0.into(),
    };
    let store = Store::with_backend(backend)?;
    for i in 0..10 {
        store.insert(QuadRef::new(
            &ex(&format!("s{i}")),
            &ex("p"),
            &ex("o"),
            GraphNameRef::DefaultGraph,
        ))?;
        store.insert(QuadRef::new(
            &ex(&format!("s{i}")),
            &ex("q"),
            &Literal::from(i),
            GraphNameRef::DefaultGraph,
        ))?;
    }
    let before = store
        .backend()
        .requests
        .load(std::sync::atomic::Ordering::SeqCst);
    let r = store.query(
        "SELECT ?s ?v WHERE { ?s <http://example.com/p> <http://example.com/o> ; <http://example.com/q> ?v . ?s ?p ?o . ?x <http://example.com/q> ?v FILTER(?v > 3) }",
    )?;
    assert_eq!(solutions(r).len(), 12);
    let after = store
        .backend()
        .requests
        .load(std::sync::atomic::Ordering::SeqCst);
    assert!(after - before <= 2, "{} requests", after - before);
    assert!(store
        .explain("SELECT * WHERE { ?s ?p ?o FILTER(?o > 3) }")?
        .contains("fully compiled"));
    Ok(())
}

// @lat: [[tests#Store#Collision aborts the batch]]
#[test]
fn collision_aborts_the_batch() -> Result<()> {
    let store = Store::new()?;
    let a = ex("a");
    // Forge a colliding dictionary entry: the id of <a> mapped to another IRI.
    let id = oxilite::core::encoding::named_node_id(a.as_str());
    store.backend().execute(&Request::atomic(vec![format!(
        "INSERT INTO terms(id, lex) VALUES ({id}, 'http://example.com/not-a')"
    )
    .into()]))?;
    let err = store
        .extend([
            Quad::new(ex("x"), ex("p"), ex("y"), GraphName::DefaultGraph),
            Quad::new(a.clone(), ex("p"), ex("y"), GraphName::DefaultGraph),
        ])
        .unwrap_err();
    assert!(matches!(err, oxilite::Error::Collision(_)), "{err}");
    assert!(store.is_empty()?, "the whole batch must be rolled back");
    Ok(())
}

// @lat: [[tests#Store#Reopen keeps data]]
#[test]
fn reopen_keeps_data() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("oxilite-reopen-{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    {
        let store = Store::open(&dir)?;
        store.insert(QuadRef::new(
            &ex("s"),
            &ex("p"),
            &Literal::from("v"),
            &ex("g"),
        ))?;
    }
    let store = Store::open(&dir)?;
    assert_eq!(store.len()?, 1);
    assert!(store.contains_named_graph(&ex("g"))?);
    store.validate()?;
    drop(store);
    let _ = std::fs::remove_file(&dir);
    Ok(())
}

// @lat: [[tests#Store#Graph index is optional]]
#[test]
fn graph_index_is_optional() -> Result<()> {
    let store = Store::with_backend_and_options(
        RusqliteBackend::memory()?,
        &StoreOptions { graph_index: false },
    )?;
    let r = store.backend().execute(&Request::read(vec![
        "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'quads' ORDER BY name"
            .into(),
    ]))?;
    let names: Vec<String> = r[0]
        .rows
        .iter()
        .filter_map(|r| r[0].as_str().map(String::from))
        .collect();
    assert_eq!(names, ["quads_ospg", "quads_posg"]);
    assert!(!store.stats().graph_index);
    Ok(())
}

// @lat: [[tests#Store#Insert and remove report changes]]
#[test]
fn insert_and_remove_report_changes() -> Result<()> {
    let store = Store::new()?;
    let q = Quad::new(
        ex("s"),
        ex("p"),
        Literal::new_typed_literal("012", vocab::xsd::INTEGER),
        GraphName::DefaultGraph,
    );
    assert!(store.insert(&q)?);
    assert!(!store.insert(&q)?);
    assert_eq!(store.len()?, 1);
    // The non-canonical lexical form round-trips exactly.
    assert_eq!(store.iter().next().unwrap()?, q);
    assert!(store.remove(&q)?);
    assert!(!store.remove(&q)?);
    Ok(())
}

// @lat: [[tests#Store#Pattern scans match a naive filter]]
#[test]
fn pattern_scans_match_a_naive_filter() -> Result<()> {
    let store = Store::new()?;
    let mut all = Vec::new();
    for s in ["a", "b"] {
        for p in ["p", "q"] {
            for o in ["x", "y"] {
                for g in [None, Some("g")] {
                    let q = Quad::new(
                        ex(s),
                        ex(p),
                        ex(o),
                        g.map_or(GraphName::DefaultGraph, |g| ex(g).into()),
                    );
                    store.insert(&q)?;
                    all.push(q);
                }
            }
        }
    }
    let seen = Cell::new(0);
    for mask in 0..16u8 {
        let t = &all[5];
        let s = (mask & 1 != 0).then(|| t.subject.as_ref());
        let p = (mask & 2 != 0).then(|| t.predicate.as_ref());
        let o = (mask & 4 != 0).then(|| t.object.as_ref());
        let g = (mask & 8 != 0).then(|| t.graph_name.as_ref());
        let mut got: Vec<Quad> = store.quads_for_pattern(s, p, o, g).collect::<Result<_>>()?;
        let mut want: Vec<Quad> = all
            .iter()
            .filter(|q| {
                s.is_none_or(|s| q.subject.as_ref() == s)
                    && p.is_none_or(|p| q.predicate.as_ref() == p)
                    && o.is_none_or(|o| q.object.as_ref() == o)
                    && g.is_none_or(|g| q.graph_name.as_ref() == g)
            })
            .cloned()
            .collect();
        got.sort_by_key(ToString::to_string);
        want.sort_by_key(ToString::to_string);
        assert_eq!(got, want, "mask {mask}");
        seen.set(seen.get() + 1);
    }
    assert_eq!(seen.get(), 16);
    Ok(())
}

// @lat: [[tests#Store#Planner uses statistics]]
#[test]
fn planner_uses_statistics() -> Result<()> {
    let store = Store::new()?;
    let mut quads = Vec::new();
    for i in 0..200 {
        quads.push(Quad::new(
            ex(&format!("i{i}")),
            vocab::rdf::TYPE,
            ex("Common"),
            GraphName::DefaultGraph,
        ));
    }
    for i in 0..3 {
        quads.push(Quad::new(
            ex(&format!("i{i}")),
            ex("rare"),
            Literal::from(i),
            GraphName::DefaultGraph,
        ));
    }
    store.extend(quads)?;
    store.optimize()?;
    let q =
        "SELECT ?x WHERE { ?x a <http://example.com/Common> . ?x <http://example.com/rare> ?y }";
    let sql = store.explain(q)?;
    let rare = oxilite::core::encoding::named_node_id("http://example.com/rare");
    let first_alias_cond = sql.split(" WHERE ").nth(1).unwrap();
    assert!(
        first_alias_cond.starts_with(&format!("q1.p = {rare}")),
        "{sql}"
    );
    assert_eq!(solutions(store.query(q)?).len(), 3);
    Ok(())
}

// @lat: [[tests#Store#Union default graph deduplicates]]
#[test]
fn union_default_graph_deduplicates() -> Result<()> {
    let store = Store::new()?;
    for g in ["g1", "g2"] {
        store.insert(QuadRef::new(&ex("s"), &ex("p"), &ex("o"), &ex(g)))?;
    }
    let r = store.query_opt(
        "SELECT ?s WHERE { ?s ?p ?o }",
        oxilite::QueryOptions {
            union_default_graph: true,
            ..Default::default()
        },
    )?;
    assert_eq!(solutions(r).len(), 1);
    let r = store.query("SELECT ?g WHERE { GRAPH ?g { ?s ?p ?o } }")?;
    assert_eq!(solutions(r).len(), 2);
    Ok(())
}

// @lat: [[tests#Store#Dylib store works end to end]]
#[test]
fn dylib_store_works_end_to_end() -> Result<()> {
    let Some(lib) = oxilite::dylib::find_system_library() else {
        eprintln!("no system SQLite, skipping");
        return Ok(());
    };
    let store = Store::open_with_library(lib, ":memory:")?;
    store.load_from_slice(
        oxilite::io::RdfFormat::Turtle,
        "@prefix ex: <http://example.com/> . ex:a ex:p 1, 2, 3 .",
    )?;
    let r = store.query("SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o FILTER(?o >= 2) }")?;
    let s = solutions(r);
    assert_eq!(s[0].get("c"), Some(&Literal::from(2).into()));
    Ok(())
}
