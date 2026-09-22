//! The store behind the CLI: bundled SQLite, a dlopen'ed library, or D1 through the sidecar.

use crate::Location;
use oxilite::dylib::DylibBackend;
use oxilite::io::{RdfFormat, RdfParser};
use oxilite::model::{GraphName, NamedNode};
use oxilite::sparql::{QueryOptions, SparqlParser};
use oxilite::store::Store;
use oxilite::AsyncStore;
use oxilite_core::encoding::graph_id;
use oxilite_core::{AsyncBackend, Capabilities, QueryOutput, Request, Response, StoreOptions};
use std::sync::Mutex;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// D1 through the local sidecar (`testsuite/d1-sidecar/server.mjs`), which runs requests on a
/// Miniflare D1 database exactly like the `@oxilite/d1` driver.
pub struct SidecarD1 {
    url: String,
    caps: Capabilities,
}

impl AsyncBackend for SidecarD1 {
    async fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        let body = serde_json::to_value(request).map_err(oxilite_core::Error::backend)?;
        match ureq::post(&format!("{}/execute", self.url)).send_json(body) {
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
        &self.caps
    }
}

pub enum Db {
    Native(Store),
    Library(Store<DylibBackend>),
    D1(Box<Mutex<AsyncStore<SidecarD1>>>),
}

macro_rules! sync_store {
    ($self:expr, $s:ident => $e:expr, $d1:ident => $f:expr) => {
        match $self {
            Db::Native($s) => $e,
            Db::Library($s) => $e,
            Db::D1(m) => {
                let $d1 = m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                futures::executor::block_on($f)
            }
        }
    };
}

fn graph_ids(iris: &[String]) -> Result<Option<Vec<i64>>> {
    if iris.is_empty() {
        return Ok(None);
    }
    iris.iter()
        .map(|i| {
            Ok(graph_id(
                GraphName::from(NamedNode::new(i.as_str())?).as_ref(),
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

impl Db {
    pub fn open(l: &Location) -> Result<Self> {
        let options = StoreOptions {
            graph_index: !l.no_graph_index,
            text_index: l.text_index,
        };
        if let Some(url) = &l.d1_sidecar {
            let backend = SidecarD1 {
                url: url.trim_end_matches('/').to_string(),
                caps: Capabilities::d1(),
            };
            let store =
                futures::executor::block_on(AsyncStore::open_with_options(backend, &options))?;
            return Ok(Self::D1(Box::new(Mutex::new(store))));
        }
        let path = l.location.clone().unwrap_or_else(|| ":memory:".into());
        Ok(match &l.library {
            Some(lib) => Self::Library(Store::with_backend_and_options(
                DylibBackend::open(lib, &path)?,
                &options,
            )?),
            None => Self::Native(Store::open_with_options(&path, options)?),
        })
    }

    pub fn query(
        &self,
        query: &str,
        default_graphs: &[String],
        named_graphs: &[String],
    ) -> Result<QueryOutput> {
        let q = SparqlParser::new().parse_query(query)?;
        let options = QueryOptions {
            default_graph: graph_ids(default_graphs)?,
            named_graphs: graph_ids(named_graphs)?,
            ..QueryOptions::default()
        };
        Ok(sync_store!(self, s => s.query_output(q, &options), s => s.query_output(q, &options))?)
    }

    pub fn explain(&self, query: &str) -> Result<String> {
        Ok(match self {
            Db::Native(s) => s.explain(query)?,
            Db::Library(s) => s.explain(query)?,
            Db::D1(m) => m
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .explain(query)?,
        })
    }

    pub fn update(&self, update: &str, _using: &[String]) -> Result<()> {
        let u = SparqlParser::new().parse_update(update)?;
        Ok(sync_store!(self, s => s.update(u), s => s.update(u))?)
    }

    pub fn load(
        &self,
        format: RdfFormat,
        data: &[u8],
        graph: Option<&str>,
        bulk: bool,
    ) -> Result<()> {
        let mut parser = RdfParser::from_format(format);
        if let Some(g) = graph {
            parser = parser.with_default_graph(NamedNode::new(g)?);
        }
        match self {
            Db::Native(s) if bulk => {
                let mut loader = s.bulk_loader();
                loader.load_from_slice(parser, data)?;
                loader.commit()?;
            }
            Db::Library(s) if bulk => {
                let mut loader = s.bulk_loader();
                loader.load_from_slice(parser, data)?;
                loader.commit()?;
            }
            Db::Native(s) => s.load_from_slice(parser, data)?,
            Db::Library(s) => s.load_from_slice(parser, data)?,
            Db::D1(m) => {
                let s = m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                futures::executor::block_on(s.bulk_load(parser, data))?;
            }
        }
        Ok(())
    }

    pub fn optimize(&self) -> Result<()> {
        Ok(sync_store!(self, s => s.optimize(), s => s.optimize())?)
    }
}
