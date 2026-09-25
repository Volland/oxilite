//! Store operations as sans-IO jobs, shared by the sync, async and JavaScript drivers.
//!
// @lat: [[architecture#Sans-IO core]]

use crate::encoding::{
    graph_id, named_node_id, subject_id, term_id, EncodedRows, DEFAULT_GRAPH_ID,
};
use crate::error::{Error, Result};
use crate::job::{Job, OneShot, Step};
use crate::resolve::TermResolver;
use crate::schema::{base_schema, StoreOptions};
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
///
/// The versioning level recorded in the store wins: opening never changes it, except that an
/// empty store opened with a higher level gets it (that is how a store is created versioned).
/// A higher level on a store holding data is refused: raising it is an explicit level change.
pub fn open_job(options: &StoreOptions, caps: &Capabilities) -> impl Job<Output = Stats> {
    enum Phase {
        Start,
        Schema,
        Stats,
        Empty(Stats),
        Upgraded,
        Reloaded,
    }
    struct Open {
        options: StoreOptions,
        caps: Capabilities,
        phase: Phase,
    }
    impl Job for Open {
        type Output = Stats;
        fn step(&mut self, response: Option<Response>) -> Result<Step<Stats>> {
            let wanted = self.options.versioning;
            match std::mem::replace(&mut self.phase, Phase::Reloaded) {
                Phase::Start => {
                    self.phase = Phase::Schema;
                    Ok(Step::Execute(base_schema(&self.options)))
                }
                Phase::Schema => {
                    self.phase = Phase::Stats;
                    Ok(Step::Execute(Stats::load_request(&self.caps)))
                }
                Phase::Stats => {
                    let stats = Stats::from_response(&response.unwrap_or_default())?;
                    if wanted <= stats.version.level {
                        return Ok(Step::Done(stats));
                    }
                    self.phase = Phase::Empty(stats);
                    Ok(Step::Execute(Request::read(vec![Statement::new(
                        "SELECT NOT EXISTS (SELECT 1 FROM quads)",
                    )])))
                }
                Phase::Empty(stats) => {
                    let response = response.unwrap_or_default();
                    let empty = response
                        .first()
                        .and_then(|r| r.rows.first())
                        .and_then(|row| row.first())
                        .and_then(SqlValue::as_i64)
                        == Some(1);
                    if !empty || stats.version.history != crate::version::History::None {
                        return Err(Error::Other(format!(
                            "the store's versioning level is `{}`; opening does not change it. Raise it explicitly (`Store::set_versioning`, `oxilite versioning set {wanted}`)",
                            stats.version.level
                        )));
                    }
                    self.phase = Phase::Upgraded;
                    Ok(Step::Execute(Request::atomic(
                        crate::version::change_statements(
                            &stats.version,
                            wanted,
                            &self.options.level_change(),
                        )?,
                    )))
                }
                Phase::Upgraded => {
                    self.phase = Phase::Reloaded;
                    Ok(Step::Execute(Stats::load_request(&self.caps)))
                }
                Phase::Reloaded => {
                    Stats::from_response(&response.unwrap_or_default()).map(Step::Done)
                }
            }
        }
    }
    Open {
        options: options.clone(),
        caps: caps.clone(),
        phase: Phase::Start,
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

/// The statements that rebuild the derived schema caches inside a write's own request:
/// `tbox_closure` when a schema axiom changed, the shape index when a SHACL triple did.
///
/// Every driver that assembles a write request itself (the async store, the JavaScript
/// bindings, the Cypher writer) appends this, so the caches cannot be refreshed on one backend
/// and forgotten on another.
pub fn schema_refresh_for<'a>(quads: impl IntoIterator<Item = QuadRef<'a>>) -> Vec<Statement> {
    let mut closure = false;
    let mut shapes = false;
    for q in quads {
        closure |= crate::reason::is_schema_quad(q);
        shapes |= crate::shapes::is_shape_quad(q);
        if closure && shapes {
            break;
        }
    }
    schema_refresh(closure, shapes)
}

fn schema_refresh(closure: bool, shapes: bool) -> Vec<Statement> {
    let mut s = Vec::new();
    if closure {
        s.extend(crate::reason::closure_statements());
    }
    if shapes {
        s.extend(crate::shapes::refresh_statements());
    }
    s
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
    let refresh = schema_refresh_for(quads.iter().copied());
    let enc = EncodedQuads::new(quads);
    let quad_stmts = crate::writer::quad_insert_statements(&enc.quads, caps).len();
    let mut stmts = enc.insert_statements(caps);
    let end = stmts.len();
    stmts.extend(refresh);
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
    let refresh = schema_refresh_for(quads.iter().copied());
    let enc = EncodedQuads::new(quads);
    let mut stmts = enc.delete_statements(caps);
    let end = stmts.len();
    stmts.extend(refresh);
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

/// Quads whose subject (or, with `incoming`, object) is one of `nodes`, optionally restricted
/// to some predicates and to the default graph: one neighbourhood hop for bounded prefetches.
pub fn neighbourhood_job(
    nodes: &[TermRef<'_>],
    incoming: bool,
    predicates: Option<&[NamedNodeRef<'_>]>,
    default_graph_only: bool,
    caps: &Capabilities,
) -> ScanJob {
    let ids: Vec<String> = nodes.iter().map(|t| term_id(*t).to_string()).collect();
    let mut w = vec![format!(
        "{} IN ({})",
        if incoming { "o" } else { "s" },
        if ids.is_empty() {
            "NULL".into()
        } else {
            ids.join(",")
        }
    )];
    if let Some(ps) = predicates {
        let ps: Vec<String> = ps
            .iter()
            .map(|p| named_node_id(p.as_str()).to_string())
            .collect();
        w.push(format!(
            "p IN ({})",
            if ps.is_empty() {
                "NULL".into()
            } else {
                ps.join(",")
            }
        ));
    }
    if default_graph_only {
        w.push(format!("g = {DEFAULT_GRAPH_ID}"));
    }
    ScanJob::new(w.join(" AND "), caps)
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
    stmts.extend(crate::registry::unregister_statements(id));
    stmts.extend(schema_refresh(true, true));
    OneShot::new(Request::atomic(stmts), |r| {
        Ok(r.iter().take(2).any(|rs| rs.changes > 0))
    })
}

/// Removes all quads of a graph (keeps the graph name).
pub fn clear_graph_job(g: GraphNameRef<'_>) -> OneShot<()> {
    let id = graph_id(g);
    let mut stmts = vec![Statement::new(format!("DELETE FROM quads WHERE g = {id}"))];
    stmts.extend(schema_refresh(true, true));
    OneShot::new(Request::atomic(stmts), |_| Ok(()))
}

/// Removes everything.
pub fn clear_job() -> OneShot<()> {
    OneShot::new(
        Request::atomic(vec![
            "DELETE FROM quads".into(),
            "DELETE FROM quads_inf".into(),
            "DELETE FROM quads_inf_src".into(),
            "DELETE FROM inf_producers".into(),
            "DELETE FROM tbox_closure".into(),
            "DELETE FROM shapes_index".into(),
            "DELETE FROM shapes_in".into(),
            "DELETE FROM schema_graphs".into(),
            "DELETE FROM graphs".into(),
            "DELETE FROM triple_terms".into(),
            "DELETE FROM terms".into(),
        ]),
        |_| Ok(()),
    )
}

/// Registers a schema graph, then rebuilds what its role feeds.
///
/// A registration changes the *scope* of the closure and the shape index — not just their
/// content — so both are rebuilt in the same atomic request as the registration itself.
///
/// A named graph may be registered before it holds anything, so the registration creates it
/// (as `insert_named_graph` would): otherwise the registry would name a graph the term
/// dictionary has never heard of.
pub fn register_schema_graph_job(
    entry: &crate::registry::SchemaGraph,
    graph: GraphNameRef<'_>,
    caps: &Capabilities,
) -> OneShot<()> {
    let mut stmts = Vec::new();
    let named: Option<NamedOrBlankNodeRef<'_>> = match graph {
        GraphNameRef::NamedNode(n) => Some(n.into()),
        GraphNameRef::BlankNode(b) => Some(b.into()),
        GraphNameRef::DefaultGraph => None,
    };
    if let Some(g) = named {
        let mut rows = EncodedRows::default();
        let id = rows.subject(g);
        stmts.extend(term_statements(&rows, caps));
        stmts.push(Statement::new(format!(
            "INSERT OR IGNORE INTO graphs(id) VALUES ({id})"
        )));
    }
    stmts.extend(crate::registry::register_statements(entry));
    stmts.extend(schema_refresh(true, true));
    OneShot::new(Request::atomic(stmts), |_| Ok(()))
}

/// Removes a registration, keeping the graph's triples.
pub fn unregister_schema_graph_job(graph: i64) -> OneShot<bool> {
    let mut stmts = crate::registry::unregister_statements(graph);
    stmts.extend(schema_refresh(true, true));
    OneShot::new(Request::atomic(stmts), |r| Ok(scalar_changes(&r, 1) > 0))
}

/// Activates or deactivates a registration.
pub fn set_schema_graph_active_job(graph: i64, active: bool) -> OneShot<bool> {
    let mut stmts = crate::registry::set_active_statements(graph, active);
    stmts.extend(schema_refresh(true, true));
    OneShot::new(Request::atomic(stmts), |r| Ok(scalar_changes(&r, 1) > 0))
}

/// Removes a registration together with every quad of its graph.
pub fn drop_schema_graph_job(graph: i64) -> OneShot<u64> {
    let mut stmts = crate::registry::drop_statements(graph);
    stmts.extend(schema_refresh(true, true));
    OneShot::new(Request::atomic(stmts), |r| Ok(scalar_changes(&r, 1)))
}

/// Reads the registry, with each row's graph name resolved.
pub fn schema_graphs_job(
    caps: &Capabilities,
) -> impl Job<Output = Vec<(crate::registry::SchemaGraph, GraphName)>> {
    struct Registry {
        request: Option<Request>,
        caps: Capabilities,
        resolver: TermResolver,
        rows: Vec<crate::registry::SchemaGraph>,
        started: bool,
    }
    impl Job for Registry {
        type Output = Vec<(crate::registry::SchemaGraph, GraphName)>;
        fn step(&mut self, response: Option<Response>) -> Result<Step<Self::Output>> {
            if let Some(r) = self.request.take() {
                return Ok(Step::Execute(r));
            }
            let response = response.unwrap_or_default();
            if self.started {
                self.resolver.absorb(response)?;
            } else {
                self.started = true;
                self.rows = crate::registry::from_response(&response)?;
                for row in &self.rows {
                    if row.graph != DEFAULT_GRAPH_ID {
                        self.resolver.want(row.graph);
                    }
                }
            }
            if let Some(r) = self.resolver.request(&self.caps) {
                return Ok(Step::Execute(r));
            }
            std::mem::take(&mut self.rows)
                .into_iter()
                .map(|row| {
                    let name = if row.graph == DEFAULT_GRAPH_ID {
                        GraphName::DefaultGraph
                    } else {
                        crate::encoding::to_graph_name(
                            row.graph,
                            Some(self.resolver.get(row.graph)?),
                        )?
                    };
                    Ok((row, name))
                })
                .collect::<Result<Vec<_>>>()
                .map(Step::Done)
        }
    }
    Registry {
        request: Some(crate::registry::load_request(caps)),
        caps: caps.clone(),
        resolver: TermResolver::default(),
        rows: Vec::new(),
        started: false,
    }
}

/// Reads the compiled shape index.
pub fn shape_index_job(caps: &Capabilities) -> OneShot<crate::shapes::ShapeIndex> {
    OneShot::new(crate::shapes::ShapeIndex::load_request(caps), |r| {
        crate::shapes::ShapeIndex::from_response(&r)
    })
}

/// Rows changed by the first `n` statements of a response.
fn scalar_changes(r: &Response, n: usize) -> u64 {
    r.iter().take(n).map(|rs| rs.changes).sum()
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
        Request::atomic(vec![
            Statement::new("DELETE FROM quads_inf"),
            Statement::new("DELETE FROM quads_inf_src"),
        ]),
        |_| Ok(()),
    )
}

/// Removes the inferences of one producer, keeping what other producers also derived.
pub fn clear_inferences_of_job(producer: &str) -> OneShot<()> {
    OneShot::new(
        Request::atomic(crate::reason::inference_reset(producer)),
        |_| Ok(()),
    )
}

/// The names of the producers that derived a quad (empty when it is not inferred). The
/// default graph matches the graph-0 conclusions every producer writes.
pub fn inference_producers_job(quad: QuadRef<'_>) -> OneShot<Vec<String>> {
    let [s, p, o, g] = [
        subject_id(quad.subject),
        named_node_id(quad.predicate.as_str()),
        term_id(quad.object),
        graph_id(quad.graph_name),
    ];
    OneShot::new(
        Request::read(vec![Statement::new(format!(
            "SELECT p.name FROM quads_inf_src q JOIN inf_producers p ON p.id = q.src \
             WHERE q.s = {s} AND q.p = {p} AND q.o = {o} AND q.g = {g} ORDER BY p.name"
        ))]),
        |r| {
            Ok(r.first()
                .map(|rs| {
                    rs.rows
                        .iter()
                        .filter_map(|row| match row.first() {
                            Some(SqlValue::Text(t)) => Some(t.clone()),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default())
        },
    )
}
