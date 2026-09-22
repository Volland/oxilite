//! Store operations as sans-IO jobs, shared by the sync, async and JavaScript drivers.
//!
// @lat: [[architecture#Sans-IO core]]

use crate::encoding::{
    graph_id, named_node_id, subject_id, term_id, EncodedRows, DEFAULT_GRAPH_ID,
};
use crate::error::{Error, Result};
use crate::job::{Job, OneShot, Step};
use crate::resolve::TermResolver;
use crate::schema::{create_schema, StoreOptions};
use crate::sql::{Capabilities, Request, Response, SqlValue, Statement};
use crate::stats::Stats;
use crate::writer::{term_statements, EncodedQuads};
use oxrdf::{
    GraphName, GraphNameRef, NamedNodeRef, NamedOrBlankNode, NamedOrBlankNodeRef, Quad, QuadRef,
    Term, TermRef,
};

fn id_col(caps: &Capabilities, c: &str) -> String {
    if caps.int64_as_text {
        format!("CAST({c} AS TEXT)")
    } else {
        c.into()
    }
}

/// Creates the schema, then loads statistics.
pub fn open_job(options: &StoreOptions, caps: &Capabilities) -> impl Job<Output = Stats> {
    let schema = create_schema(options);
    let load = Stats::load_request(caps);
    struct Open {
        schema: Option<Request>,
        load: Option<Request>,
    }
    impl Job for Open {
        type Output = Stats;
        fn step(&mut self, response: Option<Response>) -> Result<Step<Stats>> {
            if let Some(r) = self.schema.take() {
                return Ok(Step::Execute(r));
            }
            if let Some(r) = self.load.take() {
                return Ok(Step::Execute(r));
            }
            Stats::from_response(&response.unwrap_or_default()).map(Step::Done)
        }
    }
    Open {
        schema: Some(schema),
        load: Some(load),
    }
}

/// Loads statistics only.
pub fn stats_job(caps: &Capabilities) -> OneShot<Stats> {
    OneShot::new(Stats::load_request(caps), |r| Stats::from_response(&r))
}

/// Recomputes and reloads statistics.
pub fn optimize_job(caps: &Capabilities) -> impl Job<Output = Stats> {
    let refresh = Stats::refresh_request();
    let load = Stats::load_request(caps);
    struct Optimize {
        refresh: Option<Request>,
        load: Option<Request>,
    }
    impl Job for Optimize {
        type Output = Stats;
        fn step(&mut self, response: Option<Response>) -> Result<Step<Stats>> {
            if let Some(r) = self.refresh.take() {
                return Ok(Step::Execute(r));
            }
            if let Some(r) = self.load.take() {
                return Ok(Step::Execute(r));
            }
            Stats::from_response(&response.unwrap_or_default()).map(Step::Done)
        }
    }
    Optimize {
        refresh: Some(refresh),
        load: Some(load),
    }
}

fn count_tail(r: &Response, n: usize) -> u64 {
    r.iter().rev().take(n).map(|rs| rs.changes).sum()
}

/// Atomically inserts quads; returns how many were new.
pub fn insert_job<'a>(
    quads: impl IntoIterator<Item = QuadRef<'a>>,
    caps: &Capabilities,
) -> OneShot<u64> {
    let quads: Vec<QuadRef<'a>> = quads.into_iter().collect();
    let schema = quads.iter().any(|q| crate::reason::is_schema_quad(*q));
    let enc = EncodedQuads::new(quads);
    let quad_stmts = crate::writer::quad_insert_statements(&enc.quads, caps).len();
    let mut stmts = enc.insert_statements(caps);
    let end = stmts.len();
    if schema {
        stmts.extend(crate::reason::closure_statements());
    }
    OneShot::new(Request::atomic(stmts), move |mut r| {
        r.truncate(end);
        Ok(count_tail(&r, quad_stmts))
    })
}

/// Insert statements for a chunk (bulk loading).
pub fn insert_request<'a>(
    quads: impl IntoIterator<Item = QuadRef<'a>>,
    caps: &Capabilities,
) -> Request {
    Request::atomic(EncodedQuads::new(quads).insert_statements(caps))
}

/// Atomically removes quads; returns how many were present.
pub fn remove_job<'a>(
    quads: impl IntoIterator<Item = QuadRef<'a>>,
    caps: &Capabilities,
) -> OneShot<u64> {
    let quads: Vec<QuadRef<'a>> = quads.into_iter().collect();
    let schema = quads.iter().any(|q| crate::reason::is_schema_quad(*q));
    let enc = EncodedQuads::new(quads);
    let mut stmts = enc.delete_statements(caps);
    let end = stmts.len();
    if schema {
        stmts.extend(crate::reason::closure_statements());
    }
    OneShot::new(Request::atomic(stmts), move |r| {
        Ok(r.iter().take(end).map(|rs| rs.changes).sum())
    })
}

fn scalar(r: &Response) -> i64 {
    r.first()
        .and_then(|rs| rs.rows.first())
        .and_then(|row| row.first())
        .and_then(SqlValue::as_i64)
        .unwrap_or(0)
}

pub fn contains_job(quad: QuadRef<'_>) -> OneShot<bool> {
    let [s, p, o, g] = [
        subject_id(quad.subject),
        named_node_id(quad.predicate.as_str()),
        term_id(quad.object),
        graph_id(quad.graph_name),
    ];
    OneShot::new(
        Request::read(vec![Statement::new(format!(
            "SELECT EXISTS (SELECT 1 FROM quads WHERE s = {s} AND p = {p} AND o = {o} AND g = {g})"
        ))]),
        |r| Ok(scalar(&r) != 0),
    )
}

pub fn len_job() -> OneShot<usize> {
    OneShot::new(
        Request::read(vec!["SELECT COUNT(*) FROM quads".into()]),
        |r| Ok(scalar(&r) as usize),
    )
}

pub fn is_empty_job() -> OneShot<bool> {
    OneShot::new(
        Request::read(vec!["SELECT NOT EXISTS (SELECT 1 FROM quads)".into()]),
        |r| Ok(scalar(&r) != 0),
    )
}

/// A job returning quads matching a SQL WHERE clause, with batched term resolution.
pub struct ScanJob {
    sql: Option<String>,
    caps: Capabilities,
    resolver: TermResolver,
    rows: Vec<[i64; 4]>,
    started: bool,
}

impl ScanJob {
    fn new(where_clause: String, caps: &Capabilities) -> Self {
        let sql = format!(
            "SELECT {}, {}, {}, {} FROM quads{}",
            id_col(caps, "s"),
            id_col(caps, "p"),
            id_col(caps, "o"),
            id_col(caps, "g"),
            if where_clause.is_empty() {
                String::new()
            } else {
                format!(" WHERE {where_clause}")
            }
        );
        Self {
            sql: Some(sql),
            caps: caps.clone(),
            resolver: TermResolver::default(),
            rows: Vec::new(),
            started: false,
        }
    }

    fn finish(&self) -> Result<Vec<Quad>> {
        self.rows
            .iter()
            .map(|[s, p, o, g]| {
                let gname = if *g == DEFAULT_GRAPH_ID {
                    GraphName::DefaultGraph
                } else {
                    crate::encoding::to_graph_name(*g, Some(self.resolver.get(*g)?))?
                };
                crate::encoding::make_quad(
                    self.resolver.get(*s)?,
                    self.resolver.get(*p)?,
                    self.resolver.get(*o)?,
                    gname,
                )
            })
            .collect()
    }
}

impl Job for ScanJob {
    type Output = Vec<Quad>;

    fn step(&mut self, response: Option<Response>) -> Result<Step<Vec<Quad>>> {
        if let Some(sql) = self.sql.take() {
            return Ok(Step::Execute(Request::read(vec![Statement::new(sql)])));
        }
        let response =
            response.ok_or_else(|| Error::Other("scan resumed without response".into()))?;
        if self.started {
            self.resolver.absorb(response)?;
        } else {
            self.started = true;
            for rs in response {
                for row in rs.rows {
                    let ids: Vec<i64> = row.iter().filter_map(SqlValue::as_i64).collect();
                    let [s, p, o, g] = ids[..] else {
                        return Err(Error::corrupted("bad quad row"));
                    };
                    for id in [s, p, o, g] {
                        self.resolver.want(id);
                    }
                    self.rows.push([s, p, o, g]);
                }
            }
        }
        match self.resolver.request(&self.caps) {
            Some(r) => Ok(Step::Execute(r)),
            None => self.finish().map(Step::Done),
        }
    }
}

/// `quads_for_pattern` as a job. `graph_name = None` matches every graph, default included.
pub fn scan_job(
    subject: Option<NamedOrBlankNodeRef<'_>>,
    predicate: Option<NamedNodeRef<'_>>,
    object: Option<TermRef<'_>>,
    graph_name: Option<GraphNameRef<'_>>,
    caps: &Capabilities,
) -> ScanJob {
    let mut w = Vec::new();
    if let Some(s) = subject {
        w.push(format!("s = {}", subject_id(s)));
    }
    if let Some(p) = predicate {
        w.push(format!("p = {}", named_node_id(p.as_str())));
    }
    if let Some(o) = object {
        w.push(format!("o = {}", term_id(o)));
    }
    if let Some(g) = graph_name {
        w.push(format!("g = {}", graph_id(g)));
    }
    ScanJob::new(w.join(" AND "), caps)
}

/// Lists named graphs.
pub fn named_graphs_job(caps: &Capabilities) -> impl Job<Output = Vec<NamedOrBlankNode>> {
    struct Graphs {
        sql: Option<String>,
        caps: Capabilities,
        resolver: TermResolver,
        ids: Vec<i64>,
        started: bool,
    }
    impl Job for Graphs {
        type Output = Vec<NamedOrBlankNode>;
        fn step(&mut self, response: Option<Response>) -> Result<Step<Self::Output>> {
            if let Some(sql) = self.sql.take() {
                return Ok(Step::Execute(Request::read(vec![Statement::new(sql)])));
            }
            let response = response.unwrap_or_default();
            if self.started {
                self.resolver.absorb(response)?;
            } else {
                self.started = true;
                self.ids = crate::resolve::ids_of(&response, 0);
                for id in &self.ids {
                    self.resolver.want(*id);
                }
            }
            if let Some(r) = self.resolver.request(&self.caps) {
                return Ok(Step::Execute(r));
            }
            self.ids
                .iter()
                .map(|id| crate::encoding::to_subject(self.resolver.get(*id)?))
                .collect::<Result<_>>()
                .map(Step::Done)
        }
    }
    Graphs {
        sql: Some(format!("SELECT {} FROM graphs", id_col(caps, "id"))),
        caps: caps.clone(),
        resolver: TermResolver::default(),
        ids: Vec::new(),
        started: false,
    }
}

pub fn contains_named_graph_job(g: NamedOrBlankNodeRef<'_>) -> OneShot<bool> {
    let id = subject_id(g);
    OneShot::new(
        Request::read(vec![Statement::new(format!(
            "SELECT EXISTS (SELECT 1 FROM graphs WHERE id = {id})"
        ))]),
        |r| Ok(scalar(&r) != 0),
    )
}

/// Adds a named graph; returns whether it was new.
pub fn insert_named_graph_job(g: NamedOrBlankNodeRef<'_>, caps: &Capabilities) -> OneShot<bool> {
    let mut rows = EncodedRows::default();
    let id = rows.subject(g);
    let mut stmts = term_statements(&rows, caps);
    stmts.push(Statement::new(format!(
        "INSERT OR IGNORE INTO graphs(id) VALUES ({id})"
    )));
    OneShot::new(Request::atomic(stmts), |r| Ok(count_tail(&r, 1) > 0))
}

/// Removes a named graph and its quads; returns whether it existed.
pub fn remove_named_graph_job(g: NamedOrBlankNodeRef<'_>) -> OneShot<bool> {
    let id = subject_id(g);
    let mut stmts = vec![
        Statement::new(format!("DELETE FROM quads WHERE g = {id}")),
        Statement::new(format!("DELETE FROM graphs WHERE id = {id}")),
    ];
    stmts.extend(crate::reason::closure_statements());
    OneShot::new(Request::atomic(stmts), |r| {
        Ok(r.iter().take(2).any(|rs| rs.changes > 0))
    })
}

/// Removes all quads of a graph (keeps the graph name).
pub fn clear_graph_job(g: GraphNameRef<'_>) -> OneShot<()> {
    let id = graph_id(g);
    let mut stmts = vec![Statement::new(format!("DELETE FROM quads WHERE g = {id}"))];
    stmts.extend(crate::reason::closure_statements());
    OneShot::new(Request::atomic(stmts), |_| Ok(()))
}

/// Removes everything.
pub fn clear_job() -> OneShot<()> {
    OneShot::new(
        Request::atomic(vec![
            "DELETE FROM quads".into(),
            "DELETE FROM quads_inf".into(),
            "DELETE FROM tbox_closure".into(),
            "DELETE FROM graphs".into(),
            "DELETE FROM triple_terms".into(),
            "DELETE FROM terms".into(),
        ]),
        |_| Ok(()),
    )
}

/// Helper used by drivers: encodes a term to its id without I/O.
pub fn encode_term(t: &Term) -> i64 {
    term_id(t.as_ref())
}

/// Recomputes the OWL 2 RL materialization (`quads_inf`): discards previous inferences, then
/// runs rule rounds (one atomic request each, i.e. one D1 batch) until a round infers nothing.
/// Returns the number of inferred triples. Rounds are not one transaction: readers may see a
/// partial materialization while it runs.
pub fn materialize_job(max_rounds: usize, caps: &Capabilities) -> impl Job<Output = u64> {
    struct Materialize {
        reset: Option<Request>,
        round: usize,
        max_rounds: usize,
        counting: bool,
    }
    impl Job for Materialize {
        type Output = u64;
        fn step(&mut self, response: Option<Response>) -> Result<Step<u64>> {
            if let Some(r) = self.reset.take() {
                return Ok(Step::Execute(r));
            }
            if self.counting {
                return Ok(Step::Done(
                    scalar(&response.unwrap_or_default()).max(0) as u64
                ));
            }
            let changed: u64 = response.iter().flatten().map(|rs| rs.changes).sum();
            if self.round > 0 && (changed == 0 || self.round >= self.max_rounds) {
                self.counting = true;
                return Ok(Step::Execute(Request::read(vec![
                    "SELECT COUNT(*) FROM quads_inf".into(),
                ])));
            }
            self.round += 1;
            Ok(Step::Execute(Request::atomic(
                crate::reason::materialize_round(),
            )))
        }
    }
    Materialize {
        reset: Some(Request::atomic(crate::reason::materialize_reset(caps))),
        round: 0,
        max_rounds,
        counting: false,
    }
}

/// Removes every materialized inference.
pub fn clear_inferences_job() -> OneShot<()> {
    OneShot::new(
        Request::atomic(vec![Statement::new("DELETE FROM quads_inf")]),
        |_| Ok(()),
    )
}
