//! Versioning: the store clock, the immutable change log and time travel
//! (see `openspec/changes/versioned-store`).

use oxilite::core::sql::{Capabilities, Request, Response};
use oxilite::core::SyncBackend;
use oxilite::model::*;
use oxilite::rusqlite::RusqliteBackend;
use oxilite::sparql::QueryResults;
use oxilite::store::Store;
use oxilite::version::{CommitInfo, History, LevelChange, Versioning};
use oxilite::{QueryOptions, Result, StoreOptions};
use std::collections::BTreeSet;
use std::sync::Mutex;

fn ex(s: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("http://example.com/{s}"))
}

fn quad(s: &str, p: &str, o: &str) -> Quad {
    Quad::new(ex(s), ex(p), ex(o), GraphName::DefaultGraph)
}

fn store(level: Versioning) -> Store<RusqliteBackend> {
    Store::with_backend_and_options(
        RusqliteBackend::memory().unwrap(),
        &StoreOptions {
            versioning: level,
            ..Default::default()
        },
    )
    .unwrap()
}

/// The triples of the default graph as `s p o` strings.
fn triples(
    store: &Store<impl SyncBackend + Send + Sync + 'static>,
    as_of: Option<&str>,
) -> Result<BTreeSet<String>> {
    let options = QueryOptions {
        as_of: as_of.map(str::to_owned),
        ..Default::default()
    };
    let QueryResults::Solutions(s) = store.query_opt("SELECT ?s ?p ?o { ?s ?p ?o }", options)?
    else {
        panic!("expected solutions")
    };
    let mut out = BTreeSet::new();
    for row in s {
        let row = row?;
        out.insert(format!(
            "{} {} {}",
            row.get("s").unwrap(),
            row.get("p").unwrap(),
            row.get("o").unwrap()
        ));
    }
    Ok(out)
}

fn table_exists(store: &Store<RusqliteBackend>, name: &str) -> bool {
    store.backend().with_connection(|c| {
        c.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    })
}

/// Records every request sent to the backend.
struct Recording {
    inner: RusqliteBackend,
    sql: Mutex<Vec<String>>,
}

impl SyncBackend for Recording {
    fn execute(&self, request: &Request) -> Result<Response> {
        self.sql
            .lock()
            .unwrap()
            .extend(request.statements.iter().map(|s| s.sql.clone()));
        self.inner.execute(request)
    }
    fn capabilities(&self) -> &Capabilities {
        self.inner.capabilities()
    }
}

// @lat: [[tests#Versioning#Off store is unchanged]]
#[test]
fn off_store_is_unchanged() {
    let backend = Recording {
        inner: RusqliteBackend::memory().unwrap(),
        sql: Mutex::default(),
    };
    let store = Store::with_backend(backend).unwrap();
    store.insert(&quad("a", "p", "b")).unwrap();
    store
        .update("INSERT DATA { <http://example.com/c> <http://example.com/p> 1 }")
        .unwrap();
    let sql = store.backend().sql.lock().unwrap().clone();
    assert!(sql
        .iter()
        .all(|s| !s.contains("ticks") && !s.contains("quad_log")));
    assert!(sql
        .iter()
        .any(|s| s.starts_with("INSERT OR IGNORE INTO quads(s, p, o, g) VALUES ")));
    let status = store.versioning().unwrap();
    assert_eq!(status.state.level, Versioning::Off);
    assert_eq!(status.head, None);
    let err = triples(&store, Some("HEAD")).unwrap_err().to_string();
    assert!(err.contains("keeps no history"), "{err}");
}

// @lat: [[tests#Versioning#Stamps record the adding tick]]
#[test]
fn stamps_record_the_adding_tick() {
    let store = store(Versioning::Stamped);
    assert!(table_exists(&store, "ticks"));
    assert!(!table_exists(&store, "quad_log"));
    store.insert(&quad("a", "p", "1")).unwrap();
    let t1 = store.versioning().unwrap().head.unwrap();
    store.insert(&quad("b", "p", "2")).unwrap();
    store
        .update(
            "INSERT DATA { <http://example.com/c> <http://example.com/p> <http://example.com/3> }",
        )
        .unwrap();
    // Re-adding a present quad keeps its first tick.
    store.insert(&quad("a", "p", "1")).unwrap();
    let since = store.changes(t1, None).unwrap();
    let added: BTreeSet<_> = since.iter().map(|c| c.quad.subject.to_string()).collect();
    assert_eq!(
        added,
        ["<http://example.com/b>", "<http://example.com/c>"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert!(since.windows(2).all(|w| w[0].tick < w[1].tick));
    // A delete leaves no trace at this level, and as-of needs a change log.
    store.remove(&quad("b", "p", "2")).unwrap();
    assert_eq!(store.changes(t1, None).unwrap().len(), 1);
    let err = triples(&store, Some("HEAD~1")).unwrap_err().to_string();
    assert!(err.contains("keeps no history"), "{err}");
}

// @lat: [[tests#Versioning#As-of equals snapshots]]
#[test]
fn as_of_equals_snapshots() {
    let store = store(Versioning::Log);
    let mut snapshots = Vec::new();
    // A deterministic pseudo-random walk over a small universe of quads.
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    for _ in 0..40 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let q = quad(&format!("s{}", x % 5), "p", &format!("o{}", (x >> 8) % 3));
        match (x >> 16) % 4 {
            0 | 1 => {
                store.insert(&q).unwrap();
            }
            2 => {
                store.remove(&q).unwrap();
            }
            _ => store
                .update(format!(
                    "DELETE {{ {s} ?p ?o }} INSERT {{ {s} <http://example.com/q> {o} }} WHERE {{ {s} ?p ?o }}",
                    s = q.subject,
                    o = q.object
                ))
                .unwrap(),
        }
        let tick = store.versioning().unwrap().head.unwrap();
        snapshots.push((tick, triples(&store, None).unwrap()));
    }
    for (tick, expected) in &snapshots {
        assert_eq!(
            &triples(&store, Some(&format!("#{tick}"))).unwrap(),
            expected,
            "at #{tick}"
        );
    }
    assert_eq!(
        triples(&store, Some("HEAD")).unwrap(),
        snapshots.last().unwrap().1
    );
}

// @lat: [[tests#Versioning#Only effective changes are logged]]
#[test]
fn only_effective_changes_are_logged() {
    let store = store(Versioning::Log);
    store.insert(&quad("a", "p", "b")).unwrap();
    let before = store.versioning().unwrap().commits.unwrap();
    store.insert(&quad("a", "p", "b")).unwrap();
    store.remove(&quad("x", "p", "y")).unwrap();
    assert_eq!(store.versioning().unwrap().commits.unwrap(), before);
    // Removed and re-added within one update: no net change is recorded.
    store
        .update("DELETE DATA { <http://example.com/a> <http://example.com/p> <http://example.com/b> } ; INSERT DATA { <http://example.com/a> <http://example.com/p> <http://example.com/b> }")
        .unwrap();
    let head = store.versioning().unwrap().head.unwrap();
    assert!(store.changes(head - 1, None).unwrap().is_empty());
}

// @lat: [[tests#Versioning#History is immutable]]
#[test]
fn history_is_immutable() {
    let store = store(Versioning::Log);
    store.insert(&quad("a", "p", "b")).unwrap();
    store.insert(&quad("c", "p", "d")).unwrap();
    for sql in [
        "UPDATE quad_log SET op = 0",
        "DELETE FROM quad_log",
        "DELETE FROM ticks",
        "UPDATE ticks SET message = 'x'",
    ] {
        let err = store
            .backend()
            .execute(&Request::atomic(vec![sql.into()]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("history is immutable"), "{sql}: {err}");
    }
}

// @lat: [[tests#Versioning#Commits carry author and message]]
#[test]
fn commits_carry_author_and_message() {
    let store = store(Versioning::Log);
    store
        .with_commit(
            CommitInfo {
                author: Some("ada".into()),
                message: Some("seed".into()),
            },
            |s| s.insert(&quad("a", "p", "b")).map(|_| ()),
        )
        .unwrap();
    store.insert(&quad("c", "p", "d")).unwrap();
    let log = store.history(10).unwrap();
    let seed = log
        .iter()
        .find(|c| c.message.as_deref() == Some("seed"))
        .unwrap();
    assert_eq!(seed.author.as_deref(), Some("ada"));
    assert_eq!(seed.added, Some(1));
    assert_eq!(log[0].author, None);
    assert!(log.iter().any(|c| c.kind == "genesis"));
}

// @lat: [[tests#Versioning#Versions compare within one query]]
#[test]
fn versions_compare_within_one_query() {
    let store = store(Versioning::Log);
    store
        .update("INSERT DATA { <http://example.com/t1> <http://example.com/status> \"open\" . <http://example.com/t2> <http://example.com/status> \"open\" }")
        .unwrap();
    store
        .update("DELETE { <http://example.com/t1> <http://example.com/status> ?o } INSERT { <http://example.com/t1> <http://example.com/status> \"done\" } WHERE { <http://example.com/t1> <http://example.com/status> ?o }")
        .unwrap();
    let QueryResults::Solutions(s) = store
        .query(
            "SELECT ?s ?old ?new { ?s <http://example.com/status> ?new \
             SERVICE <oxilite:version/HEAD~1> { ?s <http://example.com/status> ?old } FILTER(?old != ?new) }",
        )
        .unwrap()
    else {
        panic!()
    };
    let rows: Vec<_> = s.collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("s").unwrap().to_string(),
        "<http://example.com/t1>"
    );
    assert_eq!(rows[0].get("old").unwrap().to_string(), "\"open\"");
    assert_eq!(rows[0].get("new").unwrap().to_string(), "\"done\"");
    let diff = store.diff("HEAD~1", "HEAD").unwrap();
    assert_eq!(diff.iter().filter(|c| c.added).count(), 1);
    assert_eq!(diff.iter().filter(|c| !c.added).count(), 1);
    // Property paths read the version too.
    store
        .update("INSERT DATA { <http://example.com/a> <http://example.com/next> <http://example.com/b> }")
        .unwrap();
    store
        .update("INSERT DATA { <http://example.com/b> <http://example.com/next> <http://example.com/c> }")
        .unwrap();
    let reach = |as_of: Option<&str>| {
        let QueryResults::Solutions(s) = store
            .query_opt(
                "SELECT ?x { <http://example.com/a> <http://example.com/next>+ ?x }",
                QueryOptions {
                    as_of: as_of.map(str::to_owned),
                    ..Default::default()
                },
            )
            .unwrap()
        else {
            panic!()
        };
        s.count()
    };
    assert_eq!(reach(None), 2);
    assert_eq!(reach(Some("HEAD~1")), 1);
}

// @lat: [[tests#Versioning#Opening never changes the level]]
#[test]
fn opening_never_changes_the_level() {
    let dir = tempfile_dir();
    let path = dir.join("store.db");
    {
        let s = Store::open(&path).unwrap();
        s.insert(&quad("a", "p", "b")).unwrap();
    }
    let err = Store::open_with_options(
        &path,
        StoreOptions {
            versioning: Versioning::Log,
            ..Default::default()
        },
    )
    .err()
    .unwrap()
    .to_string();
    assert!(err.contains("opening does not change it"), "{err}");
    {
        let s = Store::open(&path).unwrap();
        s.set_versioning(Versioning::Log, LevelChange::default())
            .unwrap();
    }
    // Default options on a versioned store keep its level.
    let s = Store::open(&path).unwrap();
    assert_eq!(s.versioning().unwrap().state.level, Versioning::Log);
    s.insert(&quad("c", "p", "d")).unwrap();
    assert_eq!(triples(&s, Some("HEAD~1")).unwrap().len(), 1);
    std::fs::remove_dir_all(dir).ok();
}

// @lat: [[tests#Versioning#Upgrade records the store as genesis]]
#[test]
fn upgrade_records_the_store_as_genesis() {
    let store = store(Versioning::Off);
    store.insert(&quad("a", "p", "b")).unwrap();
    store.insert(&quad("c", "p", "d")).unwrap();
    let status = store
        .set_versioning(Versioning::Log, LevelChange::default())
        .unwrap();
    assert_eq!(status.state.history, History::Live);
    let genesis = status.genesis.unwrap();
    assert_eq!(
        triples(&store, Some(&format!("#{genesis}"))).unwrap().len(),
        2
    );
    store.remove(&quad("a", "p", "b")).unwrap();
    assert_eq!(
        triples(&store, Some(&format!("#{genesis}"))).unwrap().len(),
        2
    );
    assert_eq!(triples(&store, Some("HEAD")).unwrap().len(), 1);
    // The stamping tick before genesis is outside the recorded history.
    let err = triples(&store, Some(&format!("#{}", genesis - 1)))
        .unwrap_err()
        .to_string();
    assert!(err.contains("before the recorded history"), "{err}");
}

// @lat: [[tests#Versioning#Downgrades freeze and resume]]
#[test]
fn downgrades_freeze_and_resume() {
    let store = store(Versioning::Log);
    store.insert(&quad("a", "p", "b")).unwrap();
    store.insert(&quad("c", "p", "d")).unwrap();
    let status = store
        .set_versioning(Versioning::Stamped, LevelChange::default())
        .unwrap();
    assert_eq!(status.state.history, History::Frozen);
    let freeze = status.frozen_at.unwrap();
    // Writes during the gap are not recorded, but the frozen history still answers.
    store.remove(&quad("a", "p", "b")).unwrap();
    store.insert(&quad("e", "p", "f")).unwrap();
    let gap = store.versioning().unwrap().head.unwrap();
    assert_eq!(
        triples(&store, Some(&format!("#{freeze}"))).unwrap().len(),
        2
    );
    let status = store
        .set_versioning(Versioning::Log, LevelChange::default())
        .unwrap();
    assert_eq!(status.state.history, History::Live);
    let err = triples(&store, Some(&format!("#{gap}")))
        .unwrap_err()
        .to_string();
    assert!(err.contains("gap"), "{err}");
    // The resume commit records the gap's net changes, so the history is exact again.
    store.insert(&quad("g", "p", "h")).unwrap();
    assert_eq!(triples(&store, Some("HEAD~1")).unwrap(), {
        let mut t = triples(&store, None).unwrap();
        t.remove("<http://example.com/g> <http://example.com/p> <http://example.com/h>");
        t
    });
    assert_eq!(
        triples(&store, Some(&format!("#{freeze}"))).unwrap().len(),
        2
    );
    // Deleting the history needs allow_loss.
    let status = store
        .set_versioning(
            Versioning::Off,
            LevelChange {
                allow_loss: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(status.state.history, History::None);
    assert!(!status.state.stamp_column);
    assert!(!table_exists(&store, "quad_log"));
    assert!(!table_exists(&store, "ticks"));
    assert_eq!(triples(&store, None).unwrap().len(), 3);
}

// @lat: [[tests#Versioning#Purge removes from history]]
#[test]
fn purge_removes_from_history() {
    let store = store(Versioning::Log);
    store.insert(&quad("alice", "p", "x")).unwrap();
    store.insert(&quad("bob", "p", "y")).unwrap();
    let t = store.versioning().unwrap().head.unwrap();
    store
        .purge(
            Some(ex("alice").as_ref().into()),
            None,
            None,
            None,
            Some("erasure request"),
        )
        .unwrap();
    let alice = |s: &BTreeSet<String>| s.iter().any(|t| t.contains("alice"));
    assert!(!alice(&triples(&store, None).unwrap()));
    assert!(!alice(&triples(&store, Some(&format!("#{t}"))).unwrap()));
    assert!(store.changes(0, None).unwrap().iter().all(|c| !c
        .quad
        .subject
        .to_string()
        .contains("alice")));
    let log = store.history(1).unwrap();
    assert_eq!(log[0].kind, "purge");
    assert_eq!(log[0].message.as_deref(), Some("erasure request"));
}

// @lat: [[tests#Versioning#D1 batches keep a statement for the tick]]
#[test]
fn d1_batches_keep_a_statement_for_the_tick() {
    use oxilite::core::version::{effective_caps, prepare};
    let caps = effective_caps(&Capabilities::d1(), Versioning::Log);
    assert_eq!(caps.max_statements, 48);
    let quads: Vec<Quad> = (0..3000)
        .map(|i| quad(&format!("s{i}"), "p", &format!("o{i}")))
        .collect();
    let enc = oxilite::core::writer::EncodedQuads::new(quads.iter().map(Quad::as_ref));
    let stmts = enc.insert_statements(&caps);
    assert!(stmts.iter().all(|s| s.sql.len() <= caps.max_sql_len));
    let req = Request::atomic(stmts.into_iter().take(caps.max_statements).collect());
    let prepared = prepare(&req, Versioning::Log, &CommitInfo::default()).unwrap();
    assert!(prepared.statements.len() <= Capabilities::d1().max_statements);
    assert!(prepared.statements[0]
        .sql
        .starts_with("INSERT OR IGNORE INTO terms("));
    assert!(prepared.statements[1].sql.starts_with("INSERT INTO ticks("));
    let off = Capabilities::d1();
    let plain = enc.insert_statements(&off);
    assert!(plain.iter().all(|s| !s.sql.contains("ticks")));
}

fn tempfile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "oxilite-versioning-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ------------------------------------------------------------------ every engine, D1 included

use futures::executor::block_on;
use oxilite::{AsyncBackend, AsyncStore};

/// Rusqlite behind the async interface with D1's capabilities.
struct D1Like(RusqliteBackend, Capabilities);

impl AsyncBackend for D1Like {
    async fn execute(&self, request: &Request) -> Result<Response> {
        self.0.execute(request)
    }
    fn capabilities(&self) -> &Capabilities {
        &self.1
    }
}

/// The Miniflare D1 sidecar (`testsuite/d1-sidecar`), when `OXILITE_D1_URL` is set.
struct HttpD1(String, Capabilities);

impl AsyncBackend for HttpD1 {
    async fn execute(&self, request: &Request) -> Result<Response> {
        let body = serde_json::to_value(request).map_err(oxilite::Error::backend)?;
        match ureq::post(&format!("{}/execute", self.0)).send_json(body) {
            Ok(r) => r.into_json().map_err(oxilite::Error::backend),
            Err(ureq::Error::Status(_, r)) => {
                let v: serde_json::Value = r.into_json().unwrap_or_default();
                Err(oxilite::Error::backend(
                    v["error"].as_str().unwrap_or("D1 error"),
                ))
            }
            Err(e) => Err(oxilite::Error::backend(e)),
        }
    }
    fn capabilities(&self) -> &Capabilities {
        &self.1
    }
}

static SIDECAR: Mutex<()> = Mutex::new(());

enum Engine {
    Native(Store<RusqliteBackend>),
    Dylib(Store<oxilite::dylib::DylibBackend>),
    D1(AsyncStore<D1Like>),
    Miniflare(
        AsyncStore<HttpD1>,
        #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
    ),
}

impl Engine {
    fn all(options: &StoreOptions) -> Vec<(&'static str, Self)> {
        let mut out = vec![
            (
                "native",
                Self::Native(
                    Store::with_backend_and_options(RusqliteBackend::memory().unwrap(), options)
                        .unwrap(),
                ),
            ),
            (
                "d1",
                Self::D1(
                    block_on(AsyncStore::open_with_options(
                        D1Like(RusqliteBackend::memory().unwrap(), Capabilities::d1()),
                        options,
                    ))
                    .unwrap(),
                ),
            ),
        ];
        // The platform SQLite: triggers and SQL must work on older versions too.
        if let Some(lib) = oxilite::dylib::find_system_library() {
            let backend = oxilite::dylib::DylibBackend::open(lib, ":memory:").unwrap();
            out.push((
                "dylib",
                Self::Dylib(Store::with_backend_and_options(backend, options).unwrap()),
            ));
        }
        if let Ok(url) = std::env::var("OXILITE_D1_URL") {
            let guard = SIDECAR
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ureq::post(&format!("{url}/reset")).call().unwrap();
            let store = block_on(AsyncStore::open_with_options(
                HttpD1(url, Capabilities::d1()),
                options,
            ))
            .unwrap();
            out.push(("miniflare", Self::Miniflare(store, guard)));
        }
        out
    }

    fn update(&self, u: &str) -> Result<()> {
        match self {
            Self::Native(s) => s.update(u),
            Self::Dylib(s) => s.update(u),
            Self::D1(s) => block_on(s.update(u)),
            Self::Miniflare(s, _) => block_on(s.update(u)),
        }
    }

    fn triples(&self, as_of: Option<&str>) -> Result<BTreeSet<String>> {
        let options = QueryOptions {
            as_of: as_of.map(str::to_owned),
            ..Default::default()
        };
        let q = "SELECT ?s ?p ?o { ?s ?p ?o }";
        let r = match self {
            Self::Native(s) => s.query_opt(q, options)?,
            Self::Dylib(s) => s.query_opt(q, options)?,
            Self::D1(s) => block_on(s.query_opt(q, options))?,
            Self::Miniflare(s, _) => block_on(s.query_opt(q, options))?,
        };
        let QueryResults::Solutions(s) = r else {
            panic!()
        };
        s.map(|row| {
            let row = row?;
            Ok(format!(
                "{} {} {}",
                row.get("s").unwrap(),
                row.get("p").unwrap(),
                row.get("o").unwrap()
            ))
        })
        .collect()
    }

    fn status(&self) -> Result<oxilite::version::VersionStatus> {
        match self {
            Self::Native(s) => s.versioning(),
            Self::Dylib(s) => s.versioning(),
            Self::D1(s) => block_on(s.versioning()),
            Self::Miniflare(s, _) => block_on(s.versioning()),
        }
    }

    fn history(&self) -> Result<Vec<oxilite::version::CommitRecord>> {
        match self {
            Self::Native(s) => s.history(100),
            Self::Dylib(s) => s.history(100),
            Self::D1(s) => block_on(s.history(100)),
            Self::Miniflare(s, _) => block_on(s.history(100)),
        }
    }

    fn set_versioning(&self, level: Versioning) -> Result<oxilite::version::VersionStatus> {
        let c = LevelChange::default();
        match self {
            Self::Native(s) => s.set_versioning(level, c),
            Self::Dylib(s) => s.set_versioning(level, c),
            Self::D1(s) => block_on(s.set_versioning(level, c)),
            Self::Miniflare(s, _) => block_on(s.set_versioning(level, c)),
        }
    }
}

// @lat: [[tests#Versioning#Every engine keeps the same history]]
#[test]
fn every_engine_keeps_the_same_history() {
    let options = StoreOptions {
        versioning: Versioning::Log,
        ..Default::default()
    };
    for (name, e) in Engine::all(&options) {
        e.update("INSERT DATA { <http://example.com/t1> <http://example.com/status> \"open\" . <http://example.com/t2> <http://example.com/status> \"open\" }")
            .unwrap();
        let first = e.triples(None).unwrap();
        e.update("DELETE { ?t <http://example.com/status> \"open\" } INSERT { ?t <http://example.com/status> \"done\" } WHERE { ?t <http://example.com/status> \"open\" FILTER(?t = <http://example.com/t1>) }")
            .unwrap();
        e.update("DELETE DATA { <http://example.com/t2> <http://example.com/status> \"open\" }")
            .unwrap();
        let last = e.triples(None).unwrap();
        assert_eq!(e.triples(Some("HEAD~2")).unwrap(), first, "{name}");
        assert_eq!(e.triples(Some("HEAD")).unwrap(), last, "{name}");
        let commits: Vec<_> = e
            .history()
            .unwrap()
            .into_iter()
            .filter(|c| c.kind == "write")
            .collect();
        assert_eq!(commits.len(), 3, "{name}");
        assert_eq!(
            (commits[0].added, commits[0].removed),
            (Some(0), Some(1)),
            "{name}"
        );
        assert_eq!(
            (commits[1].added, commits[1].removed),
            (Some(1), Some(1)),
            "{name}"
        );
        // Freeze, write in the gap, resume: the history stays exact on every engine.
        let frozen = e.set_versioning(Versioning::Stamped).unwrap();
        assert_eq!(frozen.state.history, History::Frozen, "{name}");
        e.update("INSERT DATA { <http://example.com/t3> <http://example.com/status> \"new\" }")
            .unwrap();
        let resumed = e.set_versioning(Versioning::Log).unwrap();
        assert_eq!(resumed.state.history, History::Live, "{name}");
        assert_eq!(
            e.triples(Some("HEAD")).unwrap(),
            e.triples(None).unwrap(),
            "{name}"
        );
        assert_eq!(
            e.triples(Some(&format!("#{}", frozen.frozen_at.unwrap())))
                .unwrap(),
            last,
            "{name}"
        );
        assert!(e.status().unwrap().commits.unwrap() >= 4, "{name}");
    }
}

// @lat: [[tests#Versioning#Updates do not read the past]]
#[test]
fn updates_do_not_read_the_past() {
    let store = store(Versioning::Log);
    store.insert(&quad("a", "p", "b")).unwrap();
    store.remove(&quad("a", "p", "b")).unwrap();
    let err = store
        .update("INSERT { ?s ?p ?o } WHERE { SERVICE <oxilite:version/HEAD~1> { ?s ?p ?o } }")
        .unwrap_err()
        .to_string();
    assert!(err.contains("works in queries, not in updates"), "{err}");
    assert!(triples(&store, None).unwrap().is_empty());
}

// @lat: [[tests#Versioning#History graph answers who changed what]]
#[test]
fn history_graph_answers_who_changed_what() {
    let store = store(Versioning::Log);
    store
        .with_commit(
            CommitInfo {
                author: Some("ada".into()),
                message: Some("grant".into()),
            },
            |s| s.insert(&quad("alice", "role", "admin")).map(|_| ()),
        )
        .unwrap();
    store
        .with_commit(
            CommitInfo {
                author: Some("bob".into()),
                message: Some("revoke".into()),
            },
            |s| s.remove(&quad("alice", "role", "admin")).map(|_| ()),
        )
        .unwrap();
    let q = "PREFIX prov: <http://www.w3.org/ns/prov#> PREFIX oxl: <https://oxilite.dev/ns#> \
             PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
             SELECT ?c ?who ?why ?when ?before WHERE { GRAPH <oxilite:history> { \
               ?c oxl:removed <<( <http://example.com/alice> <http://example.com/role> <http://example.com/admin> )>> ; \
                  prov:wasAssociatedWith ?who ; rdfs:comment ?why ; prov:startedAtTime ?when ; prov:wasInformedBy ?before } }";
    let QueryResults::Solutions(s) = store.query(q).unwrap() else {
        panic!()
    };
    let rows: Vec<_> = s.collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.get("who").unwrap().to_string(), "\"bob\"");
    assert_eq!(r.get("why").unwrap().to_string(), "\"revoke\"");
    assert!(r.get("when").unwrap().to_string().contains("dateTime"));
    let c: i64 = match r.get("c").unwrap() {
        Term::Literal(l) => l.value().parse().unwrap(),
        t => panic!("{t}"),
    };
    let before: i64 = match r.get("before").unwrap() {
        Term::Literal(l) => l.value().parse().unwrap(),
        t => panic!("{t}"),
    };
    assert!(before < c);
    // Every change, with its commit's author, as bindings.
    let q2 = "PREFIX oxl: <https://oxilite.dev/ns#> PREFIX prov: <http://www.w3.org/ns/prov#> \
              SELECT ?who ?s ?p ?o WHERE { GRAPH <oxilite:history> { ?c oxl:added <<( ?s ?p ?o )>> ; prov:wasAssociatedWith ?who } }";
    let QueryResults::Solutions(s) = store.query(q2).unwrap() else {
        panic!()
    };
    let rows: Vec<_> = s.collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("who").unwrap().to_string(), "\"ada\"");
    // The tick of a commit is the same number as `#n`.
    let at = store.resolve_version(&format!("#{c}")).unwrap();
    assert_eq!(at, c);
    // Without a change log, changes are refused; commits are still listed.
    let stamped = self::store(Versioning::Stamped);
    stamped.insert(&quad("a", "p", "b")).unwrap();
    let err = stamped.query(q2).err().unwrap().to_string();
    assert!(err.contains("no change log"), "{err}");
    let QueryResults::Solutions(s) = stamped
        .query(
            "SELECT ?c { GRAPH <oxilite:history> { ?c a <http://www.w3.org/ns/prov#Activity> } }",
        )
        .unwrap()
    else {
        panic!()
    };
    assert!(s.count() >= 2);
}
