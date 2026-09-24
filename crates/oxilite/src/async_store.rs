//! The async store: the same API as [`crate::store::Store`] over [`AsyncBackend`]s
//! (Cloudflare D1, remote engines). Futures are not `Send` (Workers are single-threaded).
//!
// @lat: [[architecture#Crate layout]]

use crate::common::{
    parse_all, serialize_quads, serialize_triples, to_results, IntoQuery, IntoUpdate,
};
use oxilite_core::job::run_async;
use oxilite_core::query::{compile_query, QueryJob, QueryOutput};
use oxilite_core::update::{plan_update_with, PlannedOp};
use oxilite_core::{
    ops, AsyncBackend, Capabilities, Error, QueryOptions, Request, Result, Stats, StoreOptions,
};
use oxrdf::{
    GraphNameRef, NamedNodeRef, NamedOrBlankNode, NamedOrBlankNodeRef, Quad, QuadRef, TermRef,
};
use oxrdfio::{RdfParser, RdfSerializer};
use spareval::QueryResults;
use std::cell::RefCell;
use std::io::{Read, Write};

/// An RDF dataset in a SQLite-compatible engine reached asynchronously.
pub struct AsyncStore<B: AsyncBackend> {
    pub(crate) backend: B,
    pub(crate) stats: RefCell<Stats>,
}

impl<B: AsyncBackend> AsyncStore<B> {
    /// Opens the store (creates the schema idempotently, loads statistics).
    pub async fn open(backend: B) -> Result<Self> {
        Self::open_with_options(backend, &StoreOptions::default()).await
    }

    pub async fn open_with_options(backend: B, options: &StoreOptions) -> Result<Self> {
        let stats = run_async(&backend, ops::open_job(options, backend.capabilities())).await?;
        Ok(Self {
            backend,
            stats: RefCell::new(stats),
        })
    }

    /// Opens a store whose schema was already applied (e.g. by a D1 migration): no DDL.
    pub async fn open_existing(backend: B) -> Result<Self> {
        let stats = run_async(&backend, ops::stats_job(backend.capabilities())).await?;
        Ok(Self {
            backend,
            stats: RefCell::new(stats),
        })
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub(crate) fn caps(&self) -> &Capabilities {
        self.backend.capabilities()
    }

    /// Executes a SPARQL query.
    pub async fn query(&self, query: impl IntoQuery) -> Result<QueryResults<'static>> {
        self.query_opt(query, QueryOptions::default()).await
    }

    pub async fn query_opt(
        &self,
        query: impl IntoQuery,
        options: QueryOptions,
    ) -> Result<QueryResults<'static>> {
        Ok(to_results(self.query_output(query, &options).await?))
    }

    /// Executes a SPARQL query, returning decoded output. Queries that need the sync fallback
    /// evaluator return [`Error::Unsupported`].
    pub async fn query_output(
        &self,
        query: impl IntoQuery,
        options: &QueryOptions,
    ) -> Result<QueryOutput> {
        let q = query.into_query()?;
        let compiled = compile_query(&q, &self.stats.borrow(), self.caps(), options)?;
        run_async(&self.backend, QueryJob::new(compiled, self.caps().clone())).await
    }

    pub fn explain(&self, query: impl IntoQuery) -> Result<String> {
        let q = query.into_query()?;
        Ok(
            match compile_query(
                &q,
                &self.stats.borrow(),
                self.caps(),
                &QueryOptions::default(),
            ) {
                Ok(c) => c.explain(),
                Err(e) if e.is_unsupported() => {
                    format!("-- oxilite: unsupported on this backend: {e}")
                }
                Err(e) => return Err(e),
            },
        )
    }

    /// Describes how an update runs: the SQL of each compiled operation, or why it needs the
    /// fallback.
    pub fn explain_update(&self, update: impl IntoUpdate) -> Result<String> {
        let update = update.into_update()?;
        let plan = plan_update_with(
            &update,
            &self.stats.borrow(),
            self.caps(),
            &QueryOptions::default(),
        )?;
        Ok(oxilite_core::update::explain_plan(&plan))
    }

    /// Executes a SPARQL update as one atomic request.
    pub async fn update(&self, update: impl IntoUpdate) -> Result<()> {
        let update = update.into_update()?;
        let mut stmts = Vec::new();
        for p in plan_update_with(
            &update,
            &self.stats.borrow(),
            self.caps(),
            &QueryOptions::default(),
        )? {
            match p {
                PlannedOp::Sql(s) => stmts.extend(s),
                PlannedOp::Fallback(_, what) => {
                    return Err(Error::unsupported(format!("{what} on an async backend")))
                }
            }
        }
        if !stmts.is_empty() {
            self.backend.execute(&Request::atomic(stmts)).await?;
        }
        if oxilite_core::reason::update_touches_schema(&update) {
            self.reload_stats().await?;
        }
        Ok(())
    }

    /// Reloads statistics and the in-memory reasoning facts.
    pub(crate) async fn reload_stats(&self) -> Result<()> {
        let stats = run_async(&self.backend, ops::stats_job(self.caps())).await?;
        *self.stats.borrow_mut() = stats;
        Ok(())
    }

    async fn after_write<'a>(&self, quads: impl IntoIterator<Item = QuadRef<'a>>) -> Result<()> {
        if quads.into_iter().any(oxilite_core::reason::is_schema_quad) {
            self.reload_stats().await?;
        }
        Ok(())
    }

    /// Computes the OWL 2 RL closure into the inference table (SQL rules, one request per
    /// round); returns the number of inferred triples.
    pub async fn materialize(&self) -> Result<u64> {
        run_async(
            &self.backend,
            ops::materialize_job(crate::store::MAX_MATERIALIZE_ROUNDS, self.caps()),
        )
        .await
    }

    /// Removes every materialized inference.
    pub async fn clear_inferences(&self) -> Result<()> {
        run_async(&self.backend, ops::clear_inferences_job()).await
    }

    pub async fn quads_for_pattern(
        &self,
        subject: Option<NamedOrBlankNodeRef<'_>>,
        predicate: Option<NamedNodeRef<'_>>,
        object: Option<TermRef<'_>>,
        graph_name: Option<GraphNameRef<'_>>,
    ) -> Result<Vec<Quad>> {
        run_async(
            &self.backend,
            ops::scan_job(subject, predicate, object, graph_name, self.caps()),
        )
        .await
    }

    pub async fn contains<'a>(&self, quad: impl Into<QuadRef<'a>>) -> Result<bool> {
        run_async(&self.backend, ops::contains_job(quad.into())).await
    }

    pub async fn len(&self) -> Result<usize> {
        run_async(&self.backend, ops::len_job()).await
    }

    pub async fn is_empty(&self) -> Result<bool> {
        run_async(&self.backend, ops::is_empty_job()).await
    }

    pub async fn insert<'a>(&self, quad: impl Into<QuadRef<'a>>) -> Result<bool> {
        let quad = quad.into();
        let added = run_async(&self.backend, ops::insert_job([quad], self.caps())).await? > 0;
        self.after_write([quad]).await?;
        Ok(added)
    }

    pub async fn extend(&self, quads: impl IntoIterator<Item = impl Into<Quad>>) -> Result<()> {
        let quads: Vec<Quad> = quads.into_iter().map(Into::into).collect();
        if !quads.is_empty() {
            run_async(
                &self.backend,
                ops::insert_job(quads.iter().map(Quad::as_ref), self.caps()),
            )
            .await?;
        }
        self.after_write(quads.iter().map(Quad::as_ref)).await
    }

    pub async fn remove<'a>(&self, quad: impl Into<QuadRef<'a>>) -> Result<bool> {
        let quad = quad.into();
        let removed = run_async(&self.backend, ops::remove_job([quad], self.caps())).await? > 0;
        self.after_write([quad]).await?;
        Ok(removed)
    }

    /// Loads a document. Atomic when it fits in one request of the backend; use
    /// [`Self::bulk_load`] for large documents.
    pub async fn load_from_reader(
        &self,
        parser: impl Into<RdfParser>,
        reader: impl Read,
    ) -> Result<()> {
        let quads = parse_all(parser.into(), reader, None)?;
        let mut req = ops::insert_request(quads.iter().map(Quad::as_ref), self.caps());
        let schema = quads
            .iter()
            .any(|q| oxilite_core::reason::is_schema_quad(q.as_ref()));
        req.statements
            .extend(ops::schema_refresh_for(quads.iter().map(Quad::as_ref)));
        if req.statements.len() > self.caps().max_statements {
            return Err(Error::Other(format!(
                "document needs {} statements, more than one atomic request allows ({}); use bulk_load",
                req.statements.len(),
                self.caps().max_statements
            )));
        }
        if !req.statements.is_empty() {
            self.backend.execute(&req).await?;
        }
        if schema {
            self.reload_stats().await?;
        }
        Ok(())
    }

    pub async fn load_from_slice(
        &self,
        parser: impl Into<RdfParser>,
        slice: &(impl AsRef<[u8]> + ?Sized),
    ) -> Result<()> {
        self.load_from_reader(parser, slice.as_ref()).await
    }

    /// Loads a large document in chunks that each fit one request (not atomic), then
    /// refreshes statistics.
    pub async fn bulk_load(&self, parser: impl Into<RdfParser>, reader: impl Read) -> Result<()> {
        let quads = parse_all(parser.into(), reader, None)?;
        let caps = self.caps().clone();
        let mut chunk = 2_000usize;
        let mut start = 0;
        while start < quads.len() {
            let end = (start + chunk).min(quads.len());
            let req = ops::insert_request(quads[start..end].iter().map(Quad::as_ref), &caps);
            if req.statements.len() > caps.max_statements && chunk > 1 {
                chunk /= 2;
                continue;
            }
            self.backend.execute(&req).await?;
            start = end;
        }
        self.optimize().await
    }

    pub async fn dump_to_writer<W: Write>(
        &self,
        serializer: impl Into<RdfSerializer>,
        writer: W,
    ) -> Result<W> {
        let quads = self.quads_for_pattern(None, None, None, None).await?;
        serialize_quads(serializer.into(), writer, &quads)
    }

    pub async fn dump_graph_to_writer<'a, W: Write>(
        &self,
        from_graph_name: impl Into<GraphNameRef<'a>>,
        serializer: impl Into<RdfSerializer>,
        writer: W,
    ) -> Result<W> {
        let quads = self
            .quads_for_pattern(None, None, None, Some(from_graph_name.into()))
            .await?;
        serialize_triples(serializer.into(), writer, &quads)
    }

    pub async fn named_graphs(&self) -> Result<Vec<NamedOrBlankNode>> {
        run_async(&self.backend, ops::named_graphs_job(self.caps())).await
    }

    pub async fn contains_named_graph<'a>(
        &self,
        graph_name: impl Into<NamedOrBlankNodeRef<'a>>,
    ) -> Result<bool> {
        run_async(
            &self.backend,
            ops::contains_named_graph_job(graph_name.into()),
        )
        .await
    }

    pub async fn insert_named_graph<'a>(
        &self,
        graph_name: impl Into<NamedOrBlankNodeRef<'a>>,
    ) -> Result<bool> {
        run_async(
            &self.backend,
            ops::insert_named_graph_job(graph_name.into(), self.caps()),
        )
        .await
    }

    pub async fn clear_graph<'a>(&self, graph_name: impl Into<GraphNameRef<'a>>) -> Result<()> {
        run_async(&self.backend, ops::clear_graph_job(graph_name.into())).await?;
        self.reload_stats().await
    }

    pub async fn remove_named_graph<'a>(
        &self,
        graph_name: impl Into<NamedOrBlankNodeRef<'a>>,
    ) -> Result<bool> {
        let existed = run_async(
            &self.backend,
            ops::remove_named_graph_job(graph_name.into()),
        )
        .await?;
        self.reload_stats().await?;
        Ok(existed)
    }

    pub async fn clear(&self) -> Result<()> {
        run_async(&self.backend, ops::clear_job()).await?;
        self.reload_stats().await
    }

    /// Refreshes planner statistics.
    pub async fn optimize(&self) -> Result<()> {
        let stats = run_async(&self.backend, ops::optimize_job(self.caps())).await?;
        *self.stats.borrow_mut() = stats;
        Ok(())
    }
}
