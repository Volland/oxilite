//! SPARQL UPDATE planning.
//!
//! Operations become SQL statements appended to one atomic request, so a whole update
//! request is applied as a single transaction (one D1 batch). Operations that cannot be
//! compiled are reported as [`PlannedOp::Fallback`] for sync backends.
//!
// @lat: [[architecture#Updates and atomicity]]

use crate::encoding::{graph_id, named_node_id, EncodedRows, DEFAULT_GRAPH_ID};
use crate::error::{Error, Result};
use crate::sql::{Capabilities, Statement};
use crate::writer::{term_statements, EncodedQuads};
use oxrdf::{BlankNode, GraphName, NamedOrBlankNode, Quad, Term};
use spargebra::algebra::GraphTarget;
use spargebra::term::{GroundQuad, GroundTerm};
use spargebra::{GraphUpdateOperation, Update};
use std::collections::HashMap;

/// A planned update operation.
#[derive(Debug)]
pub enum PlannedOp {
    /// SQL statements, to run in order inside the update's transaction.
    Sql(Vec<Statement>),
    /// Operation `index` of the update must be evaluated by the fallback (sync backends).
    Fallback(usize, String),
}

fn bnode_map(t: &Term, map: &mut HashMap<BlankNode, BlankNode>) -> Term {
    match t {
        Term::BlankNode(b) => map.entry(b.clone()).or_default().clone().into(),
        Term::Triple(tr) => oxrdf::Triple::new(
            match &tr.subject {
                NamedOrBlankNode::BlankNode(b) => {
                    NamedOrBlankNode::from(map.entry(b.clone()).or_default().clone())
                }
                s => s.clone(),
            },
            tr.predicate.clone(),
            bnode_map(&tr.object, map),
        )
        .into(),
        t => t.clone(),
    }
}

/// `INSERT DATA` quads with fresh blank nodes.
pub fn insert_data_quads(data: &[spargebra::term::Quad]) -> Vec<Quad> {
    let mut map: HashMap<BlankNode, BlankNode> = HashMap::new();
    data.iter()
        .map(|q| {
            let s = match &q.subject {
                NamedOrBlankNode::BlankNode(b) => {
                    NamedOrBlankNode::from(map.entry(b.clone()).or_default().clone())
                }
                s => s.clone(),
            };
            let g = match &q.graph_name {
                spargebra::term::GraphName::NamedNode(n) => GraphName::NamedNode(n.clone()),
                spargebra::term::GraphName::DefaultGraph => GraphName::DefaultGraph,
            };
            Quad::new(s, q.predicate.clone(), bnode_map(&q.object, &mut map), g)
        })
        .collect()
}

fn ground_term(t: &GroundTerm) -> Term {
    crate::compiler::ground_to_term(t)
}

/// Converts `DELETE DATA` quads.
pub fn delete_data_quads(data: &[GroundQuad]) -> Vec<Quad> {
    data.iter()
        .map(|q| {
            Quad::new(
                q.subject.clone(),
                q.predicate.clone(),
                ground_term(&q.object),
                match &q.graph_name {
                    spargebra::term::GraphName::NamedNode(n) => GraphName::NamedNode(n.clone()),
                    spargebra::term::GraphName::DefaultGraph => GraphName::DefaultGraph,
                },
            )
        })
        .collect()
}

fn guard(column: &str, condition: &str) -> Statement {
    Statement::new(format!(
        "INSERT INTO oxilite_guard({column}) SELECT 1 WHERE {condition}"
    ))
}

fn clear_statements(
    graph: &GraphTarget,
    silent: bool,
    drop: bool,
    caps: &Capabilities,
) -> Vec<Statement> {
    let _ = caps;
    let mut s = Vec::new();
    match graph {
        GraphTarget::NamedNode(g) => {
            let id = named_node_id(g.as_str());
            if !silent {
                s.push(guard(
                    "graph_does_not_exist",
                    &format!("NOT EXISTS (SELECT 1 FROM graphs WHERE id = {id})"),
                ));
            }
            s.push(Statement::new(format!("DELETE FROM quads WHERE g = {id}")));
            if drop {
                s.push(Statement::new(format!(
                    "DELETE FROM graphs WHERE id = {id}"
                )));
            }
        }
        GraphTarget::DefaultGraph => {
            s.push(Statement::new(format!(
                "DELETE FROM quads WHERE g = {DEFAULT_GRAPH_ID}"
            )));
        }
        GraphTarget::NamedGraphs => {
            s.push(Statement::new(format!(
                "DELETE FROM quads WHERE g <> {DEFAULT_GRAPH_ID}"
            )));
            if drop {
                s.push("DELETE FROM graphs".into());
            }
        }
        GraphTarget::AllGraphs => {
            s.push("DELETE FROM quads".into());
            if drop {
                s.push("DELETE FROM graphs".into());
            }
        }
    }
    s
}

/// Plans a SPARQL update.
pub fn plan_update(update: &Update, caps: &Capabilities) -> Result<Vec<PlannedOp>> {
    let mut out = Vec::new();
    for (i, op) in update.operations.iter().enumerate() {
        out.push(match op {
            GraphUpdateOperation::InsertData { data } => {
                let quads = insert_data_quads(data);
                PlannedOp::Sql(
                    EncodedQuads::new(quads.iter().map(Quad::as_ref)).insert_statements(caps),
                )
            }
            GraphUpdateOperation::DeleteData { data } => {
                let quads = delete_data_quads(data);
                if quads.is_empty() {
                    PlannedOp::Sql(Vec::new())
                } else {
                    PlannedOp::Sql(
                        EncodedQuads::new(quads.iter().map(Quad::as_ref)).delete_statements(caps),
                    )
                }
            }
            GraphUpdateOperation::Clear { silent, graph } => {
                PlannedOp::Sql(clear_statements(graph, *silent, false, caps))
            }
            GraphUpdateOperation::Drop { silent, graph } => {
                PlannedOp::Sql(clear_statements(graph, *silent, true, caps))
            }
            GraphUpdateOperation::Create { silent, graph } => {
                let mut rows = EncodedRows::default();
                let id = rows.iri(graph.as_str());
                let mut s = term_statements(&rows, caps);
                if *silent {
                    s.push(Statement::new(format!(
                        "INSERT OR IGNORE INTO graphs(id) VALUES ({id})"
                    )));
                } else {
                    s.push(guard(
                        "graph_already_exists",
                        &format!("EXISTS (SELECT 1 FROM graphs WHERE id = {id})"),
                    ));
                    s.push(Statement::new(format!(
                        "INSERT INTO graphs(id) VALUES ({id})"
                    )));
                }
                PlannedOp::Sql(s)
            }
            GraphUpdateOperation::Load { silent, .. } => {
                if *silent {
                    PlannedOp::Sql(Vec::new())
                } else {
                    return Err(Error::unsupported("LOAD (the core has no network access)"));
                }
            }
            GraphUpdateOperation::DeleteInsert { .. } => {
                PlannedOp::Fallback(i, "DELETE/INSERT … WHERE".into())
            }
        });
    }
    Ok(out)
}

/// Graph id helper used by drivers.
pub fn graph_name_id(g: &GraphName) -> i64 {
    graph_id(g.as_ref())
}
