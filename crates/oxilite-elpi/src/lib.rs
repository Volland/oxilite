//! λProlog programs over oxilite, run by [lean-elpi](https://github.com/Volland/lean-elpi), the
//! ELPI dialect implemented in Lean 4.
//!
//! A program reads the store through two host builtins answered by pattern scans, so only what
//! it asks for is read:
//!
//! - `ox.triple S P O`: the triples of the scope (graphs merged);
//! - `ox.quad S P O G`: the quads, with `default-graph` for the default graph.
//!
//! RDF terms are typed ELPI values (`(iri "…")`, `(str "…")`, `(num 42)`, `(lang "…" "fr")`,
//! `(lit Lex Datatype)`, `(bnode "…")`, see [`term`]). A program can then be used three ways:
//!
//! - [`query`]: solve any goal and get its bindings back as RDF terms;
//! - [`check`]: collect what `ox.violation Severity Name Message Focus` reports;
//! - [`materialize`]: store what `ox.conclude (triple S P O)` concludes as inferences of a
//!   producer (`elpi` by default), queried with `include_inferred` like OWL 2 RL, Datalog and N3.
//!
//! The engine is the native `libleanelpi`, linked by the `native` feature (set `LEANELPI_DIST`).
//! Without it the crate still builds, and every run fails with [`ElpiError::Unsupported`].
//! Host calls are synchronous, so programs run on blocking backends (not Cloudflare D1).
//!
// @lat: [[architecture#ELPI reasoning]]

mod engine;
mod error;
pub mod host;
mod run;
pub mod term;

pub use engine::{available, version};
pub use error::{ElpiError, Result};
pub use run::{
    Binding, MaterializeStats, Options, QueryResult, Solution, Violation, CONCLUDE_GOAL,
    ELPI_PRODUCER, PRELUDE, VIOLATION_GOAL,
};

use oxilite_core::ops::{reasoning_input_job, ReasoningInput};
use oxilite_core::{run_sync, SyncBackend};
use oxrdf::{GraphName, Quad};

/// A lookup answering host builtins with scans of `backend`.
fn scanner<B: SyncBackend>(
    backend: &B,
) -> impl FnMut(&ReasoningInput<'_>) -> oxilite_core::Result<Vec<Quad>> + '_ {
    let caps = backend.capabilities().clone();
    move |input| run_sync(backend, reasoning_input_job(input, &caps))
}

/// Solves `goal` against `program` over the store.
// @tg: implements:: [[openspec:elpi-programs#Goal queries]] {since: "2026-11-03"}
pub fn query<B: SyncBackend>(
    backend: &B,
    program: &str,
    goal: &str,
    options: &Options,
) -> Result<QueryResult> {
    run::query(program, goal, options, &mut scanner(backend))
}

/// Every violation `ox.violation` reports over the store.
pub fn check<B: SyncBackend>(
    backend: &B,
    program: &str,
    options: &Options,
) -> Result<Vec<Violation>> {
    run::check(program, options, &mut scanner(backend))
}

/// Stores what `ox.conclude` concludes in `quads_inf` (default graph) as inferences of
/// `options.producer`, replacing that producer's earlier inferences in one atomic request.
// @tg: implements:: [[openspec:elpi-programs#Materialized conclusions]] {since: "2026-11-03"}
pub fn materialize<B: SyncBackend>(
    backend: &B,
    program: &str,
    options: &Options,
) -> Result<MaterializeStats> {
    let (triples, unstorable, rows_read) =
        run::conclusions(program, options, &mut scanner(backend))?;
    let quads: Vec<Quad> = triples
        .into_iter()
        .map(|t| t.in_graph(GraphName::DefaultGraph))
        .collect();
    let (request, inferred) =
        oxilite_core::reason::inference_replace(&options.producer, &quads, backend.capabilities());
    backend.execute(&request)?;
    Ok(MaterializeStats {
        inferred,
        unstorable,
        rows_read,
        producer: options.producer.clone(),
    })
}
