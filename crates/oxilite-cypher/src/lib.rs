//! openCypher over oxilite.
//!
//! Property graphs are a view of the RDF 1.2 dataset: nodes are IRIs or blank nodes, labels
//! are `rdf:type`, node properties are literal triples, relationships are triples, and a
//! relationship's identity and properties live on an RDF 1.2 reifier. A Cypher statement is
//! parsed, lowered to SPARQL algebra (compiled to SQL by oxilite, with its planner,
//! reasoning and fallback), and what SQL cannot express — writes, lists, maps, `collect()` —
//! runs in Rust over the query's rows. Writes are applied as one atomic request.
//!
//! The same [`CypherJob`] drives the blocking store, the async store (Cloudflare D1) and the
//! wasm engine; see `oxilite::Store::cypher` for the usual entry point.
//!
// @lat: [[architecture#Property graph frontend]]
#![allow(clippy::type_complexity)]

pub mod ast;
mod error;
mod eval;
mod exec;
pub mod json;
mod lexer;
mod lower;
mod parser;
mod plan;
mod schema;
mod temporal;
mod validate;
mod value;
mod vocab;

pub use error::{CypherError, Result};
pub use exec::{CypherJob, CypherResult, CypherStep, StepInput, WriteStats};
pub use parser::parse;
pub use schema::{schema_query, Shapes as Schema};
pub use value::{Node, Params, Path, Relationship, TemporalKind, Value, RDF_JSON};
pub use vocab::Vocabulary;

use oxilite_core::job::Job;
use oxilite_core::query::{compile_query, QueryJob, QueryOutput};
use oxilite_core::{Capabilities, QueryOptions, Request, Response, Stats, Step};

/// How a property with several RDF values is read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MultiValue {
    /// As a list of the values (in value order).
    #[default]
    List,
    /// As its smallest value.
    First,
    /// As an error.
    Error,
}

/// Options of the Cypher frontend.
#[derive(Debug, Clone)]
pub struct CypherOptions {
    /// Label, relationship-type and property-key names ↔ IRIs.
    pub vocabulary: Vocabulary,
    pub multi_value: MultiValue,
    /// Maximum hops of an unbounded variable-length relationship (`*`, `*2..`).
    pub var_length_cap: usize,
    /// Maximum number of alternatives a `MATCH` may expand to.
    pub max_branches: usize,
    /// Maximum depth of an unbounded `shortestPath` search (one request per level).
    pub shortest_path_cap: usize,
    /// Give every created node an `rdf:type rdfs:Resource` triple, so nodes without labels,
    /// properties or relationships still exist (hidden from `labels()`).
    pub node_marker: bool,
    /// Tell parallel relationships apart in uniqueness checks (costs a reifier look-up per
    /// relationship pattern).
    pub reifier_uniqueness: bool,
    /// Check writes against the SHACL shapes stored in the dataset (loaded by each writing
    /// statement unless `schema` is given).
    pub shapes: bool,
    /// Shapes loaded beforehand (`Store::cypher_schema`): used when planning (required
    /// properties join without `OPTIONAL`), when reading (single-valued properties) and by the
    /// write guards, without a per-statement load.
    pub schema: Option<std::sync::Arc<Schema>>,
    /// Options of the SPARQL evaluation: reasoning (OWL/RDFS entailment), union default
    /// graph…
    pub query: QueryOptions,
}

impl Default for CypherOptions {
    fn default() -> Self {
        Self {
            vocabulary: Vocabulary::default(),
            multi_value: MultiValue::List,
            var_length_cap: 10,
            max_branches: 256,
            shortest_path_cap: 32,
            node_marker: true,
            reifier_uniqueness: false,
            shapes: true,
            schema: None,
            query: QueryOptions::default(),
        }
    }
}

/// Parses and plans a statement for a native backend.
pub fn prepare(query: &str, params: &Params, options: &CypherOptions) -> Result<CypherJob> {
    prepare_for(query, params, options, &Capabilities::default())
}

/// Parses and plans a statement for a backend.
pub fn prepare_for(
    query: &str,
    params: &Params,
    options: &CypherOptions,
    caps: &Capabilities,
) -> Result<CypherJob> {
    let ast = parse(query)?;
    validate::validate(&ast)?;
    let mut lw = lower::Lowerer::new(&options.vocabulary, params, options);
    let plan = plan::plan(&ast, &mut lw)?;
    Ok(CypherJob::new(
        plan,
        params.clone(),
        options.clone(),
        caps.clone(),
    ))
}

/// Runs a job with blocking callbacks: `query` evaluates SPARQL (with the store's
/// fallback), `sql` runs requests.
pub fn run_blocking(
    mut job: CypherJob,
    mut query: impl FnMut(&spargebra::Query, &QueryOptions) -> Result<QueryOutput>,
    mut sql: impl FnMut(&Request) -> Result<Response>,
) -> Result<CypherResult> {
    let mut input = None;
    loop {
        input = Some(match job.step(input)? {
            CypherStep::Query(q, o) => StepInput::Output(query(&q, &o)?),
            CypherStep::Sql(r) | CypherStep::Write(r) => StepInput::Response(sql(&r)?),
            CypherStep::Done(r) => return Ok(r),
        });
    }
}

enum Inner {
    None,
    Query(Box<QueryJob>),
    Direct,
}

/// A [`CypherJob`] as a core SQL [`Job`]: SPARQL parts are compiled with
/// [`compile_query`] (no Rust fallback), so it runs on any backend, including D1 and the
/// wasm engine.
pub struct SqlCypherJob {
    job: CypherJob,
    stats: Stats,
    caps: Capabilities,
    options: QueryOptions,
    inner: Inner,
    started: bool,
    error: Option<CypherError>,
}

impl SqlCypherJob {
    pub fn new(job: CypherJob, stats: Stats, caps: Capabilities, options: QueryOptions) -> Self {
        Self {
            job,
            stats,
            caps,
            options,
            inner: Inner::None,
            started: false,
            error: None,
        }
    }

    /// The Cypher error behind an [`oxilite_core::Error::Other`] returned by `step`.
    pub fn take_error(&mut self) -> Option<CypherError> {
        self.error.take()
    }

    fn cypher(&mut self, input: Option<StepInput>) -> oxilite_core::Result<Step<CypherResult>> {
        let step = match self.job.step(input) {
            Ok(s) => s,
            Err(CypherError::Store(e)) => return Err(e),
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(e);
                return Err(oxilite_core::Error::Other(msg));
            }
        };
        match step {
            CypherStep::Done(r) => Ok(Step::Done(r)),
            CypherStep::Sql(r) | CypherStep::Write(r) => {
                self.inner = Inner::Direct;
                Ok(Step::Execute(r))
            }
            CypherStep::Query(q, o) => {
                let mut options = self.options.clone();
                options.var_types.extend(o.var_types);
                let compiled = compile_query(&q, &self.stats, &self.caps, &options)?;
                let mut qj = QueryJob::new(compiled, self.caps.clone());
                match qj.step(None)? {
                    Step::Execute(r) => {
                        self.inner = Inner::Query(Box::new(qj));
                        Ok(Step::Execute(r))
                    }
                    Step::Done(out) => self.cypher(Some(StepInput::Output(out))),
                }
            }
        }
    }
}

impl Job for SqlCypherJob {
    type Output = CypherResult;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<CypherResult>> {
        if !self.started {
            self.started = true;
            return self.cypher(None);
        }
        match std::mem::replace(&mut self.inner, Inner::None) {
            Inner::Direct => self.cypher(response.map(StepInput::Response)),
            Inner::Query(mut qj) => match qj.step(response)? {
                Step::Execute(r) => {
                    self.inner = Inner::Query(qj);
                    Ok(Step::Execute(r))
                }
                Step::Done(out) => self.cypher(Some(StepInput::Output(out))),
            },
            Inner::None => Err(oxilite_core::Error::Other(
                "Cypher job resumed after completion".into(),
            )),
        }
    }
}
