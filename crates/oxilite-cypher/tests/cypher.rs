//! Cypher over oxilite: the M7 specification scenarios, on the native store and on the D1
//! code path (rusqlite behind the async interface with D1's capabilities).

use futures::executor::block_on;
use oxilite::cypher::{CypherError, CypherOptions, CypherResult, Params, Value};
use oxilite::rusqlite::RusqliteBackend;
use oxilite::sparql::{QueryResults, Reasoning};
use oxilite::store::Store;
use oxilite::{AsyncBackend, AsyncStore, Capabilities};
use oxilite_core::{Request, Response, SyncBackend};
use oxrdfio::RdfFormat;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

struct D1Like(RusqliteBackend, Capabilities);

impl AsyncBackend for D1Like {
    async fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        self.0.execute(request)
    }
    fn capabilities(&self) -> &Capabilities {
        &self.1
    }
}

/// The Miniflare D1 sidecar (`testsuite/d1-sidecar`), when `OXILITE_D1_URL` is set.
struct HttpD1(String, Capabilities);

impl AsyncBackend for HttpD1 {
    async fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        let body = serde_json::to_value(request).map_err(oxilite_core::Error::backend)?;
        match ureq::post(&format!("{}/execute", self.0)).send_json(body) {
            Ok(r) => r.into_json().map_err(oxilite_core::Error::backend),
            Err(ureq::Error::Status(_, r)) => {
                let v: serde_json::Value = r.into_json().unwrap_or_default();
                Err(oxilite_core::Error::backend(
                    v["error"].as_str().unwrap_or("D1 error"),
                ))
            }
            Err(e) => Err(oxilite_core::Error::backend(e)),
        }
    }
    fn capabilities(&self) -> &Capabilities {
        &self.1
    }
}

/// The sidecar serves one database: tests using it run one at a time.
static SIDECAR: std::sync::Mutex<()> = std::sync::Mutex::new(());

enum Engine {
    Native(Store),
    Dylib(Store<oxilite::dylib::DylibBackend>),
    D1(AsyncStore<D1Like>),
    Miniflare(
        AsyncStore<HttpD1>,
        #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
    ),
}

impl Engine {
    fn all() -> Vec<Self> {
        let mut out = vec![
            Self::Native(Store::new().unwrap()),
            Self::D1(
                block_on(AsyncStore::open(D1Like(
                    RusqliteBackend::memory().unwrap(),
                    Capabilities::d1(),
                )))
                .unwrap(),
            ),
        ];
        // The platform SQLite (fixed parser stack): generated SQL must stay shallow.
        if let Some(lib) = oxilite::dylib::find_system_library() {
            out.push(Self::Dylib(
                Store::open_with_library(lib, ":memory:").unwrap(),
            ));
        }
        if let Ok(url) = std::env::var("OXILITE_D1_URL") {
            let guard = SIDECAR
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ureq::post(&format!("{url}/reset")).call().unwrap();
            let store = block_on(AsyncStore::open(HttpD1(url, Capabilities::d1()))).unwrap();
            out.push(Self::Miniflare(store, guard));
        }
        out
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Native(_) => "native",
            Self::Dylib(_) => "dylib",
            Self::D1(_) => "d1",
            Self::Miniflare(..) => "miniflare",
        }
    }

    fn run(
        &self,
        q: &str,
        params: &Params,
        opts: &CypherOptions,
    ) -> Result<CypherResult, CypherError> {
        match self {
            Self::Native(s) => s.cypher_with(q, params, opts),
            Self::Dylib(s) => s.cypher_with(q, params, opts),
            Self::D1(s) => block_on(s.cypher_with(q, params, opts)),
            Self::Miniflare(s, _) => block_on(s.cypher_with(q, params, opts)),
        }
    }

    fn cypher(&self, q: &str) -> CypherResult {
        self.run(q, &Params::new(), &CypherOptions::default())
            .unwrap_or_else(|e| panic!("[{}] {q}: {e}", self.name()))
    }

    fn load(&self, turtle: &str) {
        self.load_as(RdfFormat::Turtle, turtle);
    }

    fn load_as(&self, format: RdfFormat, data: &str) {
        match self {
            Self::Native(s) => s.load_from_slice(format, data.as_bytes()).unwrap(),
            Self::Dylib(s) => s.load_from_slice(format, data.as_bytes()).unwrap(),
            Self::D1(s) => block_on(s.load_from_slice(format, data.as_bytes())).unwrap(),
            Self::Miniflare(s, _) => block_on(s.load_from_slice(format, data.as_bytes())).unwrap(),
        }
    }

    fn ask(&self, q: &str) -> bool {
        let r = match self {
            Self::Native(s) => s.query(q).unwrap(),
            Self::Dylib(s) => s.query(q).unwrap(),
            Self::D1(s) => block_on(s.query(q)).unwrap(),
            Self::Miniflare(s, _) => block_on(s.query(q)).unwrap(),
        };
        match r {
            QueryResults::Boolean(b) => b,
            _ => panic!("not an ASK"),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Native(s) => s.len().unwrap(),
            Self::Dylib(s) => s.len().unwrap(),
            Self::D1(s) => block_on(s.len()).unwrap(),
            Self::Miniflare(s, _) => block_on(s.len()).unwrap(),
        }
    }
}

fn s(x: &str) -> Value {
    Value::String(x.into())
}

fn i(x: i64) -> Value {
    Value::Int(x)
}

fn col(r: &CypherResult, c: usize) -> Vec<Value> {
    r.rows.iter().map(|row| row[c].clone()).collect()
}

const EX: &str = "http://example.com/";

fn ex_opts() -> CypherOptions {
    CypherOptions {
        vocabulary: oxilite::cypher::Vocabulary::new(EX),
        ..CypherOptions::default()
    }
}

// ----- property-graph mapping -----

// @lat: [[tests#Cypher#Cypher nodes are visible to SPARQL]]
#[test]
fn cypher_node_visible_to_sparql() {
    for e in Engine::all() {
        e.run("CREATE (:Person {name: 'Ada'})", &Params::new(), &ex_opts())
            .unwrap();
        assert!(
            e.ask("ASK { ?n a <http://example.com/Person> ; <http://example.com/name> \"Ada\" }"),
            "{}",
            e.name()
        );
    }
}

// @lat: [[tests#Cypher#RDF data is visible to Cypher]]
#[test]
fn rdf_visible_to_cypher() {
    for e in Engine::all() {
        e.load("@prefix ex: <http://example.com/> . ex:bob a ex:Person ; ex:age 42 ; ex:knows ex:carol . ex:carol ex:name \"Carol\" .");
        let r = e
            .run(
                "MATCH (p:Person)-[:knows]->(f) RETURN p.age, f.name",
                &Params::new(),
                &ex_opts(),
            )
            .unwrap();
        assert_eq!(r.rows, vec![vec![i(42), s("Carol")]], "{}", e.name());
    }
}

// @lat: [[tests#Cypher#A plain relationship is one triple]]
#[test]
fn plain_relationship_one_quad() {
    for e in Engine::all() {
        e.cypher("CREATE (:A {id: 1}), (:A {id: 2})");
        let before = e.len();
        let r = e.cypher("MATCH (a:A {id: 1}), (b:A {id: 2}) CREATE (a)-[:KNOWS]->(b)");
        assert_eq!(r.stats.relationships_created, 1);
        assert_eq!(e.len(), before + 1, "{}", e.name());
    }
}

// @lat: [[tests#Cypher#Relationship properties live on a reifier]]
#[test]
fn relationship_property_round_trip() {
    for e in Engine::all() {
        e.run(
            "CREATE (:P {n: 'a'})-[:KNOWS {since: 2020}]->(:P {n: 'b'})",
            &Params::new(),
            &ex_opts(),
        )
        .unwrap();
        let r = e
            .run(
                "MATCH (a)-[r:KNOWS]->(b) RETURN r.since",
                &Params::new(),
                &ex_opts(),
            )
            .unwrap();
        assert_eq!(col(&r, 0), vec![i(2020)], "{}", e.name());
        assert!(e.ask(
            "PREFIX ex: <http://example.com/> PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> \
             ASK { ?a ex:KNOWS ?b . ?r rdf:reifies <<( ?a ex:KNOWS ?b )>> ; ex:since 2020 }"
        ));
    }
}

// @lat: [[tests#Cypher#Parallel relationships stay distinct]]
#[test]
fn parallel_relationships() {
    for e in Engine::all() {
        e.cypher("CREATE (:P {n: 1}), (:P {n: 2})");
        e.cypher("MATCH (a:P {n: 1}), (b:P {n: 2}) CREATE (a)-[:KNOWS {since: 2001}]->(b)");
        e.cypher("MATCH (a:P {n: 1}), (b:P {n: 2}) CREATE (a)-[:KNOWS {since: 2002}]->(b)");
        let r = e.cypher("MATCH (a)-[r:KNOWS]->(b) RETURN count(r)");
        assert_eq!(col(&r, 0), vec![i(2)], "{}", e.name());
        let r = e.cypher("MATCH ()-[r:KNOWS]->() RETURN r.since ORDER BY r.since");
        assert_eq!(col(&r, 0), vec![i(2001), i(2002)], "{}", e.name());
        // A plain third relationship: the existing ones keep their reifiers.
        e.cypher("MATCH (a:P {n: 1}), (b:P {n: 2}) CREATE (a)-[:KNOWS]->(b)");
        let r = e.cypher("MATCH (a)-[r:KNOWS]->(b) RETURN count(r)");
        assert_eq!(col(&r, 0), vec![i(3)], "{}", e.name());
        // Deleting one keeps the asserted triple for the others.
        e.cypher("MATCH ()-[r:KNOWS {since: 2001}]->() DELETE r");
        let r = e.cypher("MATCH (a)-[r:KNOWS]->(b) RETURN count(r)");
        assert_eq!(col(&r, 0), vec![i(2)], "{}", e.name());
        let r = e.cypher("MATCH (a)-[:KNOWS]->(b) RETURN count(*)");
        assert_eq!(col(&r, 0), vec![i(1)], "{} asserted triple", e.name());
    }
}

// @lat: [[tests#Cypher#Plain relationship becomes parallel]]
#[test]
fn plain_then_parallel() {
    for e in Engine::all() {
        e.cypher("CREATE (:P {n: 1})-[:T]->(:P {n: 2})");
        e.cypher("MATCH (a:P {n: 1}), (b:P {n: 2}) CREATE (a)-[:T {w: 5}]->(b)");
        let r = e.cypher("MATCH ()-[r:T]->() RETURN r.w ORDER BY r.w");
        assert_eq!(col(&r, 0), vec![i(5), Value::Null], "{}", e.name());
        e.cypher("MATCH ()-[r:T]->() WHERE r.w IS NULL DELETE r");
        let r = e.cypher("MATCH ()-[r:T]->() RETURN r.w");
        assert_eq!(col(&r, 0), vec![i(5)], "{}", e.name());
    }
}

// @lat: [[tests#Cypher#Multi-valued properties read as lists]]
#[test]
fn multi_valued_list() {
    for e in Engine::all() {
        e.load("@prefix ex: <http://example.com/> . ex:n a ex:Item ; ex:tag \"b\", \"a\" .");
        let r = e
            .run("MATCH (n:Item) RETURN n.tag", &Params::new(), &ex_opts())
            .unwrap();
        assert_eq!(
            col(&r, 0),
            vec![Value::List(vec![s("a"), s("b")])],
            "{}",
            e.name()
        );
        let r = e
            .run(
                "MATCH (n:Item) WHERE n.tag = 'a' RETURN count(*)",
                &Params::new(),
                &ex_opts(),
            )
            .unwrap();
        assert_eq!(col(&r, 0), vec![i(1)]);
    }
}

// @lat: [[tests#Cypher#List properties round-trip as rdf:JSON]]
#[test]
fn list_property() {
    for e in Engine::all() {
        e.cypher("CREATE (:L {xs: [1, 2, 3], m: {a: 'x'}})");
        let r = e.cypher("MATCH (n:L) RETURN n.xs, n.m.a, size(n.xs)");
        assert_eq!(
            r.rows,
            vec![vec![Value::List(vec![i(1), i(2), i(3)]), s("x"), i(3)]],
            "{}",
            e.name()
        );
    }
}

// ----- reads -----

fn social(e: &Engine) {
    e.cypher(
        "CREATE (ada:Person {name: 'Ada', age: 36}), (alan:Person {name: 'Alan', age: 41}), \
         (grace:Person {name: 'Grace', age: 85}), (linus:Person {name: 'Linus'}), \
         (ada)-[:KNOWS]->(alan), (ada)-[:KNOWS]->(grace), (alan)-[:KNOWS]->(grace), \
         (grace)-[:KNOWS]->(ada), (linus)-[:KNOWS]->(ada), (ada)-[:OWNS]->(:Car {make: 'Ford'})",
    );
}

// @lat: [[tests#Cypher#WITH pipelines aggregation]]
#[test]
fn with_pipelines_aggregation() {
    for e in Engine::all() {
        social(&e);
        let r = e.cypher(
            "MATCH (p:Person)-[:KNOWS]->(f) WITH p, count(f) AS n WHERE n > 1 RETURN p.name ORDER BY p.name",
        );
        assert_eq!(col(&r, 0), vec![s("Ada")], "{}", e.name());
        let r =
            e.cypher("MATCH (p:Person) RETURN p.name AS name ORDER BY p.age DESC SKIP 1 LIMIT 2");
        assert_eq!(
            col(&r, 0),
            vec![s("Grace"), s("Alan")],
            "{} (nulls first when descending)",
            e.name()
        );
        let r = e.cypher("MATCH (p:Person) RETURN p.name ORDER BY p.age");
        assert_eq!(
            col(&r, 0).last(),
            Some(&s("Linus")),
            "{} nulls last",
            e.name()
        );
    }
}

// @lat: [[tests#Cypher#OPTIONAL MATCH yields null]]
#[test]
fn optional_match_null() {
    for e in Engine::all() {
        social(&e);
        let r = e.cypher("MATCH (p:Person) OPTIONAL MATCH (p)-[:OWNS]->(c) RETURN p.name, c.make ORDER BY p.name");
        assert_eq!(
            r.rows,
            vec![
                vec![s("Ada"), s("Ford")],
                vec![s("Alan"), Value::Null],
                vec![s("Grace"), Value::Null],
                vec![s("Linus"), Value::Null],
            ],
            "{}",
            e.name()
        );
        // A null node from OPTIONAL MATCH never matches later patterns.
        let r = e.cypher(
            "MATCH (p:Person {name: 'Alan'}) OPTIONAL MATCH (p)-[:OWNS]->(c) OPTIONAL MATCH (c)-[:KNOWS]->(x) RETURN count(x)",
        );
        assert_eq!(col(&r, 0), vec![i(0)], "{}", e.name());
    }
}

/// Counts requests sent to the backend.
struct Counting(RusqliteBackend, AtomicUsize);

impl SyncBackend for Counting {
    fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.execute(request)
    }
    fn capabilities(&self) -> &Capabilities {
        self.0.capabilities()
    }
    fn begin(&self) -> oxilite_core::Result<()> {
        self.0.begin()
    }
    fn commit(&self) -> oxilite_core::Result<()> {
        self.0.commit()
    }
    fn rollback(&self) -> oxilite_core::Result<()> {
        self.0.rollback()
    }
}

// @lat: [[tests#Cypher#Reads are one SQL statement]]
#[test]
fn single_statement() {
    let store = Store::with_backend(Counting(
        RusqliteBackend::memory().unwrap(),
        AtomicUsize::new(0),
    ))
    .unwrap();
    store
        .cypher("CREATE (:A {n: 1})-[:R]->(:B {n: 2})-[:R]->(:C {n: 3})-[:R]->(:D {n: 4})")
        .unwrap();
    store.backend().1.store(0, Ordering::SeqCst);
    let r = store
        .cypher("MATCH (a:A)-[:R]->(b)-[:R]->(c)-[:R]->(d) RETURN d.n")
        .unwrap();
    assert_eq!(col(&r, 0), vec![i(4)]);
    // The query, its term resolution, and the node materialization.
    let n = store.backend().1.load(Ordering::SeqCst);
    assert!(n <= 3, "{n} requests");
    let x = store
        .explain_cypher(
            "MATCH (a:A)-[:R]->(b)-[:R]->(c)-[:R]->(d) RETURN d.n",
            &Params::new(),
            &CypherOptions::default(),
        )
        .unwrap();
    assert!(x.contains("fully compiled to SQL"), "{x}");
}

// @lat: [[tests#Cypher#Relationships are not reused in one MATCH]]
#[test]
fn relationship_uniqueness() {
    for e in Engine::all() {
        e.cypher("CREATE (:X)-[:R]->(:Y)");
        let r = e.cypher("MATCH (a)-[:R]-(b)-[:R]-(c) RETURN count(*)");
        assert_eq!(col(&r, 0), vec![i(0)], "{}", e.name());
        let r = e.cypher("MATCH (a)-[:R]-(b) RETURN count(*)");
        assert_eq!(col(&r, 0), vec![i(2)], "{} both directions", e.name());
    }
}

// @lat: [[tests#Cypher#Variable-length paths follow trails]]
#[test]
fn var_length_cycle() {
    for e in Engine::all() {
        e.cypher("CREATE (a:N {id: 1})-[:NEXT]->(b:N {id: 2})-[:NEXT]->(c:N {id: 3})-[:NEXT]->(a)");
        let r = e.cypher("MATCH (a:N {id: 1})-[:NEXT*1..5]->(b) RETURN b.id ORDER BY b.id");
        // Trails of length 1, 2, 3 (back to a); a fourth hop would reuse a relationship.
        assert_eq!(col(&r, 0), vec![i(1), i(2), i(3)], "{}", e.name());
        let r = e.cypher(
            "MATCH p = (a:N {id: 1})-[:NEXT*2]->(b) RETURN length(p), [n IN nodes(p) | n.id]",
        );
        assert_eq!(
            r.rows,
            vec![vec![i(2), Value::List(vec![i(1), i(2), i(3)])]],
            "{}",
            e.name()
        );
        let r =
            e.cypher("MATCH (a:N {id: 1})-[rs:NEXT*..2]->(b) RETURN size(rs) ORDER BY size(rs)");
        assert_eq!(col(&r, 0), vec![i(1), i(2)], "{}", e.name());
    }
}

// @lat: [[tests#Cypher#Shortest paths run on D1]]
#[test]
fn shortest_path() {
    for e in Engine::all() {
        e.cypher(
            "CREATE (a:S {id: 1})-[:R]->(b:S {id: 2})-[:R]->(c:S {id: 3})-[:R]->(d:S {id: 4}), (a)-[:R]->(x:S {id: 5})-[:R]->(d)",
        );
        let r =
            e.cypher("MATCH p = shortestPath((a:S {id: 1})-[:R*]-(b:S {id: 4})) RETURN length(p)");
        assert_eq!(col(&r, 0), vec![i(2)], "{}", e.name());
        let r = e.cypher(
            "MATCH p = allShortestPaths((a:S {id: 1})-[:R*..5]->(b:S {id: 3})) RETURN length(p)",
        );
        assert_eq!(col(&r, 0), vec![i(2)], "{}", e.name());
    }
}

// @lat: [[tests#Cypher#Nodes, relationships and paths are values]]
#[test]
fn entity_values() {
    for e in Engine::all() {
        e.cypher("CREATE (:Person:Admin {name: 'Ada'})-[:KNOWS {w: 1}]->(:Person {name: 'Alan'})");
        let r = e.cypher("MATCH (n:Person {name: 'Ada'})-[k]->(m) RETURN n, k, labels(n), type(k), keys(n), m {.name}");
        let Value::Node(n) = &r.rows[0][0] else {
            panic!("{:?}", r.rows)
        };
        assert_eq!(n.labels.len(), 2, "{}", e.name());
        assert_eq!(n.properties.get("name"), Some(&s("Ada")));
        let Value::Relationship(k) = &r.rows[0][1] else {
            panic!()
        };
        assert_eq!(k.rel_type, "KNOWS");
        assert_eq!(k.properties.get("w"), Some(&i(1)));
        assert_eq!(r.rows[0][3], s("KNOWS"));
        assert_eq!(r.rows[0][4], Value::List(vec![s("name")]));
        assert_eq!(
            r.rows[0][5],
            Value::Map(BTreeMap::from([("name".into(), s("Alan"))]))
        );
    }
}

// @lat: [[tests#Cypher#Explain shows the SQL]]
#[test]
fn explain_join_order() {
    let store = Store::new().unwrap();
    let x = store
        .explain_cypher(
            "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a, b",
            &Params::new(),
            &CypherOptions::default(),
        )
        .unwrap();
    assert!(x.contains("join order"), "{x}");
    assert!(x.contains("SPARQL"), "{x}");
}

// @lat: [[tests#Cypher#Read clauses and functions]]
#[test]
fn read_features() {
    for e in Engine::all() {
        social(&e);
        let cases: Vec<(&str, Vec<Vec<Value>>)> = vec![
            ("MATCH (p:Person) WHERE p.name STARTS WITH 'A' RETURN p.name ORDER BY p.name", vec![vec![s("Ada")], vec![s("Alan")]]),
            ("MATCH (p:Person) WHERE p.name IN ['Ada', 'Linus'] RETURN count(*)", vec![vec![i(2)]]),
            ("MATCH (p:Person) WHERE p.age IS NULL RETURN p.name", vec![vec![s("Linus")]]),
            ("MATCH (p:Person) WHERE (p)-[:OWNS]->() RETURN p.name", vec![vec![s("Ada")]]),
            ("MATCH (p:Person) WHERE NOT (p)<-[:KNOWS]-() RETURN p.name", vec![vec![s("Linus")]]),
            ("MATCH (p:Person) WHERE exists { MATCH (p)-[:KNOWS]->(q) WHERE q.age > 80 } RETURN p.name ORDER BY p.name", vec![vec![s("Ada")], vec![s("Alan")]]),
            ("MATCH (p:Person) RETURN collect(p.name) AS names", vec![]),
            ("MATCH (p:Person {name: 'Ada'})-[:KNOWS]->(f) RETURN p.name, collect(f.name) AS fs", vec![]),
            ("UNWIND [3, 1, 2] AS x RETURN x ORDER BY x", vec![vec![i(1)], vec![i(2)], vec![i(3)]]),
            ("UNWIND range(1, 3) AS x RETURN sum(x)", vec![vec![i(6)]]),
            ("MATCH (p:Person) RETURN CASE WHEN p.age > 50 THEN 'old' ELSE 'young' END AS c, count(*) ORDER BY c", vec![vec![s("old"), i(1)], vec![s("young"), i(3)]]),
            ("MATCH (p:Person) RETURN DISTINCT size(p.name) AS l ORDER BY l", vec![vec![i(3)], vec![i(4)], vec![i(5)]]),
            ("MATCH (p:Person {name: 'Ada'}) RETURN p.age / 5, p.age % 5, p.age * 1.5, toString(p.age) + '!'", vec![vec![i(7), i(1), Value::Float(54.0), s("36!")]]),
            ("MATCH (a:Person {name: 'Ada'}) RETURN a.name AS n UNION MATCH (b:Person {name: 'Ada'}) RETURN b.name AS n", vec![vec![s("Ada")]]),
            ("MATCH (a:Person {name: 'Ada'}) RETURN a.name AS n UNION ALL MATCH (b:Person {name: 'Ada'}) RETURN b.name AS n", vec![vec![s("Ada")], vec![s("Ada")]]),
            ("MATCH (p:Person) WITH p ORDER BY p.name LIMIT 2 RETURN p.name", vec![vec![s("Ada")], vec![s("Alan")]]),
            ("MATCH (p:Person) RETURN min(p.age), max(p.age), avg(p.age)", vec![vec![i(36), i(85), Value::Float(54.0)]]),
            ("MATCH (p:Person {name: 'Linus'}) RETURN avg(p.age)", vec![vec![Value::Null]]),
            ("MATCH (p:Person)-[:KNOWS|OWNS]->(x) WHERE p.name = 'Ada' RETURN count(x)", vec![vec![i(3)]]),
            ("MATCH (a {name: 'Ada'})-[r]->(b) RETURN type(r), count(*) ORDER BY type(r)", vec![vec![s("KNOWS"), i(2)], vec![s("OWNS"), i(1)]]),
        ];
        for (q, expected) in cases {
            let r = e.cypher(q);
            if !expected.is_empty() {
                assert_eq!(r.rows, expected, "[{}] {q}", e.name());
            }
        }
        let r = e.cypher("MATCH (p:Person) RETURN collect(p.name) AS names");
        let Value::List(mut names) = r.rows[0][0].clone() else {
            panic!()
        };
        names.sort_by(|a, b| a.order(b));
        assert_eq!(names, vec![s("Ada"), s("Alan"), s("Grace"), s("Linus")]);
    }
}

// @lat: [[tests#Cypher#Parameters]]
#[test]
fn parameters() {
    for e in Engine::all() {
        let mut params = Params::new();
        params.insert(
            "rows".into(),
            Value::List(vec![
                Value::Map(BTreeMap::from([
                    ("name".into(), s("a")),
                    ("n".into(), i(1)),
                ])),
                Value::Map(BTreeMap::from([
                    ("name".into(), s("b")),
                    ("n".into(), i(2)),
                ])),
            ]),
        );
        e.run(
            "UNWIND $rows AS row CREATE (:R {name: row.name, n: row.n})",
            &params,
            &CypherOptions::default(),
        )
        .unwrap();
        params.insert("min".into(), i(2));
        let r = e
            .run(
                "MATCH (r:R) WHERE r.n >= $min RETURN r.name",
                &params,
                &CypherOptions::default(),
            )
            .unwrap();
        assert_eq!(col(&r, 0), vec![s("b")], "{}", e.name());
        params.insert(
            "props".into(),
            Value::Map(BTreeMap::from([("name".into(), s("a"))])),
        );
        let r = e
            .run(
                "MATCH (r:R $props) RETURN r.n",
                &params,
                &CypherOptions::default(),
            )
            .unwrap();
        assert_eq!(col(&r, 0), vec![i(1)], "{}", e.name());
    }
}

// ----- writes -----

// @lat: [[tests#Cypher#MERGE is idempotent]]
#[test]
fn merge_idempotent() {
    for e in Engine::all() {
        for _ in 0..2 {
            e.cypher("MERGE (p:Person {name: 'Ada'}) ON CREATE SET p.created = true ON MATCH SET p.seen = true");
        }
        let r = e.cypher("MATCH (p:Person {name: 'Ada'}) RETURN count(p), p.created, p.seen");
        assert_eq!(
            r.rows,
            vec![vec![i(1), Value::Bool(true), Value::Bool(true)]],
            "{}",
            e.name()
        );
        // Within one statement, a later row matches what an earlier row created.
        e.cypher("UNWIND ['x', 'x', 'y'] AS n MERGE (:Tag {n: n})");
        let r = e.cypher("MATCH (t:Tag) RETURN count(t)");
        assert_eq!(col(&r, 0), vec![i(2)], "{}", e.name());
        e.cypher("MATCH (a:Person {name: 'Ada'}), (t:Tag {n: 'x'}) MERGE (a)-[:TAGGED]->(t)");
        e.cypher("MATCH (a:Person {name: 'Ada'}), (t:Tag {n: 'x'}) MERGE (a)-[:TAGGED]->(t)");
        let r = e.cypher("MATCH ()-[r:TAGGED]->() RETURN count(r)");
        assert_eq!(col(&r, 0), vec![i(1)], "{}", e.name());
    }
}

// @lat: [[tests#Cypher#SET and REMOVE]]
#[test]
fn set_remove() {
    for e in Engine::all() {
        e.cypher("CREATE (:P {name: 'a', x: 1})");
        e.cypher("MATCH (p:P) SET p.x = p.x + 1, p.y = 'y', p:Extra");
        let r = e.cypher("MATCH (p:Extra) RETURN p.x, p.y");
        assert_eq!(r.rows, vec![vec![i(2), s("y")]], "{}", e.name());
        e.cypher("MATCH (p:P) SET p += {z: 3, x: null}");
        let r = e.cypher("MATCH (p:P) RETURN p.x, p.z");
        assert_eq!(r.rows, vec![vec![Value::Null, i(3)]], "{}", e.name());
        e.cypher("MATCH (p:P) REMOVE p.z, p:Extra");
        let r = e.cypher("MATCH (p:P) RETURN labels(p), keys(p)");
        assert_eq!(
            r.rows,
            vec![vec![
                Value::List(vec![s("P")]),
                Value::List(vec![s("name"), s("y")])
            ]],
            "{}",
            e.name()
        );
        e.cypher("MATCH (p:P) SET p = {only: 1}");
        let r = e.cypher("MATCH (p:P) RETURN properties(p)");
        assert_eq!(
            r.rows,
            vec![vec![Value::Map(BTreeMap::from([("only".into(), i(1))]))]],
            "{}",
            e.name()
        );
        let r = e.cypher("MATCH (p:P) SET p.v = 7 RETURN p.v");
        assert_eq!(col(&r, 0), vec![i(7)], "{} RETURN sees the write", e.name());
    }
}

// @lat: [[tests#Cypher#DETACH DELETE removes relationships]]
#[test]
fn detach_delete() {
    for e in Engine::all() {
        let empty = e.len();
        e.cypher("CREATE (a:P {name: 'Ada'})-[:KNOWS {since: 1}]->(b:P {name: 'B'}), (b)-[:KNOWS]->(a), (c:P)-[:KNOWS]->(a)");
        e.cypher("MATCH (n {name: 'Ada'}) DETACH DELETE n");
        let r = e.cypher("MATCH ()-[r]->() RETURN count(r)");
        assert_eq!(col(&r, 0), vec![i(0)], "{}", e.name());
        let r = e.cypher("MATCH (n:P) RETURN count(n)");
        assert_eq!(col(&r, 0), vec![i(2)], "{}", e.name());
        assert!(!e.ask("PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> ASK { ?r rdf:reifies ?t }"), "{}", e.name());
        e.cypher("MATCH (n) DETACH DELETE n");
        assert_eq!(e.len(), empty, "{}", e.name());
    }
}

// @lat: [[tests#Cypher#Deleting a connected node fails atomically]]
#[test]
fn delete_connected_fails() {
    for e in Engine::all() {
        e.cypher("CREATE (:P {name: 'Ada', age: 35})-[:KNOWS]->(:P {name: 'B'})");
        let err = e
            .run(
                "MATCH (n {name: 'Ada'}) SET n.age = 36 DELETE n",
                &Params::new(),
                &CypherOptions::default(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("relationships"), "{err}");
        let r = e.cypher("MATCH (n {name: 'Ada'}) RETURN n.age");
        assert_eq!(col(&r, 0), vec![i(35)], "{}", e.name());
    }
}

// @lat: [[tests#Cypher#Per-row creation on D1]]
#[test]
fn per_row_creation() {
    for e in Engine::all() {
        let r = e.cypher("UNWIND range(1, 100) AS i CREATE (:Item {i: i})");
        assert_eq!(r.stats.nodes_created, 100);
        let r = e.cypher("MATCH (n:Item) RETURN count(DISTINCT n), sum(n.i)");
        assert_eq!(r.rows, vec![vec![i(100), i(5050)]], "{}", e.name());
    }
}

// ----- ontology and shapes -----

// @lat: [[tests#Cypher#Labels follow the class hierarchy]]
#[test]
fn label_hierarchy() {
    for e in Engine::all() {
        e.load("@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . ex:Employee rdfs:subClassOf ex:Person . ex:ada a ex:Employee .");
        let q = "MATCH (p:Person) RETURN count(p)";
        let r = e.run(q, &Params::new(), &ex_opts()).unwrap();
        assert_eq!(col(&r, 0), vec![i(0)], "{}", e.name());
        let mut opts = ex_opts();
        opts.query.reasoning = Reasoning::Rdfs;
        let r = e.run(q, &Params::new(), &opts).unwrap();
        assert_eq!(col(&r, 0), vec![i(1)], "{}", e.name());
    }
}

// @lat: [[tests#Cypher#Inverse relationship types]]
#[test]
fn inverse_types() {
    for e in Engine::all() {
        e.load("@prefix ex: <http://example.com/> . @prefix owl: <http://www.w3.org/2002/07/owl#> . ex:hasParent owl:inverseOf ex:hasChild . ex:p ex:hasChild ex:c .");
        let mut opts = ex_opts();
        opts.query.reasoning = Reasoning::OwlQl;
        let r = e
            .run(
                "MATCH (c)-[:hasParent]->(p) RETURN count(*)",
                &Params::new(),
                &opts,
            )
            .unwrap();
        assert_eq!(col(&r, 0), vec![i(1)], "{}", e.name());
    }
}

const SHAPES: &str = "@prefix ex: <http://example.com/> . @prefix sh: <http://www.w3.org/ns/shacl#> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:property [ sh:path ex:name ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
  sh:property [ sh:path ex:age ; sh:datatype xsd:integer ; sh:maxCount 1 ] ;
  sh:property [ sh:path ex:status ; sh:in ( \"active\" \"retired\" ) ] .";

// @lat: [[tests#Cypher#Required properties join without OPTIONAL]]
#[test]
fn required_property_inner_join() {
    let store = Store::new().unwrap();
    store
        .load_from_slice(RdfFormat::Turtle, SHAPES.as_bytes())
        .unwrap();
    let mut opts = ex_opts();
    opts.schema = Some(store.cypher_schema().unwrap());
    let x = store
        .explain_cypher(
            "MATCH (p:Person) RETURN p ORDER BY p.name",
            &Params::new(),
            &opts,
        )
        .unwrap();
    assert!(!x.contains("LEFT JOIN"), "{x}");
    let x = store
        .explain_cypher(
            "MATCH (p:Person) RETURN p ORDER BY p.nick",
            &Params::new(),
            &opts,
        )
        .unwrap();
    assert!(x.contains("LEFT JOIN"), "{x}");
}

// @lat: [[tests#Cypher#SHACL guards abort invalid writes]]
#[test]
fn shacl_guards() {
    for e in Engine::all() {
        e.load(SHAPES);
        let before = e.len();
        for bad in [
            "CREATE (:Person {name: 'Ada', age: 'old'})",
            "CREATE (:Person {age: 3})",
            "CREATE (:Person {name: 'Ada', status: 'gone'})",
        ] {
            let err = e.run(bad, &Params::new(), &ex_opts()).unwrap_err();
            assert!(
                matches!(err, CypherError::ShapeViolation(_)),
                "[{}] {bad}: {err}",
                e.name()
            );
        }
        assert_eq!(e.len(), before, "{}", e.name());
        e.run(
            "CREATE (:Person {name: 'Ada', age: 36, status: 'active'})",
            &Params::new(),
            &ex_opts(),
        )
        .unwrap();
        let err = e
            .run(
                "MATCH (p:Person) SET p.age = 'x'",
                &Params::new(),
                &ex_opts(),
            )
            .unwrap_err();
        assert!(matches!(err, CypherError::ShapeViolation(_)), "{err}");
    }
}

// @lat: [[tests#Cypher#Schema procedures]]
#[test]
fn procedures() {
    for e in Engine::all() {
        e.load(SHAPES);
        e.run(
            "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Robot {name: 'R2'})",
            &Params::new(),
            &ex_opts(),
        )
        .unwrap();
        let r = e
            .run("CALL db.labels()", &Params::new(), &ex_opts())
            .unwrap();
        assert!(
            col(&r, 0).contains(&s("Person")) && col(&r, 0).contains(&s("Robot")),
            "{:?}",
            r.rows
        );
        let r = e
            .run(
                "CALL db.relationshipTypes() YIELD relationshipType RETURN relationshipType",
                &Params::new(),
                &ex_opts(),
            )
            .unwrap();
        assert_eq!(col(&r, 0), vec![s("KNOWS")], "{}", e.name());
        let r = e
            .run("CALL db.schema.nodeTypeProperties() YIELD propertyName, mandatory WHERE mandatory RETURN propertyName", &Params::new(), &ex_opts())
            .unwrap();
        assert_eq!(col(&r, 0), vec![s("name")], "{}", e.name());
    }
}

// @lat: [[tests#Cypher#Temporal values are stored as XSD literals]]
#[test]
fn temporal_values() {
    for e in Engine::all() {
        e.run(
            "CREATE (:Event {on: date('2020-01-02'), at: datetime('2015-07-21T21:40:32.142[Europe/London]'), for: duration({hours: 36})})",
            &Params::new(),
            &ex_opts(),
        )
        .unwrap();
        assert!(
            e.ask(
                "PREFIX ex: <http://example.com/> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> \
             ASK { ?e ex:on \"2020-01-02\"^^xsd:date ; ex:for \"PT36H\"^^xsd:duration }"
            ),
            "{}",
            e.name()
        );
        let r = e
            .run(
                "MATCH (e:Event) RETURN e.at, e.on + duration({days: 30}), e.on.year, date({year: 1817, week: 1}), duration.between(e.on, date('2020-03-01')).days",
                &Params::new(),
                &ex_opts(),
            )
            .unwrap();
        let t = |k, v: &str| Value::Temporal(k, v.into());
        use oxilite::cypher::TemporalKind::*;
        assert_eq!(
            r.rows,
            vec![vec![
                t(DateTime, "2015-07-21T21:40:32.142+01:00[Europe/London]"),
                t(Date, "2020-02-01"),
                i(2020),
                t(Date, "1816-12-30"),
                i(28),
            ]],
            "{}",
            e.name()
        );
    }
}

// @lat: [[tests#Cypher#Schema datatypes type comparisons]]
#[test]
fn schema_datatypes_type_comparisons() {
    let store = Store::new().unwrap();
    store
        .load_from_slice(RdfFormat::Turtle, SHAPES.as_bytes())
        .unwrap();
    let mut typed = ex_opts();
    typed.schema = Some(store.cypher_schema().unwrap());
    store
        .cypher_with(
            "CREATE (:Person {name: 'Ada', age: 36}), (:Person {name: 'Bob', age: 25})",
            &Params::new(),
            &typed,
        )
        .unwrap();
    let q = "MATCH (p:Person) WHERE p.age > 30 RETURN p.name";
    let sql = |o: &CypherOptions| store.explain_cypher(q, &Params::new(), o).unwrap();
    let (plain, with) = (sql(&ex_opts()), sql(&typed));
    // Without the schema the comparison tries every value type; with it, one numeric one.
    assert!(
        plain.matches("WHEN").count() > with.matches("WHEN").count(),
        "{plain}\n----\n{with}"
    );
    for o in [&ex_opts(), &typed] {
        let r = store.cypher_with(q, &Params::new(), o).unwrap();
        assert_eq!(col(&r, 0), vec![s("Ada")]);
    }
}

// @lat: [[tests#Cypher#Pattern comprehensions]]
#[test]
fn pattern_comprehensions() {
    for e in Engine::all() {
        social(&e);
        let r = e.cypher(
            "MATCH (p:Person) RETURN p.name AS name, [(p)-[:KNOWS]->(f) WHERE f.age > 40 | f.name] AS old ORDER BY name",
        );
        let sorted = |v: &Value| match v {
            Value::List(l) => {
                let mut l = l.clone();
                l.sort_by(|a, b| a.order(b));
                Value::List(l)
            }
            o => o.clone(),
        };
        let got: Vec<(Value, Value)> = r
            .rows
            .iter()
            .map(|row| (row[0].clone(), sorted(&row[1])))
            .collect();
        assert_eq!(
            got,
            vec![
                (s("Ada"), Value::List(vec![s("Alan"), s("Grace")])),
                (s("Alan"), Value::List(vec![s("Grace")])),
                (s("Grace"), Value::List(vec![])),
                (s("Linus"), Value::List(vec![])),
            ],
            "{}",
            e.name()
        );
        let r = e.cypher("MATCH (p:Person {name: 'Ada'}) RETURN size([q = (p)-->() | length(q)]) AS n, [(p)-[:OWNS]->(c) | c.make] AS cars");
        assert_eq!(
            r.rows,
            vec![vec![i(3), Value::List(vec![s("Ford")])]],
            "{}",
            e.name()
        );
    }
}

// @lat: [[tests#Cypher#Union default graph reads every graph]]
#[test]
fn union_default_graph() {
    for e in Engine::all() {
        e.load_as(
            RdfFormat::TriG,
            r#"@prefix ex: <http://example.com/> .
            ex:acme ex:name "ACME" .
            ex:g1 { ex:c1 a ex:Credential ; ex:issuer ex:acme ; ex:subject ex:alice . ex:alice ex:job "Engineer" . }
            ex:g2 { ex:c2 a ex:Credential ; ex:issuer ex:acme ; ex:subject ex:bob . ex:bob ex:job "Analyst" . }
            ex:g3 { ex:c2 ex:issuer ex:acme . }"#,
        );
        let q = "MATCH (c:Credential)-[:subject]->(s), (c)-[:issuer]->(i) RETURN s.job AS job, i.name AS issuer, labels(c) AS l ORDER BY job";
        let default = e.run(q, &Params::new(), &ex_opts()).unwrap();
        assert!(default.rows.is_empty(), "{}", e.name());
        let mut opts = ex_opts();
        opts.query.union_default_graph = true;
        let r = e.run(q, &Params::new(), &opts).unwrap();
        let l = Value::List(vec![s("Credential")]);
        assert_eq!(
            r.rows,
            vec![
                vec![s("Analyst"), s("ACME"), l.clone()],
                vec![s("Engineer"), s("ACME"), l],
            ],
            "{}",
            e.name()
        );
        // ex:c2's issuer triple is in two graphs: still two credentials, not three.
        let r = e
            .run(
                "MATCH (c:Credential)-[:issuer]->(i) RETURN i.name AS issuer, count(c) AS n",
                &Params::new(),
                &opts,
            )
            .unwrap();
        assert_eq!(r.rows, vec![vec![s("ACME"), i(2)]], "{}", e.name());
    }
}

// @lat: [[tests#Cypher#Ordering by an aggregate]]
#[test]
fn order_by_aggregate() {
    for e in Engine::all() {
        social(&e);
        let r = e.cypher(
            "MATCH (p:Person)-[:KNOWS]->(f) RETURN p.name AS name, count(f) AS n ORDER BY n DESC, name",
        );
        assert_eq!(
            r.rows,
            vec![
                vec![s("Ada"), i(2)],
                vec![s("Alan"), i(1)],
                vec![s("Grace"), i(1)],
                vec![s("Linus"), i(1)],
            ],
            "{}",
            e.name()
        );
        let r = e.cypher(
            "MATCH (p:Person)-[:KNOWS]->(f) RETURN p.name AS name, count(f) AS n ORDER BY n DESC LIMIT 1",
        );
        assert_eq!(r.rows, vec![vec![s("Ada"), i(2)]], "{}", e.name());
    }
}

/// Shapes exercising every constraint the index carries, including two shapes targeting the
/// same class and path.
const AGREEMENT_SHAPES: &str = "@prefix ex: <http://example.com/> . @prefix sh: <http://www.w3.org/ns/shacl#> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:property [ sh:path ex:name ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ; sh:pattern \"^[A-Z]\" ] ;
  sh:property [ sh:path ex:age ; sh:datatype xsd:integer ] ;
  sh:property [ sh:path ex:status ; sh:in ( \"active\" \"retired\" 1 true ) ] ;
  sh:property [ sh:path ex:employer ; sh:class ex:Company ] ;
  sh:property [ sh:path ex:address ; sh:node ex:AddressShape ] .
ex:PersonAgeShape a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:property [ sh:path ex:age ; sh:minCount 0 ; sh:maxCount 1 ] .
ex:CompanyShape a sh:NodeShape ; sh:targetClass ex:Company ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ] .";

// @lat: [[tests#Cypher#Shape index agrees with the shapes query]]
#[test]
fn shape_index_agrees_with_shapes_query() {
    let store = Store::new().unwrap();
    store
        .load_from_slice(RdfFormat::Turtle, AGREEMENT_SHAPES.as_bytes())
        .unwrap();

    let from_index = oxilite::cypher::Schema::from_index(store.shape_index().unwrap());
    let from_query = oxilite::cypher::Schema::from_output(
        store
            .query_output(
                oxilite_cypher::schema_query(),
                &oxilite_core::QueryOptions::default(),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(!from_index.is_empty());
    assert_eq!(from_index, from_query);
}
