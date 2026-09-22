//! Test engines shared by the JSON-LD and VC tests: the native store, the platform SQLite
//! (dylib), the D1 code path (rusqlite behind the async interface with D1's limits) and,
//! when `OXILITE_D1_URL` is set, the Miniflare D1 sidecar (`testsuite/d1-sidecar`).
#![allow(dead_code)]

use futures::executor::block_on;
use oxilite::rusqlite::RusqliteBackend;
use oxilite::sparql::QueryResults;
use oxilite::store::Store;
use oxilite::{AsyncBackend, AsyncStore, Capabilities};
use oxilite_core::{Request, Response, SyncBackend};

pub struct D1Like(RusqliteBackend, Capabilities);

impl AsyncBackend for D1Like {
    async fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        if request.statements.len() > self.1.max_statements {
            return Err(oxilite_core::Error::backend("too many statements in batch"));
        }
        if request
            .statements
            .iter()
            .any(|s| s.sql.len() > self.1.max_sql_len)
        {
            return Err(oxilite_core::Error::backend("statement too long"));
        }
        self.0.execute(request)
    }
    fn capabilities(&self) -> &Capabilities {
        &self.1
    }
}

/// The Miniflare D1 sidecar (`testsuite/d1-sidecar`), when `OXILITE_D1_URL` is set.
pub struct HttpD1(String, Capabilities);

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

static SIDECAR: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub enum Engine {
    Native(Store),
    Dylib(Store<oxilite::dylib::DylibBackend>),
    D1(AsyncStore<D1Like>),
    Miniflare(
        AsyncStore<HttpD1>,
        #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
    ),
}

pub fn d1_like(caps: Capabilities) -> AsyncStore<D1Like> {
    block_on(AsyncStore::open(D1Like(
        RusqliteBackend::memory().unwrap(),
        caps,
    )))
    .unwrap()
}

impl Engine {
    pub fn all() -> Vec<Self> {
        let mut out = vec![
            Self::Native(Store::new().unwrap()),
            Self::D1(d1_like(Capabilities::d1())),
        ];
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

    pub fn name(&self) -> &'static str {
        match self {
            Self::Native(_) => "native",
            Self::Dylib(_) => "dylib",
            Self::D1(_) => "d1",
            Self::Miniflare(..) => "miniflare",
        }
    }

    pub fn query(&self, q: &str) -> QueryResults<'static> {
        match self {
            Self::Native(s) => s.query(q).unwrap(),
            Self::Dylib(s) => s.query(q).unwrap(),
            Self::D1(s) => block_on(s.query(q)).unwrap(),
            Self::Miniflare(s, _) => block_on(s.query(q)).unwrap(),
        }
    }

    pub fn update(&self, u: &str) {
        match self {
            Self::Native(s) => s.update(u).unwrap(),
            Self::Dylib(s) => s.update(u).unwrap(),
            Self::D1(s) => block_on(s.update(u)).unwrap(),
            Self::Miniflare(s, _) => block_on(s.update(u)).unwrap(),
        }
    }

    pub fn ask(&self, q: &str) -> bool {
        match self.query(q) {
            QueryResults::Boolean(b) => b,
            _ => panic!("not an ASK query"),
        }
    }

    pub fn count(&self, q: &str) -> usize {
        match self.query(q) {
            QueryResults::Solutions(s) => s.count(),
            _ => panic!("not a SELECT query"),
        }
    }

    pub fn quads(&self) -> usize {
        self.count("SELECT * WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } }")
    }

    pub fn sql_count(&self, sql: &str) -> i64 {
        let req = Request::read(vec![sql.into()]);
        let r = match self {
            Self::Native(s) => s.backend().execute(&req).unwrap(),
            Self::Dylib(s) => s.backend().execute(&req).unwrap(),
            Self::D1(s) => block_on(s.backend().execute(&req)).unwrap(),
            Self::Miniflare(s, _) => block_on(s.backend().execute(&req)).unwrap(),
        };
        r[0].rows[0][0].as_i64().unwrap()
    }
}

/// Calls a document-handle method on any engine (awaiting it on async ones).
#[macro_export]
macro_rules! docs {
    ($e:expr, $o:expr, $m:ident ( $($a:expr),* )) => {
        match $e {
            $crate::engine::Engine::Native(s) => s.jsonld_with($o.clone()).and_then(|d| d.$m($($a),*)),
            $crate::engine::Engine::Dylib(s) => s.jsonld_with($o.clone()).and_then(|d| d.$m($($a),*)),
            $crate::engine::Engine::D1(s) => ::futures::executor::block_on(async {
                match s.jsonld_with($o.clone()).await {
                    Ok(d) => d.$m($($a),*).await,
                    Err(e) => Err(e),
                }
            }),
            $crate::engine::Engine::Miniflare(s, _) => ::futures::executor::block_on(async {
                match s.jsonld_with($o.clone()).await {
                    Ok(d) => d.$m($($a),*).await,
                    Err(e) => Err(e),
                }
            }),
        }
    };
}
