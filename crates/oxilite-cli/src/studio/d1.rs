//! Cloudflare D1 over its HTTP API, as a blocking backend, and the store handle every
//! connection uses, so the same requests run on SQLite files and on D1.
//!
// @lat: [[architecture#Studio server#D1 connections]]

use oxilite::model::{GraphNameRef, NamedNodeRef, NamedOrBlankNodeRef, Quad, TermRef};
use oxilite::sparql::{Query, QueryOptions, Update};
use oxilite::store::Store;
use oxilite_core::{
    Capabilities, QueryOutput, Request, Response, ResultSet, SqlValue, SyncBackend,
};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// What D1 bills: rows read and written, and requests made, since the connection opened.
#[derive(Debug, Default)]
pub struct Meter {
    pub requests: AtomicU64,
    pub rows_read: AtomicU64,
    pub rows_written: AtomicU64,
}

impl Meter {
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.requests.load(Ordering::Relaxed),
            self.rows_read.load(Ordering::Relaxed),
            self.rows_written.load(Ordering::Relaxed),
        )
    }

    pub fn to_json(&self) -> Value {
        let (requests, read, written) = self.snapshot();
        json!({"requests": requests, "rowsRead": read, "rowsWritten": written})
    }
}

/// A D1 database reached through `POST /accounts/{account}/d1/database/{database}/raw`.
///
/// A request's statements go as one multi-statement `sql` string, which D1 runs as a batch,
/// its only transaction. oxilite inlines constants, so there are no bound parameters, and
/// `Capabilities::d1()` makes 64-bit ids come back as text, which JSON keeps exactly.
pub struct D1Http {
    endpoint: String,
    token: String,
    caps: Capabilities,
    pub meter: Arc<Meter>,
}

impl D1Http {
    pub fn new(account: &str, database: &str, token: &str) -> Self {
        Self::with_endpoint(
            &format!(
                "https://api.cloudflare.com/client/v4/accounts/{account}/d1/database/{database}"
            ),
            token,
        )
    }

    /// Against another base URL (a proxy, or a test double).
    pub fn with_endpoint(endpoint: &str, token: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token: token.to_string(),
            caps: Capabilities::d1(),
            meter: Arc::default(),
        }
    }
}

fn value(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => SqlValue::Integer(i),
            None => SqlValue::Real(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

impl SyncBackend for D1Http {
    fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        if request.statements.is_empty() {
            return Ok(Vec::new());
        }
        if request.statements.iter().any(|s| !s.params.is_empty()) {
            return Err(oxilite_core::Error::unsupported("bound parameters on D1"));
        }
        let sql = request
            .statements
            .iter()
            .map(|s| s.sql.trim_end_matches(';'))
            .collect::<Vec<_>>()
            .join(";\n");
        let response = ureq::post(&format!("{}/raw", self.endpoint))
            .set("Authorization", &format!("Bearer {}", self.token))
            .send_json(json!({ "sql": sql }));
        let body: Value = match response {
            Ok(r) => r.into_json().map_err(oxilite_core::Error::backend)?,
            Err(ureq::Error::Status(code, r)) => {
                let v: Value = r.into_json().unwrap_or_default();
                let message = v["errors"][0]["message"]
                    .as_str()
                    .map_or_else(|| format!("D1 HTTP {code}"), String::from);
                return Err(oxilite_core::Error::backend(message));
            }
            Err(e) => return Err(oxilite_core::Error::backend(e)),
        };
        if body["success"] == false {
            let message = body["errors"][0]["message"].as_str().unwrap_or("D1 error");
            return Err(oxilite_core::Error::backend(message));
        }
        self.meter.requests.fetch_add(1, Ordering::Relaxed);
        let results = body["result"].as_array().cloned().unwrap_or_default();
        Ok(results
            .iter()
            .map(|r| {
                let meta = &r["meta"];
                self.meter
                    .rows_read
                    .fetch_add(meta["rows_read"].as_u64().unwrap_or(0), Ordering::Relaxed);
                self.meter.rows_written.fetch_add(
                    meta["rows_written"].as_u64().unwrap_or(0),
                    Ordering::Relaxed,
                );
                ResultSet {
                    rows: r["results"]["rows"]
                        .as_array()
                        .map(|rows| {
                            rows.iter()
                                .map(|row| {
                                    row.as_array()
                                        .map(|c| c.iter().map(value).collect())
                                        .unwrap_or_default()
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    changes: meta["changes"].as_u64().unwrap_or(0),
                }
            })
            .collect())
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }
}

/// A connection's store: a SQLite file (bundled SQLite) or D1 over HTTP.
#[derive(Clone)]
pub enum Handle {
    Sqlite(Store),
    D1(Store<D1Http>, Arc<Meter>),
}

macro_rules! each {
    ($self:expr, $s:ident => $e:expr) => {
        match $self {
            Handle::Sqlite($s) => $e,
            Handle::D1($s, _) => $e,
        }
    };
}

impl Handle {
    pub fn meter(&self) -> Option<&Arc<Meter>> {
        match self {
            Handle::Sqlite(_) => None,
            Handle::D1(_, m) => Some(m),
        }
    }

    pub fn query_output(
        &self,
        q: Query,
        options: &QueryOptions,
    ) -> oxilite_core::Result<QueryOutput> {
        each!(self, s => s.query_output(q, options))
    }

    pub fn explain_opt(&self, q: Query, options: &QueryOptions) -> oxilite_core::Result<String> {
        each!(self, s => s.explain_opt(q, options))
    }

    pub fn update(&self, u: Update) -> oxilite_core::Result<()> {
        each!(self, s => s.update(u))
    }

    pub fn len(&self) -> oxilite_core::Result<usize> {
        each!(self, s => s.len())
    }

    pub fn materialize(&self) -> oxilite_core::Result<u64> {
        each!(self, s => s.materialize())
    }

    pub fn inference_producers(&self, quad: &Quad) -> oxilite_core::Result<Vec<String>> {
        each!(self, s => s.inference_producers(quad))
    }

    /// Whether any graph holds the triple.
    pub fn contains_any(
        &self,
        s: NamedOrBlankNodeRef<'_>,
        p: NamedNodeRef<'_>,
        o: TermRef<'_>,
    ) -> Result<bool> {
        Ok(
            each!(self, st => st.quads_for_pattern(Some(s), Some(p), Some(o), None).next().transpose()?.is_some()),
        )
    }

    pub fn datalog_with(
        &self,
        program: &str,
        options: &oxilite::datalog::Options,
    ) -> std::result::Result<oxilite::datalog::DatalogResult, oxilite::datalog::DatalogError> {
        each!(self, s => s.datalog_with(program, options))
    }

    pub fn explain_datalog(
        &self,
        program: &str,
    ) -> std::result::Result<String, oxilite::datalog::DatalogError> {
        each!(self, s => s.explain_datalog(program))
    }

    pub fn cypher_with(
        &self,
        text: &str,
        params: &oxilite::cypher::Params,
        options: &oxilite::cypher::CypherOptions,
    ) -> std::result::Result<oxilite::cypher::CypherResult, oxilite::cypher::CypherError> {
        each!(self, s => s.cypher_with(text, params, options))
    }

    pub fn explain_cypher(
        &self,
        text: &str,
        params: &oxilite::cypher::Params,
        options: &oxilite::cypher::CypherOptions,
    ) -> std::result::Result<String, oxilite::cypher::CypherError> {
        each!(self, s => s.explain_cypher(text, params, options))
    }

    pub fn load(&self, parser: oxilite::io::RdfParser, data: &[u8]) -> Result<()> {
        each!(self, s => {
            let mut loader = s.bulk_loader();
            loader.load_from_slice(parser, data)?;
            loader.commit()?;
        });
        Ok(())
    }

    pub fn dump<W: Write>(
        &self,
        format: oxilite::io::RdfFormat,
        graph: Option<GraphNameRef<'_>>,
        w: W,
    ) -> Result<()> {
        each!(self, s => match graph {
            Some(g) => { s.dump_graph_to_writer(g, format, w)?; }
            None => { s.dump_to_writer(format, w)?; }
        });
        Ok(())
    }
}

/// A D1 database's local file under `.wrangler/state` (from `wrangler dev`), if any.
pub fn local_databases(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let dir = root.join(".wrangler/state/v3/d1/miniflare-D1DatabaseObject");
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sqlite"))
        .collect();
    out.sort();
    out
}
