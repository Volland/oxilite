//! SPARQL UPDATE planning.
//!
//! Operations become SQL statements appended to one atomic request, so a whole update
//! request is applied as a single transaction (one D1 batch). Operations that cannot be
//! compiled are reported as [`PlannedOp::Fallback`] for sync backends.
//!
// @lat: [[architecture#Updates and atomicity]]

use crate::compiler::{Col, Compiler, QueryOptions};
use crate::encoding::{
    graph_id, named_node_id, EncodedRows, Tag, DEFAULT_GRAPH_ID, PAYLOAD_BITS, PAYLOAD_MASK,
};
use crate::error::{Error, Result};
use crate::sql::{Capabilities, Statement};
use crate::stats::Stats;
use crate::writer::{term_statements, EncodedQuads};
use oxrdf::{BlankNode, GraphName, NamedOrBlankNode, Quad, Term, Variable};
use spargebra::algebra::{GraphPattern, GraphTarget, QueryDataset};
use spargebra::term::{
    GraphNamePattern, GroundQuad, GroundQuadPattern, GroundTerm, GroundTermPattern,
    NamedNodePattern, QuadPattern, TermPattern,
};
use spargebra::{GraphUpdateOperation, Update};
use std::collections::{BTreeSet, HashMap};

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

/// A template position after compilation: a SQL expression of the term id.
enum Slot {
    Sql(String),
}

/// Compiles `DELETE { … } INSERT { … } WHERE { … }` into self-reading statements: the WHERE
/// solutions are evaluated once (a MATERIALIZED CTE) into `update_buffer`, then deletions and
/// insertions are applied from the buffer — SPARQL's "evaluate, then apply" semantics inside
/// one atomic batch.
///
// @lat: [[architecture#Updates and atomicity]]
#[allow(clippy::too_many_arguments)]
fn delete_insert(
    delete: &[GroundQuadPattern],
    insert: &[QuadPattern],
    using: Option<&QueryDataset>,
    pattern: &GraphPattern,
    base_iri: Option<String>,
    stats: &Stats,
    caps: &Capabilities,
    options: &QueryOptions,
) -> Result<Vec<Statement>> {
    let mut c = Compiler::new(stats, caps, options, using, base_iri);
    let block = c.pattern(pattern)?;
    // Variables used by the templates.
    let mut vars: BTreeSet<Variable> = BTreeSet::new();
    let mut bnodes: BTreeSet<String> = BTreeSet::new();
    fn tvars(
        t: &TermPattern,
        vars: &mut BTreeSet<Variable>,
        bnodes: &mut BTreeSet<String>,
    ) -> Result<()> {
        match t {
            TermPattern::Variable(v) => {
                vars.insert(v.clone());
            }
            TermPattern::BlankNode(b) => {
                bnodes.insert(b.as_str().to_string());
            }
            TermPattern::Triple(_) => {
                return Err(Error::unsupported("triple terms in update templates"))
            }
            _ => {}
        }
        Ok(())
    }
    fn gvars(t: &GroundTermPattern, vars: &mut BTreeSet<Variable>) -> Result<()> {
        match t {
            GroundTermPattern::Variable(v) => {
                vars.insert(v.clone());
            }
            GroundTermPattern::Triple(_) => {
                return Err(Error::unsupported("triple terms in update templates"))
            }
            _ => {}
        }
        Ok(())
    }
    let add_nn = |p: &NamedNodePattern, vars: &mut BTreeSet<Variable>| {
        if let NamedNodePattern::Variable(v) = p {
            vars.insert(v.clone());
        }
    };
    let add_g = |g: &GraphNamePattern, vars: &mut BTreeSet<Variable>| {
        if let GraphNamePattern::Variable(v) = g {
            vars.insert(v.clone());
        }
    };
    for q in delete {
        gvars(&q.subject, &mut vars)?;
        add_nn(&q.predicate, &mut vars);
        gvars(&q.object, &mut vars)?;
        add_g(&q.graph_name, &mut vars);
    }
    for q in insert {
        tvars(&q.subject, &mut vars, &mut bnodes)?;
        add_nn(&q.predicate, &mut vars);
        tvars(&q.object, &mut vars, &mut bnodes)?;
        add_g(&q.graph_name, &mut vars);
    }
    let idxs: Vec<usize> = vars.iter().map(|v| c.var(v)).collect();
    let mut cols = HashMap::new();
    // Rows whose computed value cannot be stored from SQL (not an inline integer).
    let mut unstorable = Vec::new();
    for (v, idx) in vars.iter().zip(&idxs) {
        let key = match block.cols.get(idx).map(|b| &b.col) {
            None => "NULL".to_string(),
            Some(Col::Id(_)) => format!("w.v{idx}"),
            Some(Col::Val(val)) if val.id.is_some() => format!("w.v{idx}_i"),
            Some(Col::Val(val)) if val.stat == crate::compiler::expr::Stat::Numeric => {
                // Computed numbers are stored as inline integer ids; anything else aborts the
                // batch (native stores then use the fallback).
                let (n, t) = (format!("w.v{idx}_n"), format!("w.v{idx}_t"));
                let slot = format!(
                    "(CASE WHEN {t} = 1 AND {n} = CAST({n} AS INTEGER) AND abs({n}) < 288230376151711744 THEN {} + CAST({n} AS INTEGER) END)",
                    crate::compiler::expr::INT_BASE
                );
                unstorable.push(format!("({n} IS NOT NULL AND {slot} IS NULL)"));
                slot
            }
            Some(Col::Val(_)) => {
                return Err(Error::unsupported(format!(
                    "computed value ?{} in an update template",
                    v.as_str()
                )))
            }
        };
        cols.insert(v.clone(), key);
    }
    let mut rows = EncodedRows::default();
    let bnode_base = Tag::BlankNode.base();
    let bnode_salt: HashMap<String, i64> = bnodes
        .iter()
        .map(|l| (l.clone(), crate::encoding::blank_node_id(l) & PAYLOAD_MASK))
        .collect();
    let term = |t: &Term, rows: &mut EncodedRows| rows.term(t.as_ref()).to_string();
    let tslot = |t: &TermPattern, rows: &mut EncodedRows| -> Slot {
        Slot::Sql(match t {
            TermPattern::NamedNode(n) => term(&n.clone().into(), rows),
            TermPattern::Literal(l) => term(&l.clone().into(), rows),
            TermPattern::Variable(v) => cols[v].clone(),
            // A fresh blank node per solution: the row's random seed mixed with the label.
            TermPattern::BlankNode(b) => format!(
                "({bnode_base} + ((w.rnd + {}) & {PAYLOAD_MASK}))",
                bnode_salt[b.as_str()]
            ),
            TermPattern::Triple(_) => unreachable!("rejected above"),
        })
    };
    let gslot = |t: &GroundTermPattern, rows: &mut EncodedRows| -> Slot {
        Slot::Sql(match t {
            GroundTermPattern::NamedNode(n) => term(&n.clone().into(), rows),
            GroundTermPattern::Literal(l) => term(&l.clone().into(), rows),
            GroundTermPattern::Variable(v) => cols[v].clone(),
            GroundTermPattern::Triple(_) => unreachable!("rejected above"),
        })
    };
    let nslot = |p: &NamedNodePattern, rows: &mut EncodedRows| -> String {
        match p {
            NamedNodePattern::NamedNode(n) => rows.iri(n.as_str()).to_string(),
            NamedNodePattern::Variable(v) => cols[v].clone(),
        }
    };
    let grslot = |g: &GraphNamePattern, rows: &mut EncodedRows| -> String {
        match g {
            GraphNamePattern::DefaultGraph => DEFAULT_GRAPH_ID.to_string(),
            GraphNamePattern::NamedNode(n) => rows.iri(n.as_str()).to_string(),
            GraphNamePattern::Variable(v) => cols[v].clone(),
        }
    };
    let mut selects = Vec::new();
    let valid_subject = |x: &str| format!("({x} >> {PAYLOAD_BITS}) IN (1, 2)");
    let valid_iri = |x: &str| format!("({x} >> {PAYLOAD_BITS}) = 1");
    let valid_graph = |x: &str| format!("({x} = 0 OR ({x} >> {PAYLOAD_BITS}) IN (1, 2))");
    for q in delete {
        let (Slot::Sql(s), p, Slot::Sql(o), g) = (
            gslot(&q.subject, &mut rows),
            nslot(&q.predicate, &mut rows),
            gslot(&q.object, &mut rows),
            grslot(&q.graph_name, &mut rows),
        );
        selects.push(format!(
            "SELECT 0, {s}, {p}, {o}, {g} FROM w WHERE {s} IS NOT NULL AND {p} IS NOT NULL AND {o} IS NOT NULL AND {g} IS NOT NULL"
        ));
    }
    for q in insert {
        let (Slot::Sql(s), p, Slot::Sql(o), g) = (
            tslot(&q.subject, &mut rows),
            nslot(&q.predicate, &mut rows),
            tslot(&q.object, &mut rows),
            grslot(&q.graph_name, &mut rows),
        );
        selects.push(format!(
            "SELECT 1, {s}, {p}, {o}, {g} FROM w WHERE {s} IS NOT NULL AND {p} IS NOT NULL AND {o} IS NOT NULL AND {g} IS NOT NULL AND {} AND {} AND {}",
            valid_subject(&s),
            valid_iri(&p),
            valid_graph(&g)
        ));
    }
    // Constants that may end up stored need their dictionary rows.
    for t in c.constants.values() {
        rows.term(t.as_ref());
    }
    rows.dedup();
    let mut out = vec![Statement::new("DELETE FROM update_buffer")];
    if !selects.is_empty() {
        let mut where_block = block;
        where_block
            .extra_select
            .push("(abs(random()) & 576460752303423487) AS rnd".into());
        let where_sql = where_block.to_select(Some(&idxs), false);
        if !unstorable.is_empty() {
            out.push(Statement::new(format!(
                "WITH w AS ({where_sql}) INSERT INTO oxilite_guard(computed_value_not_storable) SELECT 1 FROM w WHERE {} LIMIT 1",
                unstorable.join(" OR ")
            )));
        }
        out.push(Statement::new(format!(
            "WITH w AS MATERIALIZED ({where_sql}) INSERT INTO update_buffer(op, s, p, o, g) {}",
            crate::sql::union_all(selects, caps.max_compound_select)
        )));
    }
    out.extend(term_statements(&rows, caps));
    if !bnodes.is_empty() {
        // Dictionary rows for the fresh blank nodes.
        for c in ["s", "o"] {
            out.push(Statement::new(format!(
                "INSERT OR IGNORE INTO terms(id, lex) SELECT DISTINCT {c}, 'ox' || lower(hex({c})) FROM update_buffer WHERE op = 1 AND ({c} >> {PAYLOAD_BITS}) = 2 AND NOT EXISTS (SELECT 1 FROM terms t WHERE t.id = update_buffer.{c})"
            )));
        }
    }
    out.push(Statement::new(
        "DELETE FROM quads WHERE (s, p, o, g) IN (SELECT s, p, o, g FROM update_buffer WHERE op = 0)",
    ));
    out.push(Statement::new(
        "INSERT OR IGNORE INTO quads(s, p, o, g) SELECT s, p, o, g FROM update_buffer WHERE op = 1",
    ));
    out.push(Statement::new(
        "INSERT OR IGNORE INTO graphs(id) SELECT DISTINCT g FROM update_buffer WHERE op = 1 AND g <> 0",
    ));
    out.push(Statement::new("DELETE FROM update_buffer"));
    Ok(out)
}

/// Plans a SPARQL update.
pub fn plan_update(update: &Update, caps: &Capabilities) -> Result<Vec<PlannedOp>> {
    plan_update_with(update, &Stats::default(), caps, &QueryOptions::default())
}

/// Plans a SPARQL update using planner statistics.
pub fn plan_update_with(
    update: &Update,
    stats: &Stats,
    caps: &Capabilities,
    options: &QueryOptions,
) -> Result<Vec<PlannedOp>> {
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
            GraphUpdateOperation::DeleteInsert {
                delete,
                insert,
                using,
                pattern,
            } => match delete_insert(
                delete,
                insert,
                using.as_ref(),
                pattern,
                update.base_iri.as_ref().map(|b| b.as_str().to_string()),
                stats,
                caps,
                options,
            ) {
                Ok(stmts) => PlannedOp::Sql(stmts),
                Err(e) if e.is_unsupported() => {
                    PlannedOp::Fallback(i, format!("DELETE/INSERT … WHERE ({e})"))
                }
                Err(e) => return Err(e),
            },
        });
    }
    // Writes to schema triples keep the reasoning closure current, in the same transaction.
    if crate::reason::update_touches_schema(update) {
        out.push(PlannedOp::Sql(crate::reason::closure_statements()));
    }
    if crate::shapes::update_touches_shapes(update) {
        out.push(PlannedOp::Sql(crate::shapes::refresh_statements()));
    }
    Ok(out)
}

/// Describes how each operation of an update runs (for `explain_update()`).
pub fn explain_plan(plan: &[PlannedOp]) -> String {
    let mut out = String::new();
    for (i, p) in plan.iter().enumerate() {
        match p {
            PlannedOp::Sql(s) => {
                out.push_str(&format!(
                    "-- operation {i}: compiled to {} SQL statement(s)\n",
                    s.len()
                ));
                for st in s {
                    out.push_str(&st.sql);
                    out.push_str(";\n");
                }
            }
            PlannedOp::Fallback(_, why) => {
                out.push_str(&format!("-- operation {i}: not compiled ({why}); evaluated by the fallback (sync backends only)\n"));
            }
        }
    }
    out
}

/// Graph id helper used by drivers.
pub fn graph_name_id(g: &GraphName) -> i64 {
    graph_id(g.as_ref())
}
