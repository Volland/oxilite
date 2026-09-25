//! The blocking store, a drop-in replacement for `oxigraph::store::Store`.
//!
// @lat: [[architecture#Crate layout]]

use crate::common::{
    explain_unsupported, parse_all, serialize_quads, serialize_triples, to_results,
};
pub use crate::common::{IntoQuery, IntoUpdate};
use oxilite_core::job::run_sync;
use oxilite_core::query::{compile_query, QueryJob, QueryOutput};
use oxilite_core::update::{plan_update_with, PlannedOp};
use oxilite_core::version::VersionedBackend;
use oxilite_core::{
    ops, Capabilities, Error, QueryOptions, Request, Result, Stats, StoreOptions, SyncBackend,
};
use oxrdf::{
    GraphNameRef, NamedNodeRef, NamedOrBlankNode, NamedOrBlankNodeRef, Quad, QuadRef, TermRef,
};
use oxrdfio::{RdfParser, RdfSerializer};
use spareval::QueryResults;
use std::io::{Read, Write};
use std::sync::{Arc, RwLock};

/// Upper bound on OWL 2 RL rule rounds (each round is one atomic request).
pub const MAX_MATERIALIZE_ROUNDS: usize = 1000;

/// Error type of storage operations (alias of [`oxilite_core::Error`]).
pub type StorageError = Error;
/// Error type of load operations.
pub type LoaderError = Error;
/// Error type of dump operations.
pub type SerializerError = Error;

#[cfg(feature = "rusqlite")]
type DefaultBackend = oxilite_rusqlite::RusqliteBackend;
#[cfg(not(feature = "rusqlite"))]
type DefaultBackend = NoBackend;

/// Placeholder backend type when no default backend feature is enabled.
#[cfg(not(feature = "rusqlite"))]
pub struct NoBackend;

#[cfg(not(feature = "rusqlite"))]
impl SyncBackend for NoBackend {
    fn execute(&self, _: &Request) -> Result<oxilite_core::Response> {
        Err(Error::unsupported("no backend"))
    }
    fn capabilities(&self) -> &Capabilities {
        unreachable!()
    }
}

struct Inner<B> {
    /// The backend, wrapped so every write of a versioned store opens a tick.
    backend: Arc<VersionedBackend<B>>,
    stats: RwLock<Stats>,
}

/// An RDF dataset stored in SQLite, queryable with SPARQL.
///
/// Mirrors `oxigraph::store::Store`. It is cheap to clone (shared handle).
pub struct Store<B: SyncBackend = DefaultBackend> {
    inner: Arc<Inner<B>>,
}

impl<B: SyncBackend> Clone for Store<B> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(feature = "rusqlite")]
impl Store {
    /// A new in-memory store.
    pub fn new() -> Result<Self> {
        Self::with_backend(oxilite_rusqlite::RusqliteBackend::memory()?)
    }

    /// Opens (or creates) a store in a SQLite database file.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::with_backend(oxilite_rusqlite::RusqliteBackend::open(path)?)
    }

    /// Opens a store with explicit options (used when the database is created).
    pub fn open_with_options(
        path: impl AsRef<std::path::Path>,
        options: StoreOptions,
    ) -> Result<Self> {
        Self::with_backend_and_options(oxilite_rusqlite::RusqliteBackend::open(path)?, &options)
    }

    /// Opens an existing store read-only.
    pub fn open_read_only(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let backend = oxilite_rusqlite::RusqliteBackend::open_read_only(path)?;
        let stats = run_sync(&backend, ops::stats_job(backend.capabilities()))?;
        Ok(Self::from_parts(backend, stats))
    }
}

#[cfg(feature = "dylib")]
impl Store<oxilite_dylib::DylibBackend> {
    /// Opens a store on `database` through the SQLite shared library at `library`.
    pub fn open_with_library(library: impl AsRef<std::path::Path>, database: &str) -> Result<Self> {
        Self::with_backend(oxilite_dylib::DylibBackend::open(library, database)?)
    }
}

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// Opens a store over any backend (creating the schema if needed).
    pub fn with_backend(backend: B) -> Result<Self> {
        Self::with_backend_and_options(backend, &StoreOptions::default())
    }

    pub fn with_backend_and_options(backend: B, options: &StoreOptions) -> Result<Self> {
        let stats = run_sync(&backend, ops::open_job(options, backend.capabilities()))?;
        Ok(Self::from_parts(backend, stats))
    }

    fn from_parts(backend: B, stats: Stats) -> Self {
        let caps = backend.capabilities().clone();
        let level = stats.version.level;
        Self {
            inner: Arc::new(Inner {
                backend: Arc::new(VersionedBackend::new(backend, &caps, level)),
                stats: RwLock::new(stats),
            }),
        }
    }

    /// The backend.
    pub fn backend(&self) -> &B {
        self.inner.backend.inner()
    }

    /// The backend as the store's operations see it: writes of a versioned store open a tick.
    pub(crate) fn versioned(&self) -> &VersionedBackend<B> {
        &self.inner.backend
    }

    /// Replaces the statistics (and the versioning level the backend applies).
    pub(crate) fn set_stats(&self, stats: Stats) {
        self.inner.backend.set_level(stats.version.level);
        *self
            .inner
            .stats
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stats;
    }

    pub(crate) fn caps(&self) -> &Capabilities {
        self.inner.backend.capabilities()
    }

    pub(crate) fn run<J: oxilite_core::Job>(&self, job: J) -> Result<J::Output> {
        run_sync(&*self.inner.backend, job)
    }

    pub(crate) fn evaluate(
        &self,
        query: &spargebra::Query,
        options: &QueryOptions,
    ) -> Result<QueryOutput> {
        let options = &*self.resolve_versions(query, options)?;
        let compiled = {
            let stats = self
                .inner
                .stats
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            compile_query(query, &stats, self.caps(), options)
        };
        match compiled {
            Ok(c) => self.run(QueryJob::new(c, self.caps().clone())),
            Err(e) if e.is_unsupported() && !oxilite_core::version::query_reads_history(query) => {
                let stats = self.stats();
                crate::partial::evaluate(Arc::clone(&self.inner.backend), query, &stats, options)
            }
            Err(e) => Err(e),
        }
    }

    /// Executes a SPARQL query.
    pub fn query(&self, query: impl IntoQuery) -> Result<QueryResults<'static>> {
        self.query_opt(query, QueryOptions::default())
    }

    /// Executes a SPARQL query with options.
    pub fn query_opt(
        &self,
        query: impl IntoQuery,
        options: QueryOptions,
    ) -> Result<QueryResults<'static>> {
        let q = query.into_query()?;
        Ok(to_results(self.evaluate(&q, &options)?))
    }

    /// Executes a SPARQL query, returning decoded output.
    pub fn query_output(
        &self,
        query: impl IntoQuery,
        options: &QueryOptions,
    ) -> Result<QueryOutput> {
        let q = query.into_query()?;
        self.evaluate(&q, options)
    }

    /// Returns the SQL a query compiles to (or why it is evaluated by the fallback).
    pub fn explain(&self, query: impl IntoQuery) -> Result<String> {
        self.explain_opt(query, &QueryOptions::default())
    }

    pub fn explain_opt(&self, query: impl IntoQuery, options: &QueryOptions) -> Result<String> {
        let q = query.into_query()?;
        let options = &*self.resolve_versions(&q, options)?;
        let stats = self
            .inner
            .stats
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(match compile_query(&q, &stats, self.caps(), options) {
            Ok(c) => c.explain(),
            Err(e) if e.is_unsupported() => {
                let (_, jobs) = crate::partial::plan(&q, &stats, self.caps(), options);
                let mut out = explain_unsupported(&e);
                let mut names: Vec<_> = jobs.keys().cloned().collect();
                names.sort();
                if !names.is_empty() {
                    out.push_str(&format!(
                        "\n-- {} subquery(ies) still run as SQL:",
                        names.len()
                    ));
                    for n in names {
                        out.push_str(&format!("\n-- <{n}>\n{}", jobs[&n].sql));
                    }
                }
                out
            }
            Err(e) => return Err(e),
        })
    }

    /// Quads matching a pattern (`graph_name = None` means any graph, default included).
    pub fn quads_for_pattern(
        &self,
        subject: Option<NamedOrBlankNodeRef<'_>>,
        predicate: Option<NamedNodeRef<'_>>,
        object: Option<TermRef<'_>>,
        graph_name: Option<GraphNameRef<'_>>,
    ) -> QuadIter {
        QuadIter::new(self.run(ops::scan_job(
            subject,
            predicate,
            object,
            graph_name,
            self.caps(),
        )))
    }

    /// All quads.
    pub fn iter(&self) -> QuadIter {
        self.quads_for_pattern(None, None, None, None)
    }

    pub fn contains<'a>(&self, quad: impl Into<QuadRef<'a>>) -> Result<bool> {
        self.run(ops::contains_job(quad.into()))
    }

    pub fn len(&self) -> Result<usize> {
        self.run(ops::len_job())
    }

    pub fn is_empty(&self) -> Result<bool> {
        self.run(ops::is_empty_job())
    }

    /// Describes how an update runs: the SQL of each compiled operation, or why it needs the
    /// fallback.
    pub fn explain_update(&self, update: impl IntoUpdate) -> Result<String> {
        let update = update.into_update()?;
        let plan = plan_update_with(
            &update,
            &self.stats(),
            self.caps(),
            &QueryOptions::default(),
        )?;
        Ok(oxilite_core::update::explain_plan(&plan))
    }

    /// Executes a SPARQL update atomically.
    pub fn update(&self, update: impl IntoUpdate) -> Result<()> {
        let update = update.into_update()?;
        self.update_inner(&update)?;
        if oxilite_core::reason::update_touches_schema(&update) {
            self.reload_stats()?;
        }
        Ok(())
    }

    fn update_inner(&self, update: &spargebra::Update) -> Result<()> {
        let mut plan =
            plan_update_with(update, &self.stats(), self.caps(), &QueryOptions::default())?;
        if plan.iter().all(|p| matches!(p, PlannedOp::Sql(_))) {
            let stmts = plan
                .iter()
                .flat_map(|p| match p {
                    PlannedOp::Sql(s) => s.clone(),
                    PlannedOp::Fallback(..) => Vec::new(),
                })
                .collect::<Vec<_>>();
            if stmts.is_empty() {
                return Ok(());
            }
            match self.inner.backend.execute(&Request::atomic(stmts)) {
                Ok(_) => return Ok(()),
                // A runtime guard rejected the SQL plan (e.g. a computed non-integer value in a
                // template): the batch rolled back, evaluate DELETE/INSERT with the fallback.
                Err(e) if e.is_unsupported() => {
                    for (i, op) in update.operations.iter().enumerate() {
                        if matches!(op, spargebra::GraphUpdateOperation::DeleteInsert { .. }) {
                            plan[i] = PlannedOp::Fallback(i, e.to_string());
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }
        // Mixed plan: an interactive transaction with fallback evaluation.
        let backend = &*self.inner.backend;
        backend.begin()?;
        let result = (|| {
            for p in plan {
                match p {
                    PlannedOp::Sql(s) => {
                        if !s.is_empty() {
                            backend.execute(&Request::atomic(s))?;
                        }
                    }
                    PlannedOp::Fallback(i, _) => {
                        let (deletes, inserts) = oxilite_core::fallback::delete_insert(
                            backend,
                            &update.operations[i],
                            update.base_iri.as_ref(),
                        )?;
                        if !deletes.is_empty() {
                            self.run(ops::remove_job(
                                deletes.iter().map(Quad::as_ref),
                                self.caps(),
                            ))?;
                        }
                        if !inserts.is_empty() {
                            self.run(ops::insert_job(
                                inserts.iter().map(Quad::as_ref),
                                self.caps(),
                            ))?;
                        }
                    }
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => backend.commit(),
            Err(e) => {
                let _ = backend.rollback();
                Err(e)
            }
        }
    }

    /// Loads a document atomically (blank nodes are renamed, like Oxigraph).
    pub fn load_from_reader(&self, parser: impl Into<RdfParser>, reader: impl Read) -> Result<()> {
        let quads = parse_all(parser.into(), reader, None)?;
        self.run(ops::insert_job(quads.iter().map(Quad::as_ref), self.caps()))?;
        self.after_write(quads.iter().map(Quad::as_ref))
    }

    pub fn load_from_slice(
        &self,
        parser: impl Into<RdfParser>,
        slice: &(impl AsRef<[u8]> + ?Sized),
    ) -> Result<()> {
        self.load_from_reader(parser, slice.as_ref())
    }

    /// Inserts a quad; returns `true` if it was not already present.
    pub fn insert<'a>(&self, quad: impl Into<QuadRef<'a>>) -> Result<bool> {
        let quad = quad.into();
        let added = self.run(ops::insert_job([quad], self.caps()))? > 0;
        self.after_write([quad])?;
        Ok(added)
    }

    /// Inserts quads atomically.
    pub fn extend(&self, quads: impl IntoIterator<Item = impl Into<Quad>>) -> Result<()> {
        let quads: Vec<Quad> = quads.into_iter().map(Into::into).collect();
        if !quads.is_empty() {
            self.run(ops::insert_job(quads.iter().map(Quad::as_ref), self.caps()))?;
        }
        self.after_write(quads.iter().map(Quad::as_ref))
    }

    /// Removes a quad; returns `true` if it was present.
    pub fn remove<'a>(&self, quad: impl Into<QuadRef<'a>>) -> Result<bool> {
        let quad = quad.into();
        let removed = self.run(ops::remove_job([quad], self.caps()))? > 0;
        self.after_write([quad])?;
        Ok(removed)
    }

    /// Reloads the in-memory reasoning facts after a write that changed schema triples (the
    /// closure itself was recomputed inside the write's transaction).
    fn after_write<'a>(&self, quads: impl IntoIterator<Item = QuadRef<'a>>) -> Result<()> {
        if quads.into_iter().any(oxilite_core::reason::is_schema_quad) {
            self.reload_stats()?;
        }
        Ok(())
    }

    pub(crate) fn reload_stats(&self) -> Result<()> {
        let stats = self.run(ops::stats_job(self.caps()))?;
        self.set_stats(stats);
        Ok(())
    }

    /// Computes the OWL 2 RL closure of the whole dataset into a separate inference table,
    /// replacing previous inferences; returns the number of inferred triples. Queries see them
    /// with `QueryOptions::include_inferred`. Runs SQL rules on every backend.
    pub fn materialize(&self) -> Result<u64> {
        self.run(ops::materialize_job(MAX_MATERIALIZE_ROUNDS, self.caps()))
    }

    /// Like [`Self::materialize`], computed in memory by the `reasonable` reasoner: much
    /// faster on large datasets, with the same results (see the agreement tests).
    #[cfg(feature = "reasonable")]
    pub fn materialize_with_reasonable(&self) -> Result<u64> {
        oxilite_reason::materialize(&*self.inner.backend)
    }

    /// Removes every materialized inference.
    pub fn clear_inferences(&self) -> Result<()> {
        self.run(ops::clear_inferences_job())
    }

    /// Removes the inferences of one producer (`"owl2rl"`, or a Datalog `Options::producer`),
    /// keeping those another producer also derived.
    pub fn clear_inferences_of(&self, producer: &str) -> Result<()> {
        self.run(ops::clear_inferences_of_job(producer))
    }

    /// The producers that derived a materialized quad: empty when it is not an inference.
    pub fn inference_producers<'a>(&self, quad: impl Into<QuadRef<'a>>) -> Result<Vec<String>> {
        self.run(ops::inference_producers_job(quad.into()))
    }

    /// Dumps the whole dataset (dataset formats: N-Quads, TriG).
    pub fn dump_to_writer<W: Write>(
        &self,
        serializer: impl Into<RdfSerializer>,
        writer: W,
    ) -> Result<W> {
        let quads = self.iter().collect::<Result<Vec<_>>>()?;
        serialize_quads(serializer.into(), writer, &quads)
    }

    /// Dumps one graph (graph formats: N-Triples, Turtle, RDF/XML…).
    pub fn dump_graph_to_writer<'a, W: Write>(
        &self,
        from_graph_name: impl Into<GraphNameRef<'a>>,
        serializer: impl Into<RdfSerializer>,
        writer: W,
    ) -> Result<W> {
        let quads = self
            .quads_for_pattern(None, None, None, Some(from_graph_name.into()))
            .collect::<Result<Vec<_>>>()?;
        serialize_triples(serializer.into(), writer, &quads)
    }

    pub fn named_graphs(&self) -> GraphNameIter {
        GraphNameIter {
            inner: match self.run(ops::named_graphs_job(self.caps())) {
                Ok(g) => g.into_iter().map(Ok).collect::<Vec<_>>().into_iter(),
                Err(e) => vec![Err(e)].into_iter(),
            },
        }
    }

    pub fn contains_named_graph<'a>(
        &self,
        graph_name: impl Into<NamedOrBlankNodeRef<'a>>,
    ) -> Result<bool> {
        self.run(ops::contains_named_graph_job(graph_name.into()))
    }

    /// Adds a named graph; returns `true` if it was new.
    pub fn insert_named_graph<'a>(
        &self,
        graph_name: impl Into<NamedOrBlankNodeRef<'a>>,
    ) -> Result<bool> {
        self.run(ops::insert_named_graph_job(graph_name.into(), self.caps()))
    }

    pub fn clear_graph<'a>(&self, graph_name: impl Into<GraphNameRef<'a>>) -> Result<()> {
        self.run(ops::clear_graph_job(graph_name.into()))?;
        self.reload_stats()
    }

    /// Removes a named graph and its quads; returns `true` if it existed.
    pub fn remove_named_graph<'a>(
        &self,
        graph_name: impl Into<NamedOrBlankNodeRef<'a>>,
    ) -> Result<bool> {
        let existed = self.run(ops::remove_named_graph_job(graph_name.into()))?;
        self.reload_stats()?;
        Ok(existed)
    }

    pub fn clear(&self) -> Result<()> {
        self.run(ops::clear_job())?;
        self.reload_stats()
    }

    /// No-op: SQLite commits are durable (kept for Oxigraph API compatibility).
    pub fn flush(&self) -> Result<()> {
        Ok(())
    }

    /// Refreshes the planner statistics (replaces RocksDB compaction).
    pub fn optimize(&self) -> Result<()> {
        let stats = self.run(ops::optimize_job(self.caps()))?;
        self.set_stats(stats);
        Ok(())
    }

    /// Current planner statistics.
    pub fn stats(&self) -> Stats {
        self.inner
            .stats
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Writes a consistent copy of the database to `target` (`VACUUM INTO`).
    pub fn backup(&self, target: impl AsRef<std::path::Path>) -> Result<()> {
        let path = target.as_ref().to_string_lossy().into_owned();
        self.inner
            .backend
            .execute(&Request::read(vec![oxilite_core::Statement::new(format!(
                "VACUUM INTO {}",
                oxilite_core::sql::sql_str(&path)
            ))]))?;
        Ok(())
    }

    /// A loader for large imports, committing in chunks (not atomic, like Oxigraph's).
    pub fn bulk_loader(&self) -> BulkLoader<'_, B> {
        BulkLoader {
            store: self,
            on_parse_error: None,
            buffer: Vec::new(),
            chunk: 50_000,
            loaded: 0,
        }
    }

    /// Checks internal consistency: SQLite integrity and that every stored id is decodable.
    pub fn validate(&self) -> Result<()> {
        let hashed = "(1, 2, 3, 4, 5, 10)";
        let mut checks = vec![oxilite_core::Statement::new("PRAGMA quick_check")];
        for c in ["s", "p", "o", "g"] {
            checks.push(oxilite_core::Statement::new(format!(
                "SELECT COUNT(*) FROM quads q WHERE (q.{c} >> 59) IN {hashed} AND NOT EXISTS (SELECT 1 FROM terms t WHERE t.id = q.{c})"
            )));
        }
        let r = self.inner.backend.execute(&Request::read(checks))?;
        let ok = r[0]
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|v| v.as_str())
            == Some("ok");
        if !ok {
            return Err(Error::corrupted(format!(
                "SQLite integrity check failed: {:?}",
                r[0].rows
            )));
        }
        for (i, c) in ["subject", "predicate", "object", "graph"]
            .iter()
            .enumerate()
        {
            let n = r[i + 1].rows[0][0].as_i64().unwrap_or(0);
            if n > 0 {
                return Err(Error::corrupted(format!(
                    "{n} quad(s) have a {c} id missing from the dictionary"
                )));
            }
        }
        Ok(())
    }
}

/// An iterator over quads (materialized: it sees a snapshot, like Oxigraph's).
pub struct QuadIter {
    inner: std::vec::IntoIter<Result<Quad>>,
}

impl QuadIter {
    fn new(r: Result<Vec<Quad>>) -> Self {
        Self {
            inner: match r {
                Ok(q) => q.into_iter().map(Ok).collect::<Vec<_>>().into_iter(),
                Err(e) => vec![Err(e)].into_iter(),
            },
        }
    }
}

impl Iterator for QuadIter {
    type Item = Result<Quad>;

    fn next(&mut self) -> Option<Result<Quad>> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// An iterator over named graphs.
pub struct GraphNameIter {
    inner: std::vec::IntoIter<Result<NamedOrBlankNode>>,
}

impl Iterator for GraphNameIter {
    type Item = Result<NamedOrBlankNode>;

    fn next(&mut self) -> Option<Result<NamedOrBlankNode>> {
        self.inner.next()
    }
}

type ParseErrorHandler = Box<dyn FnMut(oxrdfio::RdfParseError) -> Result<()>>;

/// Bulk loader: buffers quads and inserts them in chunked atomic requests; `commit()` flushes
/// the rest and refreshes statistics.
#[must_use]
pub struct BulkLoader<'a, B: SyncBackend> {
    store: &'a Store<B>,
    on_parse_error: Option<ParseErrorHandler>,
    buffer: Vec<Quad>,
    chunk: usize,
    loaded: u64,
}

impl<'a, B: SyncBackend + Send + Sync + 'static> BulkLoader<'a, B> {
    /// Accepted for compatibility; SQLite writes are single-threaded.
    pub fn with_num_threads(self, _num_threads: usize) -> Self {
        self
    }

    /// Number of quads per committed chunk.
    pub fn with_chunk_size(mut self, chunk: usize) -> Self {
        self.chunk = chunk.max(1);
        self
    }

    /// Called on parse errors; returning `Ok` skips the invalid statement.
    pub fn on_parse_error(
        mut self,
        callback: impl FnMut(oxrdfio::RdfParseError) -> std::result::Result<(), oxrdfio::RdfParseError>
            + 'static,
    ) -> Self {
        let mut callback = callback;
        self.on_parse_error = Some(Box::new(move |e| callback(e).map_err(Error::from)));
        self
    }

    /// Accepted for compatibility (no progress reporting).
    pub fn on_progress(self, _callback: impl Fn(u64) + 'static) -> Self {
        self
    }

    fn flush(&mut self, all: bool) -> Result<()> {
        while self.buffer.len() >= self.chunk || (all && !self.buffer.is_empty()) {
            let n = self.buffer.len().min(self.chunk);
            let chunk: Vec<Quad> = self.buffer.drain(..n).collect();
            let req = ops::insert_request(chunk.iter().map(Quad::as_ref), self.store.caps());
            self.store.inner.backend.execute(&req)?;
            self.loaded += n as u64;
        }
        Ok(())
    }

    pub fn load_from_reader(
        &mut self,
        parser: impl Into<RdfParser>,
        reader: impl Read,
    ) -> Result<()> {
        let handler = self
            .on_parse_error
            .as_mut()
            .map(|h| h.as_mut() as &mut dyn FnMut(_) -> Result<()>);
        let quads = parse_all(parser.into(), reader, handler)?;
        self.buffer.extend(quads);
        self.flush(false)
    }

    pub fn load_from_slice(
        &mut self,
        parser: impl Into<RdfParser>,
        slice: &(impl AsRef<[u8]> + ?Sized),
    ) -> Result<()> {
        self.load_from_reader(parser, slice.as_ref())
    }

    pub fn load_quads(&mut self, quads: impl IntoIterator<Item = impl Into<Quad>>) -> Result<()> {
        self.buffer.extend(quads.into_iter().map(Into::into));
        self.flush(false)
    }

    pub fn load_ok_quads<E: From<Error>>(
        &mut self,
        quads: impl IntoIterator<Item = std::result::Result<impl Into<Quad>, E>>,
    ) -> std::result::Result<(), E> {
        for q in quads {
            self.buffer.push(q?.into());
        }
        self.flush(false).map_err(E::from)
    }

    /// Writes the remaining quads and refreshes planner statistics.
    pub fn commit(mut self) -> Result<()> {
        self.flush(true)?;
        self.store.optimize()
    }
}
