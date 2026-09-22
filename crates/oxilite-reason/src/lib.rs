//! OWL 2 RL materialization with [`reasonable`], for native backends.
//!
//! `materialize` reads every asserted triple (all graphs, merged), runs the `reasonable`
//! Datalog engine in memory and writes the new conclusions into `quads_inf` (graph 0), exactly
//! where the SQL rules of `oxilite_core::reason` put theirs. It is much faster than the SQL
//! fixpoint on large data but needs the whole dataset in memory, so it is not used on D1.
//!
// @lat: [[architecture#Reasoning]]

use oxilite_core::encoding::{EncodedRows, DEFAULT_GRAPH_ID};
use oxilite_core::writer::{insert_statements_into, term_statements};
use oxilite_core::{ops, run_sync, Request, Result, Statement, SyncBackend};
use oxrdf::{GraphNameRef, QuadRef, Triple};
use reasonable::reasoner::Reasoner;
use std::collections::HashSet;

/// The triples `reasonable` infers from the asserted triples (every graph merged), minus
/// the triples asserted in the default graph.
pub fn infer<B: SyncBackend>(backend: &B) -> Result<Vec<Triple>> {
    let caps = backend.capabilities().clone();
    let quads = run_sync(backend, ops::scan_job(None, None, None, None, &caps))?;
    let default: HashSet<Triple> = quads
        .iter()
        .filter(|q| q.graph_name.is_default_graph())
        .map(|q| Triple::from(q.clone()))
        .collect();
    let input: Vec<Triple> = quads
        .into_iter()
        .map(Triple::from)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut reasoner = Reasoner::new();
    reasoner.load_triples(input);
    reasoner.reason();
    // `reasonable` reports consistency diagnostics (e.g. `cax-dw`) without failing; like the
    // SQL rules, materialization keeps going on inconsistent data.
    let mut out: Vec<Triple> = reasoner
        .get_triples()
        .into_iter()
        .filter(|t| !default.contains(t))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    out.sort_by_cached_key(ToString::to_string);
    Ok(out)
}

/// Replaces `quads_inf` with the inferences of `reasonable`; returns their number.
pub fn materialize<B: SyncBackend>(backend: &B) -> Result<u64> {
    let caps = backend.capabilities().clone();
    let triples = infer(backend)?;
    let mut rows = EncodedRows::default();
    let ids: Vec<[i64; 4]> = triples
        .iter()
        .map(|t| {
            let [s, p, o, _] = rows.quad(QuadRef::new(
                t.subject.as_ref(),
                t.predicate.as_ref(),
                t.object.as_ref(),
                GraphNameRef::DefaultGraph,
            ));
            [s, p, o, DEFAULT_GRAPH_ID]
        })
        .collect();
    rows.dedup();
    let mut stmts = vec![Statement::new("DELETE FROM quads_inf")];
    stmts.extend(term_statements(&rows, &caps));
    stmts.extend(insert_statements_into("quads_inf", &ids, &caps));
    backend.execute(&Request::atomic(stmts))?;
    Ok(ids.len() as u64)
}
