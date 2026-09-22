//! The oxilite side of the harness: store variants that every test runs on.
//!
// @lat: [[test-plan#Oxigraph compatibility harness]]

use anyhow::{Context, Result};
use oxigraph::model::{Dataset, Quad};
use oxilite::sparql::QueryResults;
use oxilite::QueryOptions;
use spargebra::algebra::GraphPattern;
use spargebra::{Query, Update};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

type Coverage = (usize, HashMap<String, usize>);

thread_local! {
    /// Queries evaluated fully in SQL vs. by the fallback, with fallback reasons (per test
    /// thread: each W3C suite runs on its own thread).
    static COVERAGE: RefCell<Option<Coverage>> = const { RefCell::new(None) };
}

fn record(explain: &str) {
    COVERAGE.with(|c| {
        let mut c = c.borrow_mut();
        let (compiled, reasons) = c.get_or_insert_with(|| (0, HashMap::new()));
        if explain.starts_with("-- oxilite: fully compiled") {
            *compiled += 1;
        } else {
            let reason = explain
                .split("SQL (")
                .nth(1)
                .and_then(|r| r.split(')').next())
                .unwrap_or(explain)
                .to_string();
            *reasons.entry(reason).or_default() += 1;
        }
    });
}

/// Takes and resets this thread's coverage counters: (compiled, fallback reasons).
pub fn take_coverage() -> Coverage {
    COVERAGE.with(|c| c.borrow_mut().take().unwrap_or_default())
}

/// A configuration oxilite is tested in. Plans and backends must never change results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// rusqlite, no statistics (heuristic planner).
    Rusqlite,
    /// rusqlite after `optimize()` (statistics-driven planner).
    RusqliteOptimized,
    /// rusqlite, SQLite chooses the join order.
    RusqliteSqlitePlanner,
    /// A system `libsqlite3` loaded at runtime (no UDFs).
    Dylib,
    /// Cloudflare D1 (Miniflare, the engine behind `wrangler dev`) through the
    /// `testsuite/d1-sidecar` HTTP bridge and the async store; enabled by `OXILITE_D1_URL`.
    D1,
}

impl fmt::Display for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rusqlite => "rusqlite",
            Self::RusqliteOptimized => "rusqlite-optimized",
            Self::RusqliteSqlitePlanner => "rusqlite-sqlite-planner",
            Self::Dylib => "dylib",
            Self::D1 => "d1",
        })
    }
}

/// Variants to run, from `OXILITE_VARIANTS` (comma separated) or all available ones.
pub fn variants() -> Vec<Variant> {
    let all = [
        Variant::Rusqlite,
        Variant::RusqliteOptimized,
        Variant::RusqliteSqlitePlanner,
        Variant::Dylib,
        Variant::D1,
    ];
    let wanted = std::env::var("OXILITE_VARIANTS").ok();
    all.into_iter()
        .filter(|v| {
            wanted
                .as_ref()
                .is_none_or(|w| w.split(',').any(|x| x.trim() == v.to_string()))
        })
        .filter(|v| *v != Variant::Dylib || oxilite::dylib::find_system_library().is_some())
        .filter(|v| *v != Variant::D1 || std::env::var("OXILITE_D1_URL").is_ok())
        .collect()
}

/// A query or update the D1 backend cannot run (it needs the sync fallback evaluator).
#[derive(Debug)]
pub struct D1Unsupported(pub String);

impl fmt::Display for D1Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported on D1: {}", self.0)
    }
}

impl std::error::Error for D1Unsupported {}

/// D1 through the HTTP sidecar (`testsuite/d1-sidecar/server.mjs`).
pub struct HttpD1 {
    url: String,
    caps: oxilite::Capabilities,
}

impl HttpD1 {
    fn post(&self, path: &str, body: serde_json::Value) -> oxilite::Result<serde_json::Value> {
        match ureq::post(&format!("{}{path}", self.url)).send_json(body) {
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
}

impl oxilite::AsyncBackend for HttpD1 {
    async fn execute(
        &self,
        request: &oxilite::core::Request,
    ) -> oxilite::Result<oxilite::core::Response> {
        let v = self.post(
            "/execute",
            serde_json::to_value(request).map_err(oxilite::Error::backend)?,
        )?;
        serde_json::from_value(v).map_err(oxilite::Error::backend)
    }

    fn capabilities(&self) -> &oxilite::Capabilities {
        &self.caps
    }
}

/// The sidecar has one database: D1 engines take turns.
static D1_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

enum Backend {
    Rusqlite(oxilite::store::Store),
    Dylib(oxilite::store::Store<oxilite::dylib::DylibBackend>),
    D1(
        oxilite::AsyncStore<HttpD1>,
        #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
    ),
}

macro_rules! with_store {
    ($self:expr, $s:ident => $e:expr) => {
        match &$self.backend {
            Backend::Rusqlite($s) => $e,
            Backend::Dylib($s) => $e,
            Backend::D1(..) => unreachable!("D1 is handled separately"),
        }
    };
}

fn block<T>(f: impl std::future::Future<Output = T>) -> T {
    futures::executor::block_on(f)
}

/// An oxilite store loaded with a dataset.
pub struct OxiliteEngine {
    backend: Backend,
    options: QueryOptions,
}

impl OxiliteEngine {
    pub fn new(variant: Variant, dataset: &Dataset) -> Result<Self> {
        if variant == Variant::D1 {
            let guard = D1_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let http = HttpD1 {
                url: std::env::var("OXILITE_D1_URL").context("OXILITE_D1_URL")?,
                caps: oxilite::Capabilities::d1(),
            };
            http.post("/reset", serde_json::json!({}))?;
            let store = block(oxilite::AsyncStore::open(http))?;
            let quads: Vec<Quad> = dataset.iter().map(|q| q.into_owned()).collect();
            for chunk in quads.chunks(300) {
                block(store.extend(chunk.to_vec()))?;
            }
            return Ok(Self {
                backend: Backend::D1(store, guard),
                options: QueryOptions::default(),
            });
        }
        let backend = match variant {
            Variant::Dylib => Backend::Dylib(oxilite::store::Store::open_with_library(
                oxilite::dylib::find_system_library().context("no system SQLite")?,
                ":memory:",
            )?),
            _ => Backend::Rusqlite(oxilite::store::Store::new()?),
        };
        let me = Self {
            backend,
            options: QueryOptions {
                sqlite_planner: variant == Variant::RusqliteSqlitePlanner,
                ..QueryOptions::default()
            },
        };
        with_store!(me, s => s.extend(dataset.iter().map(|q| q.into_owned())))?;
        if variant == Variant::RusqliteOptimized {
            with_store!(me, s => s.optimize())?;
        }
        Ok(me)
    }

    pub fn query(&self, query: &Query) -> Result<QueryResults<'static>> {
        record(&self.explain(query));
        if let Backend::D1(s, _) = &self.backend {
            return match block(s.query_opt(query, self.options.clone())) {
                Err(e) if e.is_unsupported() => Err(D1Unsupported(e.to_string()).into()),
                r => Ok(r?),
            };
        }
        Ok(with_store!(self, s => s.query_opt(query, self.options.clone()))?)
    }

    pub fn update(&self, update: &Update) -> Result<()> {
        if let Backend::D1(s, _) = &self.backend {
            return match block(s.update(update)) {
                Err(e) if e.is_unsupported() => Err(D1Unsupported(e.to_string()).into()),
                r => Ok(r?),
            };
        }
        let plan = with_store!(self, s => s.explain_update(update)).unwrap_or_default();
        record(if plan.contains("not compiled") {
            "-- oxilite: not fully compiled to SQL (update needs the fallback)"
        } else {
            "-- oxilite: fully compiled"
        });
        Ok(with_store!(self, s => s.update(update))?)
    }

    pub fn explain_update(&self, update: &Update) -> String {
        if let Backend::D1(s, _) = &self.backend {
            return s
                .explain_update(update)
                .unwrap_or_else(|e| format!("-- explain failed: {e}"));
        }
        with_store!(self, s => s.explain_update(update))
            .unwrap_or_else(|e| format!("-- explain failed: {e}"))
    }

    pub fn explain(&self, query: &Query) -> String {
        if let Backend::D1(s, _) = &self.backend {
            return s
                .explain(query)
                .unwrap_or_else(|e| format!("-- explain failed: {e}"));
        }
        with_store!(self, s => s.explain_opt(query, &self.options))
            .unwrap_or_else(|e| format!("-- explain failed: {e}"))
    }

    pub fn dataset(&self) -> Result<Dataset> {
        if let Backend::D1(s, _) = &self.backend {
            return Ok(block(s.quads_for_pattern(None, None, None, None))?
                .into_iter()
                .collect());
        }
        Ok(with_store!(self, s => s.iter().collect::<Result<Dataset, _>>())?)
    }
}

/// Does the query use SERVICE (out of scope for oxilite)?
pub fn uses_service(query: &Query) -> bool {
    fn walk(p: &GraphPattern) -> bool {
        match p {
            GraphPattern::Service { .. } => true,
            GraphPattern::Join { left, right }
            | GraphPattern::LeftJoin { left, right, .. }
            | GraphPattern::Union { left, right }
            | GraphPattern::Minus { left, right } => walk(left) || walk(right),
            GraphPattern::Filter { inner, .. }
            | GraphPattern::Graph { inner, .. }
            | GraphPattern::Extend { inner, .. }
            | GraphPattern::OrderBy { inner, .. }
            | GraphPattern::Project { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. }
            | GraphPattern::Group { inner, .. } => walk(inner),
            _ => false,
        }
    }
    match query {
        Query::Select { pattern, .. }
        | Query::Construct { pattern, .. }
        | Query::Describe { pattern, .. }
        | Query::Ask { pattern, .. } => walk(pattern),
    }
}
