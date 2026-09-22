//! Execution of a Cypher statement as a sans-IO step machine.
//!
//! [`CypherJob`] asks its driver for three kinds of work: a SPARQL query (the SQL part of the
//! statement, run by the store so sync backends keep the spareval fallback), plain SQL reads
//! on term ids (materializing nodes and relationships, probing existing edges before
//! writing), and one atomic write request. Drivers exist for the blocking store, the async
//! store (D1) and a core-level [`oxilite_core::Job`] adapter used by the wasm engine.
//!
//! Writes are computed in Rust from the rows of the read: the statement's effect is a set of
//! quads to delete and insert, applied as one atomic request (one D1 batch). New nodes get
//! fresh IRIs minted in Rust, so no read-back is needed.
//!
// @lat: [[architecture#Property graph frontend]]

use crate::ast::*;
use crate::error::{CypherError, Result};
use crate::eval::{eval, eval_pred, Acc, Env, Row};
use crate::lower::{Bind, RelVars};
use crate::plan::{Plan, TailOp};
use crate::schema::Shapes;
use crate::value::{GroupKey, Node, Params, Path, Relationship, Value, REIFIES};
use crate::vocab::Vocabulary;
use crate::{CypherOptions, MultiValue};
use oxilite_core::encoding::{named_node_id, rdf_type_id, subject_id, triple_id};
use oxilite_core::job::Job;
use oxilite_core::query::QueryOutput;
use oxilite_core::resolve::TermResolver;
use oxilite_core::sql::col;
use oxilite_core::writer::EncodedQuads;
use oxilite_core::{Capabilities, Request, Response, Statement, Step};
use oxrdf::vocab::rdf;
use oxrdf::{GraphName, NamedNode, NamedOrBlankNode, Quad, Term, Triple, Variable};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// What a [`CypherJob`] needs next.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum CypherStep {
    /// Evaluate this SPARQL query with these options and resume with [`StepInput::Output`].
    Query(spargebra::Query, oxilite_core::QueryOptions),
    /// Run this read request and resume with [`StepInput::Response`].
    Sql(Request),
    /// Run this atomic write request and resume with [`StepInput::Response`].
    Write(Request),
    Done(CypherResult),
}

/// The input resuming a [`CypherJob`].
#[derive(Debug)]
pub enum StepInput {
    Output(QueryOutput),
    Response(Response),
}

/// Counters of what a statement changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteStats {
    pub nodes_created: u64,
    pub nodes_deleted: u64,
    pub relationships_created: u64,
    pub relationships_deleted: u64,
    pub properties_set: u64,
    pub labels_added: u64,
    pub labels_removed: u64,
}

impl WriteStats {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The result of a Cypher statement.
#[derive(Debug, Clone, PartialEq)]
pub struct CypherResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub stats: WriteStats,
    /// A write changed ontology triples (the store must reload its reasoning facts).
    pub schema_changed: bool,
}

impl CypherResult {
    /// Rows as maps from column name to value.
    pub fn records(&self) -> Vec<BTreeMap<String, Value>> {
        self.rows
            .iter()
            .map(|r| {
                self.columns
                    .iter()
                    .cloned()
                    .zip(r.iter().cloned())
                    .collect()
            })
            .collect()
    }

    /// The single value of a one-row, one-column result.
    pub fn single(&self) -> Option<&Value> {
        match (self.rows.as_slice(), self.columns.len()) {
            ([row], 1) => row.first(),
            _ => None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "columns": self.columns,
            "rows": self.rows.iter().map(|r| r.iter().map(Value::to_json).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "stats": {
                "nodesCreated": self.stats.nodes_created,
                "nodesDeleted": self.stats.nodes_deleted,
                "relationshipsCreated": self.stats.relationships_created,
                "relationshipsDeleted": self.stats.relationships_deleted,
                "propertiesSet": self.stats.properties_set,
                "labelsAdded": self.stats.labels_added,
                "labelsRemoved": self.stats.labels_removed,
            },
        })
    }
}

// ----- SQL fetches on term ids -----

const CHUNK: usize = 300;

fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn spo_cols(caps: &Capabilities, t: &str) -> String {
    ["s", "p", "o"]
        .iter()
        .map(|c| {
            if caps.int64_as_text {
                format!("CAST({t}.{c} AS TEXT)")
            } else {
                format!("{t}.{c}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

enum FetchState {
    Start,
    Main,
    Resolving,
}

/// Runs statements returning `(s, p, o)` id rows — optionally followed by the `terms`
/// columns of `p` (lex) and `o` (lex, dt, lang, dir) — then decodes every id, looking up
/// only what the rows did not carry.
struct QuadFetch {
    stmts: Vec<Statement>,
    rows: Vec<[i64; 3]>,
    known: HashMap<i64, Term>,
    resolver: TermResolver,
    caps: Capabilities,
    state: FetchState,
}

impl QuadFetch {
    fn new(stmts: Vec<Statement>, caps: &Capabilities) -> Self {
        Self {
            stmts,
            rows: Vec::new(),
            known: HashMap::new(),
            resolver: TermResolver::default(),
            caps: caps.clone(),
            state: FetchState::Start,
        }
    }

    fn with_known(mut self, known: HashMap<i64, Term>) -> Self {
        self.known = known;
        self
    }

    /// Also decodes these ids (see [`Self::decoded`]).
    fn with_extra(mut self, ids: impl IntoIterator<Item = i64>) -> Self {
        for id in ids {
            self.want(id);
        }
        self
    }

    /// Every id decoded so far.
    fn decoded(&self, ids: impl IntoIterator<Item = i64>) -> HashMap<i64, Term> {
        ids.into_iter()
            .filter_map(|id| self.get(id).ok().map(|t| (id, t)))
            .collect()
    }

    fn get(&self, id: i64) -> oxilite_core::Result<Term> {
        match self.known.get(&id) {
            Some(t) => Ok(t.clone()),
            None => self.resolver.get(id),
        }
    }

    fn want(&mut self, id: i64) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.known.entry(id) {
            match oxilite_core::encoding::decode_inline(id) {
                Some(t) => {
                    e.insert(t);
                }
                None => self.resolver.want(id),
            }
        }
    }

    fn next(&mut self) -> oxilite_core::Result<Step<Vec<[Term; 3]>>> {
        if let Some(r) = self.resolver.request(&self.caps) {
            return Ok(Step::Execute(r));
        }
        let mut out = Vec::with_capacity(self.rows.len());
        for [s, p, o] in &self.rows {
            out.push([self.get(*s)?, self.get(*p)?, self.get(*o)?]);
        }
        Ok(Step::Done(out))
    }
}

/// `SELECT` list and joins returning the terms of `p` and `o` with a quad row.
fn with_terms(caps: &Capabilities, t: &str) -> (String, String) {
    (
        format!(
            "{}, tp.lex, tt.lex, tt.dt, tt.lang, tt.dir",
            spo_cols(caps, t)
        ),
        format!(" LEFT JOIN terms tp ON tp.id = {t}.p LEFT JOIN terms tt ON tt.id = {t}.o"),
    )
}

impl Job for QuadFetch {
    type Output = Vec<[Term; 3]>;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<Self::Output>> {
        match self.state {
            FetchState::Start => {
                if self.stmts.is_empty() {
                    self.state = FetchState::Resolving;
                    return self.next();
                }
                self.state = FetchState::Main;
                Ok(Step::Execute(Request::read(std::mem::take(
                    &mut self.stmts,
                ))))
            }
            FetchState::Main => {
                let response = response
                    .ok_or_else(|| oxilite_core::Error::Other("missing response".into()))?;
                for rs in response {
                    for row in rs.rows {
                        let get = |i| {
                            col(&row, i)?
                                .as_i64()
                                .ok_or_else(|| oxilite_core::Error::corrupted("bad id"))
                        };
                        let ids = [get(0)?, get(1)?, get(2)?];
                        if row.len() >= 8 {
                            let text = |i: usize| row.get(i).and_then(|v| v.clone().into_string());
                            if let Some(lex) = text(3) {
                                if let Ok(t) = oxilite_core::encoding::decode_row(
                                    ids[1], lex, None, None, None,
                                ) {
                                    self.known.insert(ids[1], t);
                                }
                            }
                            if let Some(lex) = text(4) {
                                let dir = row.get(7).and_then(|v| v.as_i64());
                                if let Ok(t) = oxilite_core::encoding::decode_row(
                                    ids[2],
                                    lex,
                                    text(5),
                                    text(6),
                                    dir,
                                ) {
                                    self.known.insert(ids[2], t);
                                }
                            }
                        }
                        for id in ids {
                            self.want(id);
                        }
                        self.rows.push(ids);
                    }
                }
                self.state = FetchState::Resolving;
                self.next()
            }
            FetchState::Resolving => {
                let response = response
                    .ok_or_else(|| oxilite_core::Error::Other("missing response".into()))?;
                self.resolver.absorb(response)?;
                self.next()
            }
        }
    }
}

// ----- graph state -----

/// Labels and literal properties of a node or relationship (by reifier).
#[derive(Debug, Clone, Default)]
struct Entity {
    labels: Vec<NamedNode>,
    props: Vec<(NamedNode, Term)>,
    deleted: bool,
}

#[derive(Debug, Clone)]
struct RelCreate {
    s: NamedOrBlankNode,
    p: NamedNode,
    o: NamedOrBlankNode,
    rid: NamedOrBlankNode,
    dropped: bool,
}

#[derive(Debug, Clone)]
struct RelDelete {
    triple: Triple,
    rid: Option<NamedOrBlankNode>,
}

#[derive(Debug, Default)]
struct GraphState {
    nodes: HashMap<NamedOrBlankNode, Entity>,
    rels: HashMap<NamedOrBlankNode, Entity>,
    fetched: HashSet<i64>,
    created_nodes: HashSet<NamedOrBlankNode>,
    created_rels: Vec<RelCreate>,
    provisional: HashSet<NamedOrBlankNode>,
    /// Unreified relationships that got a reifier in this statement.
    reified: HashMap<Triple, NamedOrBlankNode>,
    /// Reifiers that turned out unnecessary (a single plain relationship was created).
    dropped: HashSet<NamedOrBlankNode>,
    node_deletes: Vec<(NamedOrBlankNode, bool)>,
    rel_deletes: Vec<RelDelete>,
    touched: BTreeSet<String>,
    touched_nodes: Vec<NamedOrBlankNode>,
    ops: HashMap<Quad, bool>,
    order: Vec<Quad>,
    merge_cache: HashMap<String, Vec<(String, Value)>>,
    stats: WriteStats,
    counter: u64,
    seed: Option<String>,
}

impl GraphState {
    fn set(&mut self, q: Quad, present: bool) {
        if self.ops.insert(q.clone(), present).is_none() {
            self.order.push(q);
        }
    }

    fn touch(&mut self, n: &NamedOrBlankNode) {
        if self.touched.insert(crate::value::subject_str(n)) {
            self.touched_nodes.push(n.clone());
        }
    }
}

fn to_subject(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(n.clone().into()),
        Term::BlankNode(b) => Some(b.clone().into()),
        _ => None,
    }
}

fn subject_term(s: &NamedOrBlankNode) -> Term {
    match s {
        NamedOrBlankNode::NamedNode(n) => n.clone().into(),
        NamedOrBlankNode::BlankNode(b) => b.clone().into(),
    }
}

/// `rdfs:Resource`: the marker type of nodes created by Cypher.
pub(crate) const RESOURCE: &str = "http://www.w3.org/2000/01/rdf-schema#Resource";

fn reifies() -> NamedNode {
    NamedNode::new_unchecked(REIFIES)
}

fn is_rel_triple(p: &NamedNode, o: &Term) -> bool {
    p.as_ref() != rdf::TYPE
        && p.as_str() != REIFIES
        && matches!(o, Term::NamedNode(_) | Term::BlankNode(_))
}

// ----- the job -----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Start,
    Shapes,
    Main,
    Comprehension,
    Bfs,
    Materialize,
    Fixup1,
    Fixup2,
    Write,
    Finished,
}

/// A Cypher statement being executed. See the module documentation.
pub struct CypherJob {
    plan: Plan,
    params: Params,
    vocab: Vocabulary,
    opts: CypherOptions,
    caps: Capabilities,
    graph: GraphName,
    state: State,
    part: usize,
    raw: Vec<HashMap<Variable, Term>>,
    outputs: Vec<Vec<Value>>,
    g: GraphState,
    fetch: Option<QuadFetch>,
    shapes: Option<std::sync::Arc<Shapes>>,
    fix: FixData,
    schema_changed: bool,
    bfs: Option<Bfs>,
    /// Paths found per shortest-path pattern: (from, to) ids → id sequences.
    found: Vec<HashMap<(i64, i64), Vec<IdPath>>>,
    decoded: HashMap<i64, Term>,
    pending_extra: Vec<i64>,
    /// Rows of the pattern-comprehension queries of the current part.
    comp_raw: Vec<Vec<HashMap<Variable, Term>>>,
}

/// A path as ids: its nodes, and the `(s, p, o)` of each relationship.
type IdPath = (Vec<i64>, Vec<[i64; 3]>);

/// Breadth-first search state of one shortest-path pattern.
struct Bfs {
    spec: usize,
    level: usize,
    /// Per source: parents of every reached node (all parents at its level), the frontier and
    /// the targets still to reach.
    sources: BTreeMap<i64, BfsSource>,
}

#[derive(Default)]
struct BfsSource {
    parents: HashMap<i64, (usize, Vec<(i64, [i64; 3])>)>,
    frontier: Vec<i64>,
    targets: HashSet<i64>,
    done: bool,
}

#[derive(Debug, Default)]
struct FixData {
    incident: Vec<[Term; 3]>,
    existing: HashSet<Triple>,
    reifier_quads: Vec<[Term; 3]>,
}

impl CypherJob {
    pub(crate) fn new(plan: Plan, params: Params, opts: CypherOptions, caps: Capabilities) -> Self {
        let vocab = opts.vocabulary.clone();
        let graph = vocab.graph();
        let shapes = opts.schema.clone();
        Self {
            plan,
            params,
            vocab,
            opts,
            caps,
            graph,
            state: State::Start,
            part: 0,
            raw: Vec::new(),
            outputs: Vec::new(),
            g: GraphState::default(),
            fetch: None,
            shapes,
            fix: FixData::default(),
            schema_changed: false,
            bfs: None,
            found: Vec::new(),
            decoded: HashMap::new(),
            pending_extra: Vec::new(),
            comp_raw: Vec::new(),
        }
    }

    /// The options of the SPARQL query of part `i`: the statement's, plus the value types
    /// the schema gives its property variables.
    pub fn query_options(&self, i: usize) -> oxilite_core::QueryOptions {
        let mut o = self.opts.query.clone();
        if let Some(p) = self.plan.parts.get(i) {
            o.var_types
                .extend(p.var_types.iter().map(|(k, v)| (k.clone(), *v)));
        }
        o
    }

    /// The SPARQL queries the statement runs (one per `UNION` member), with their options.
    pub fn queries(&self) -> Vec<(&spargebra::Query, oxilite_core::QueryOptions)> {
        self.plan
            .parts
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                p.query
                    .iter()
                    .map(move |q| (q, self.query_options(i)))
                    .chain(
                        p.comprehensions
                            .iter()
                            .map(|c| (&c.query, self.opts.query.clone())),
                    )
            })
            .collect()
    }

    /// Planner notes (what runs in Rust and why, capped patterns…).
    pub fn notes(&self) -> &[String] {
        &self.plan.notes
    }

    /// Whether the statement writes.
    pub fn writes(&self) -> bool {
        self.plan.writes
    }

    /// A human-readable plan: SPARQL of each part and what runs in Rust.
    pub fn explain(&self) -> String {
        let mut out = String::new();
        for n in &self.plan.notes {
            out.push_str(&format!("-- {n}\n"));
        }
        for (i, p) in self.plan.parts.iter().enumerate() {
            if self.plan.parts.len() > 1 {
                out.push_str(&format!("-- UNION member {}\n", i + 1));
            }
            match &p.query {
                Some(q) => out.push_str(&format!("-- SPARQL (compiled to SQL):\n{q}\n")),
                None => out.push_str("-- no read\n"),
            }
            let tail: Vec<&str> = p
                .tail
                .iter()
                .filter_map(|t| match t {
                    TailOp::Clause(c) => Some(clause_name(c)),
                    TailOp::Merge { .. } => Some("MERGE"),
                    TailOp::Output(_) => None,
                })
                .collect();
            if !tail.is_empty() {
                out.push_str(&format!("-- evaluated in Rust: {}\n", tail.join(", ")));
            }
            for c in &p.comprehensions {
                out.push_str(&format!(
                    "-- pattern comprehension (SPARQL):\n{}\n",
                    c.query
                ));
            }
            if !p.shortest.is_empty() {
                out.push_str(
                    "-- shortest paths: breadth-first search, one SQL request per level\n",
                );
            }
        }
        out
    }

    /// Advances the job. The first call gets `None`.
    pub fn step(&mut self, input: Option<StepInput>) -> Result<CypherStep> {
        match (self.state, input) {
            (State::Start, _) => {
                crate::temporal::reset_clock();
                if self.plan.writes && self.opts.shapes && self.shapes.is_none() {
                    self.state = State::Shapes;
                    return Ok(CypherStep::Query(
                        crate::schema::schema_query(),
                        oxilite_core::QueryOptions::default(),
                    ));
                }
                self.start_part()
            }
            (State::Shapes, Some(StepInput::Output(out))) => {
                self.shapes = Some(std::sync::Arc::new(Shapes::from_output(out)?));
                self.start_part()
            }
            (State::Main, Some(StepInput::Output(out))) => {
                self.raw = solutions(out)?;
                self.after_main()
            }
            (State::Comprehension, Some(StepInput::Output(out))) => {
                self.comp_raw.push(solutions(out)?);
                self.after_main()
            }
            (State::Materialize, Some(StepInput::Response(r))) => self.drive_fetch(Some(r)),
            (State::Bfs, Some(StepInput::Response(r))) => self.bfs_step(r),
            (State::Fixup1 | State::Fixup2, Some(StepInput::Response(r))) => {
                self.drive_fetch(Some(r))
            }
            (State::Write, Some(StepInput::Response(_))) => self.finish(),
            (s, _) => Err(CypherError::runtime(format!(
                "Cypher job resumed with an unexpected input in state {s:?}"
            ))),
        }
    }

    fn start_part(&mut self) -> Result<CypherStep> {
        if self.part >= self.plan.parts.len() {
            return self.begin_fixup();
        }
        match &self.plan.parts[self.part].query {
            Some(q) => {
                self.state = State::Main;
                Ok(CypherStep::Query(q.clone(), self.query_options(self.part)))
            }
            None => {
                self.raw = vec![HashMap::new()];
                self.after_main()
            }
        }
    }

    fn drive_fetch(&mut self, response: Option<Response>) -> Result<CypherStep> {
        let step = self.fetch.as_mut().expect("active fetch").step(response)?;
        match step {
            Step::Execute(r) => Ok(CypherStep::Sql(r)),
            Step::Done(quads) => {
                let fetch = self.fetch.take().expect("active fetch");
                let extra = std::mem::take(&mut self.pending_extra);
                self.decoded.extend(fetch.decoded(extra));
                match self.state {
                    State::Materialize => {
                        // Path nodes exist even without labels or properties.
                        let ids: Vec<i64> = self
                            .found
                            .iter()
                            .flat_map(|f| f.values().flatten().flat_map(|(ns, _)| ns.clone()))
                            .collect();
                        for id in ids {
                            if let Some(s) = self.decoded.get(&id).and_then(to_subject) {
                                self.g.nodes.entry(s).or_default();
                            }
                        }
                        self.absorb_materialized(quads);
                        self.run_part()
                    }
                    State::Fixup1 => {
                        self.absorb_fixup1(quads);
                        self.fixup2()
                    }
                    State::Fixup2 => {
                        self.fix.reifier_quads = quads;
                        self.apply_fixup()
                    }
                    _ => unreachable!("fetch in state {:?}", self.state),
                }
            }
        }
    }

    fn start_fetch(&mut self, state: State, stmts: Vec<Statement>) -> Result<CypherStep> {
        self.state = state;
        self.fetch = Some(QuadFetch::new(stmts, &self.caps));
        self.drive_fetch(None)
    }

    fn graph_cond(&self, t: &str) -> String {
        format!(
            "{t}.g = {}",
            oxilite_core::encoding::graph_id(self.graph.as_ref())
        )
    }

    // ----- materialization -----

    fn after_main(&mut self) -> Result<CypherStep> {
        let part = &self.plan.parts[self.part];
        if self.found.len() < part.shortest.len() {
            return self.start_bfs();
        }
        if let Some(c) = part.comprehensions.get(self.comp_raw.len()) {
            self.state = State::Comprehension;
            let mut o = self.query_options(self.part);
            o.var_types.clear();
            return Ok(CypherStep::Query(c.query.clone(), o));
        }
        self.materialize()
    }

    // ----- shortest paths -----

    fn start_bfs(&mut self) -> Result<CypherStep> {
        let spec_i = self.found.len();
        let spec = self.plan.parts[self.part].shortest[spec_i].clone();
        let mut sources: BTreeMap<i64, BfsSource> = BTreeMap::new();
        for row in &self.raw {
            let (Some(f), Some(t)) = (
                row.get(&spec.from).and_then(to_subject),
                row.get(&spec.to).and_then(to_subject),
            ) else {
                continue;
            };
            let (f, t) = (subject_id(f.as_ref()), subject_id(t.as_ref()));
            let src = sources.entry(f).or_default();
            src.targets.insert(t);
            if src.frontier.is_empty() {
                src.frontier.push(f);
                src.parents.insert(f, (0, Vec::new()));
            }
        }
        let mut found: HashMap<(i64, i64), Vec<IdPath>> = HashMap::new();
        if spec.min == 0 {
            for (f, src) in &mut sources {
                if src.targets.remove(f) {
                    found.insert((*f, *f), vec![(vec![*f], Vec::new())]);
                }
                src.done = src.targets.is_empty();
            }
        }
        self.found.push(found);
        self.bfs = Some(Bfs {
            spec: spec_i,
            level: 0,
            sources,
        });
        self.bfs_request()
    }

    fn bfs_request(&mut self) -> Result<CypherStep> {
        let bfs = self.bfs.as_ref().expect("bfs");
        let spec = &self.plan.parts[self.part].shortest[bfs.spec];
        let frontier: BTreeSet<i64> = bfs
            .sources
            .values()
            .filter(|s| !s.done)
            .flat_map(|s| s.frontier.iter().copied())
            .collect();
        if frontier.is_empty() || bfs.level >= spec.max {
            return self.finish_bfs();
        }
        let frontier: Vec<i64> = frontier.into_iter().collect();
        let preds = if spec.types.is_empty() {
            format!(
                "(q.o >> 59) IN (1, 2) AND q.p NOT IN ({}, {})",
                rdf_type_id(),
                named_node_id(REIFIES)
            )
        } else {
            format!(
                "q.p IN ({})",
                id_list(
                    &spec
                        .types
                        .iter()
                        .map(|t| named_node_id(t.as_str()))
                        .collect::<Vec<_>>()
                )
            )
        };
        let cols = spo_cols(&self.caps, "q");
        let g = self.graph_cond("q");
        let mut stmts = Vec::new();
        for chunk in frontier.chunks(CHUNK) {
            let l = id_list(chunk);
            if spec.dir != Direction::Left {
                stmts.push(Statement::new(format!(
                    "SELECT {cols} FROM quads q WHERE {g} AND q.s IN ({l}) AND {preds}"
                )));
            }
            if spec.dir != Direction::Right {
                stmts.push(Statement::new(format!(
                    "SELECT {cols} FROM quads q WHERE {g} AND q.o IN ({l}) AND {preds}"
                )));
            }
        }
        self.state = State::Bfs;
        Ok(CypherStep::Sql(Request::read(stmts)))
    }

    fn bfs_step(&mut self, response: Response) -> Result<CypherStep> {
        let spec = {
            let bfs = self.bfs.as_ref().expect("bfs");
            self.plan.parts[self.part].shortest[bfs.spec].clone()
        };
        // Adjacency of this level: node → (neighbour, edge).
        let mut adj: HashMap<i64, Vec<(i64, [i64; 3])>> = HashMap::new();
        for rs in response {
            for row in rs.rows {
                let get = |i| col(&row, i).ok().and_then(|v| v.as_i64());
                let (Some(s), Some(p), Some(o)) = (get(0), get(1), get(2)) else {
                    continue;
                };
                let e = [s, p, o];
                if spec.dir != Direction::Left {
                    adj.entry(s).or_default().push((o, e));
                }
                if spec.dir != Direction::Right && s != o {
                    adj.entry(o).or_default().push((s, e));
                }
            }
        }
        let bfs = self.bfs.as_mut().expect("bfs");
        bfs.level += 1;
        let level = bfs.level;
        let found = self.found.last_mut().expect("found");
        for (src_id, src) in bfs.sources.iter_mut() {
            if src.done {
                continue;
            }
            let mut next = Vec::new();
            for x in std::mem::take(&mut src.frontier) {
                for (y, e) in adj.get(&x).cloned().unwrap_or_default() {
                    // A trail never reuses a relationship: in a shortest path a node repeats
                    // only if the path could be shorter, so reaching a node once suffices.
                    match src.parents.get_mut(&y) {
                        None => {
                            src.parents.insert(y, (level, vec![(x, e)]));
                            next.push(y);
                        }
                        Some((l, ps)) if *l == level && spec.all && !ps.contains(&(x, e)) => {
                            ps.push((x, e));
                        }
                        _ => {}
                    }
                }
            }
            next.sort_unstable();
            next.dedup();
            let reached: Vec<i64> = src
                .targets
                .iter()
                .copied()
                .filter(|t| src.parents.get(t).is_some_and(|(l, _)| *l == level))
                .collect();
            for t in reached {
                src.targets.remove(&t);
                found.insert((*src_id, t), unfold(&src.parents, *src_id, t, spec.all));
            }
            src.frontier = next;
            if src.targets.is_empty() {
                src.done = true;
            }
        }
        self.bfs_request()
    }

    fn finish_bfs(&mut self) -> Result<CypherStep> {
        self.bfs = None;
        self.after_main()
    }

    fn materialize(&mut self) -> Result<CypherStep> {
        let part = &self.plan.parts[self.part];
        // The main rows and the pattern-comprehension rows, with their bindings.
        let mut sets: Vec<(&Vec<HashMap<Variable, Term>>, &crate::lower::Scope)> =
            vec![(&self.raw, &part.scope)];
        for (rows, c) in self.comp_raw.iter().zip(&part.comprehensions) {
            sets.push((rows, &c.scope));
        }
        let mut nodes: BTreeSet<i64> = BTreeSet::new();
        let mut rels: BTreeSet<i64> = BTreeSet::new();
        let add = |t: Option<&Term>, set: &mut BTreeSet<i64>, fetched: &HashSet<i64>| {
            if let Some(s) = t.and_then(to_subject) {
                let id = subject_id(s.as_ref());
                if !fetched.contains(&id) {
                    set.insert(id);
                }
            }
        };
        for (row, scope) in sets
            .iter()
            .flat_map(|(rows, scope)| rows.iter().map(move |r| (r, *scope)))
        {
            for b in scope.values() {
                match b {
                    Bind::Node { var, .. } => add(row.get(var), &mut nodes, &self.g.fetched),
                    Bind::Rel { r, .. } => add(row.get(&r.rid), &mut rels, &self.g.fetched),
                    Bind::Path {
                        nodes: ns,
                        rels: rs,
                        ..
                    } => {
                        for n in ns {
                            add(row.get(n), &mut nodes, &self.g.fetched);
                        }
                        for r in rs {
                            add(row.get(&r.rid), &mut rels, &self.g.fetched);
                        }
                    }
                    Bind::RelList { rels: rs, .. } => {
                        for r in rs {
                            add(row.get(&r.rid), &mut rels, &self.g.fetched);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut extra: BTreeSet<i64> = BTreeSet::new();
        for found in &self.found {
            for paths in found.values() {
                for (ns, rs) in paths {
                    for n in ns {
                        if !self.g.fetched.contains(n) {
                            nodes.insert(*n);
                        }
                        extra.insert(*n);
                    }
                    for [s, p, o] in rs {
                        extra.extend([*s, *p, *o]);
                    }
                }
            }
        }
        if nodes.is_empty() && rels.is_empty() && extra.is_empty() {
            return self.run_part();
        }
        let type_id = rdf_type_id();
        let reifies_id = named_node_id(REIFIES);
        let mut stmts = Vec::new();
        let (cols, joins) = with_terms(&self.caps, "q");
        let g = self.graph_cond("q");
        let nodes: Vec<i64> = nodes.into_iter().collect();
        let rels: Vec<i64> = rels.into_iter().collect();
        // Labels and literal properties (objects that are not IRIs, blank nodes or triples).
        for chunk in nodes.chunks(CHUNK) {
            stmts.push(Statement::new(format!(
                "SELECT {cols} FROM quads q{joins} WHERE q.s IN ({}) AND {g} AND (q.p = {type_id} OR (q.o >> 59) NOT IN (1, 2, 9))",
                id_list(chunk)
            )));
        }
        for chunk in rels.chunks(CHUNK) {
            stmts.push(Statement::new(format!(
                "SELECT {cols} FROM quads q{joins} WHERE q.s IN ({}) AND {g} AND q.p <> {reifies_id} AND (q.o >> 59) NOT IN (1, 2, 9)",
                id_list(chunk)
            )));
        }
        self.g
            .fetched
            .extend(nodes.iter().copied().chain(rels.iter().copied()));
        // Entities without any label or property still exist; their terms are known.
        let mut entities: Vec<(NamedOrBlankNode, bool)> = Vec::new();
        for (row, scope) in sets
            .iter()
            .flat_map(|(rows, scope)| rows.iter().map(move |r| (r, *scope)))
        {
            for b in scope.values() {
                for (var, is_rel) in entity_vars(b) {
                    if let Some(s) = row.get(&var).and_then(to_subject) {
                        entities.push((s, is_rel));
                    }
                }
            }
        }
        let raw = std::mem::take(&mut self.raw);
        let mut known = HashMap::new();
        for (s, is_rel) in entities {
            known.insert(subject_id(s.as_ref()), subject_term(&s));
            let map = if is_rel {
                &mut self.g.rels
            } else {
                &mut self.g.nodes
            };
            map.entry(s).or_default();
        }
        self.raw = raw;
        self.state = State::Materialize;
        self.pending_extra = extra.iter().copied().collect();
        self.fetch = Some(
            QuadFetch::new(stmts, &self.caps)
                .with_known(known)
                .with_extra(extra),
        );
        self.drive_fetch(None)
    }

    fn absorb_materialized(&mut self, quads: Vec<[Term; 3]>) {
        for [s, p, o] in quads {
            let (Some(s), Term::NamedNode(p)) = (to_subject(&s), p) else {
                continue;
            };
            let is_rel = self.g.rels.contains_key(&s);
            let e = if is_rel {
                self.g.rels.entry(s).or_default()
            } else {
                self.g.nodes.entry(s).or_default()
            };
            if p.as_ref() == rdf::TYPE {
                if let Term::NamedNode(l) = o {
                    if l.as_str() != RESOURCE && !e.labels.contains(&l) {
                        e.labels.push(l);
                    }
                }
            } else if matches!(o, Term::Literal(_)) {
                e.props.push((p, o));
            }
        }
    }

    // ----- values -----

    fn prop_values(
        &self,
        owner_labels: &[NamedNode],
        props: &[(NamedNode, Term)],
    ) -> Result<BTreeMap<String, Value>> {
        let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut preds: BTreeMap<String, NamedNode> = BTreeMap::new();
        for (p, o) in props {
            if let Term::Literal(l) = o {
                let name = self.vocab.name(p.as_str());
                preds.entry(name.clone()).or_insert_with(|| p.clone());
                grouped
                    .entry(name)
                    .or_default()
                    .push(Value::from_literal(l));
            }
        }
        let mut out = BTreeMap::new();
        for (k, mut vs) in grouped {
            let scalar = self
                .shapes
                .as_ref()
                .is_some_and(|s| s.is_scalar(owner_labels, &preds[&k]));
            let v = if vs.len() == 1 {
                vs.pop().expect("one")
            } else if scalar {
                vs.sort_by(|a, b| a.order(b));
                vs.into_iter().next().expect("some")
            } else {
                match self.opts.multi_value {
                    MultiValue::List => {
                        vs.sort_by(|a, b| a.order(b));
                        Value::List(vs)
                    }
                    MultiValue::First => {
                        vs.sort_by(|a, b| a.order(b));
                        vs.into_iter().next().expect("some")
                    }
                    MultiValue::Error => {
                        return Err(CypherError::runtime(format!(
                            "property `{k}` has {} values (multi_value = error)",
                            vs.len()
                        )))
                    }
                }
            };
            out.insert(k, v);
        }
        Ok(out)
    }

    fn node_value(&self, id: &NamedOrBlankNode) -> Result<Value> {
        let e = self.g.nodes.get(id).cloned().unwrap_or_default();
        Ok(Value::Node(Node {
            id: id.clone(),
            labels: e
                .labels
                .iter()
                .map(|l| self.vocab.name(l.as_str()))
                .collect(),
            properties: self.prop_values(&e.labels, &e.props)?,
        }))
    }

    fn rel_value(
        &self,
        start: NamedOrBlankNode,
        predicate: NamedNode,
        end: NamedOrBlankNode,
        reifier: Option<NamedOrBlankNode>,
    ) -> Result<Value> {
        let reifier = reifier.or_else(|| {
            self.g
                .reified
                .get(&Triple::new(
                    start.clone(),
                    predicate.clone(),
                    subject_term(&end),
                ))
                .cloned()
        });
        let reifier = reifier.filter(|r| !self.g.dropped.contains(r));
        let properties = match &reifier {
            Some(r) => {
                let e = self.g.rels.get(r).cloned().unwrap_or_default();
                self.prop_values(&[], &e.props)?
            }
            None => BTreeMap::new(),
        };
        Ok(Value::Relationship(Relationship {
            rel_type: self.vocab.name(predicate.as_str()),
            start,
            predicate,
            end,
            reifier,
            properties,
        }))
    }

    /// Re-reads entity values from the current state (after writes).
    fn refresh(&self, v: &mut Value) -> Result<()> {
        match v {
            Value::Node(n) => *v = self.node_value(&n.id)?,
            Value::Relationship(r) => {
                *v = self.rel_value(
                    r.start.clone(),
                    r.predicate.clone(),
                    r.end.clone(),
                    r.reifier.clone(),
                )?
            }
            Value::Path(p) => {
                for n in &mut p.nodes {
                    if let Value::Node(x) = self.node_value(&n.id)? {
                        *n = x;
                    }
                }
                for r in &mut p.relationships {
                    if let Value::Relationship(x) = self.rel_value(
                        r.start.clone(),
                        r.predicate.clone(),
                        r.end.clone(),
                        r.reifier.clone(),
                    )? {
                        *r = x;
                    }
                }
            }
            Value::List(items) => {
                for i in items {
                    self.refresh(i)?;
                }
            }
            Value::Map(m) => {
                for i in m.values_mut() {
                    self.refresh(i)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn refresh_rows(&self, rows: &mut [Row]) -> Result<()> {
        for r in rows {
            for v in r.values_mut() {
                self.refresh(v)?;
            }
        }
        Ok(())
    }

    fn id_path_value(&self, ns: &[i64], rs: &[[i64; 3]]) -> Result<Value> {
        let term = |id: &i64| {
            self.decoded
                .get(id)
                .cloned()
                .ok_or_else(|| CypherError::runtime(format!("term {id} of a path was not decoded")))
        };
        let mut nodes = Vec::new();
        for n in ns {
            let s = to_subject(&term(n)?)
                .ok_or_else(|| CypherError::runtime("path node is not a resource"))?;
            if let Value::Node(x) = self.node_value(&s)? {
                nodes.push(x);
            }
        }
        let mut rels = Vec::new();
        for [s, p, o] in rs {
            let (Some(s), Term::NamedNode(p), Some(o)) =
                (to_subject(&term(s)?), term(p)?, to_subject(&term(o)?))
            else {
                return Err(CypherError::runtime("invalid path relationship"));
            };
            if let Value::Relationship(r) = self.rel_value(s, p, o, None)? {
                rels.push(r);
            }
        }
        Ok(Value::Path(Path {
            nodes,
            relationships: rels,
        }))
    }

    fn rel_from_vars(&self, row: &HashMap<Variable, Term>, r: &RelVars) -> Result<Value> {
        let (Some(s), Some(Term::NamedNode(p)), Some(o)) = (
            row.get(&r.s).and_then(to_subject),
            row.get(&r.p).cloned(),
            row.get(&r.o).and_then(to_subject),
        ) else {
            return Ok(Value::Null);
        };
        let rid = row.get(&r.rid).and_then(to_subject);
        self.rel_value(s, p, o, rid)
    }

    fn bind_value(&self, b: &Bind, row: &HashMap<Variable, Term>) -> Result<Value> {
        Ok(match b {
            Bind::Node { var, .. } => match row.get(var).and_then(to_subject) {
                Some(s) => self.node_value(&s)?,
                None => Value::Null,
            },
            Bind::Rel { r, .. } => self.rel_from_vars(row, r)?,
            Bind::Value { var, name, .. } => match row.get(var) {
                None => Value::Null,
                Some(Term::NamedNode(n)) if *name => Value::String(self.vocab.name(n.as_str())),
                Some(t) => Value::from_term(t),
            },
            Bind::Map { fields, .. } => Value::Map(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), row.get(v).map_or(Value::Null, Value::from_term)))
                    .collect(),
            ),
            Bind::Path {
                len, nodes, rels, ..
            } => {
                let Some(n) = row.get(len).map(Value::from_term).and_then(|v| match v {
                    Value::Int(i) => Some(i as usize),
                    _ => None,
                }) else {
                    return Ok(Value::Null);
                };
                let mut ns = Vec::new();
                for v in nodes.iter().take(n + 1) {
                    match row.get(v).and_then(to_subject) {
                        Some(s) => {
                            if let Value::Node(x) = self.node_value(&s)? {
                                ns.push(x);
                            }
                        }
                        None => return Ok(Value::Null),
                    }
                }
                let mut rs = Vec::new();
                for r in rels.iter().take(n) {
                    match self.rel_from_vars(row, r)? {
                        Value::Relationship(x) => rs.push(x),
                        _ => return Ok(Value::Null),
                    }
                }
                Value::Path(Path {
                    nodes: ns,
                    relationships: rs,
                })
            }
            Bind::RelList { len, rels, .. } => {
                let Some(n) = row.get(len).map(Value::from_term).and_then(|v| match v {
                    Value::Int(i) => Some(i as usize),
                    _ => None,
                }) else {
                    return Ok(Value::Null);
                };
                let mut out = Vec::new();
                for r in rels.iter().take(n) {
                    out.push(self.rel_from_vars(row, r)?);
                }
                Value::List(out)
            }
        })
    }

    // ----- running a part -----

    fn run_part(&mut self) -> Result<CypherStep> {
        let part = self.plan.parts[self.part].clone();
        let raw = std::mem::take(&mut self.raw);
        let mut rows: Vec<Row> = Vec::with_capacity(raw.len());
        if let Some(proc_) = &part.procedure {
            for r in &raw {
                let mut row = Row::new();
                for (col, alias) in &proc_.yields {
                    let v = Variable::new_unchecked(format!("proc_{col}"));
                    let val = match r.get(&v) {
                        None => Value::Null,
                        Some(Term::NamedNode(n)) => Value::String(self.vocab.name(n.as_str())),
                        Some(t) => Value::from_term(t),
                    };
                    row.insert(alias.clone(), val);
                }
                rows.push(row);
            }
            if let Some(w) = &proc_.where_ {
                let params = self.params.clone();
                rows = rows
                    .into_iter()
                    .filter_map(|r| match eval_pred(w, &Env::new(&r, &params)) {
                        Ok(true) => Some(Ok(r)),
                        Ok(false) => None,
                        Err(e) => Some(Err(e)),
                    })
                    .collect::<Result<_>>()?;
            }
        } else {
            for r in &raw {
                let mut row = Row::new();
                for (name, b) in &part.scope {
                    row.insert(name.clone(), self.bind_value(b, r)?);
                }
                rows.push(row);
            }
        }
        // Pattern comprehensions: each row gets the list of its outer values' matches.
        let comp_raw = std::mem::take(&mut self.comp_raw);
        for (c, crows) in part.comprehensions.iter().zip(&comp_raw) {
            let mut lists: HashMap<Vec<Option<Term>>, BTreeMap<String, Vec<Value>>> =
                HashMap::new();
            for cr in crows {
                let mut env_row = Row::new();
                for (name, b) in &c.scope {
                    env_row.insert(name.clone(), self.bind_value(b, cr)?);
                }
                let env = Env::new(&env_row, &self.params);
                if let Some(f) = &c.filter {
                    if !eval_pred(f, &env)? {
                        continue;
                    }
                }
                let key: Vec<Option<Term>> = c.keys.iter().map(|k| cr.get(k).cloned()).collect();
                let free = c
                    .free
                    .iter()
                    .map(|v| {
                        cr.get(v)
                            .and_then(to_subject)
                            .map_or_else(String::new, |s| crate::value::subject_str(&s))
                    })
                    .collect::<Vec<_>>()
                    .join("|");
                lists
                    .entry(key)
                    .or_default()
                    .entry(free)
                    .or_default()
                    .push(eval(&c.map, &env)?);
            }
            for (row, r) in rows.iter_mut().zip(&raw) {
                let key: Vec<Option<Term>> = c.keys.iter().map(|k| r.get(k).cloned()).collect();
                let by_free = lists.get(&key).cloned().unwrap_or_default();
                let v = if c.free.is_empty() {
                    Value::List(by_free.into_values().next().unwrap_or_default())
                } else {
                    Value::Map(
                        by_free
                            .into_iter()
                            .map(|(k, l)| (k, Value::List(l)))
                            .collect(),
                    )
                };
                row.insert(c.name.clone(), v);
            }
        }
        for (si, spec) in part.shortest.iter().enumerate() {
            let mut out = Vec::new();
            for (row, r) in rows.into_iter().zip(&raw) {
                let (Some(f), Some(t)) = (
                    r.get(&spec.from).and_then(to_subject),
                    r.get(&spec.to).and_then(to_subject),
                ) else {
                    continue;
                };
                let key = (subject_id(f.as_ref()), subject_id(t.as_ref()));
                for (ns, rs) in self.found[si].get(&key).cloned().unwrap_or_default() {
                    let path = self.id_path_value(&ns, &rs)?;
                    let mut row2 = row.clone();
                    if let Some(rv) = &spec.rel {
                        if let Value::Path(p) = &path {
                            row2.insert(
                                rv.clone(),
                                Value::List(
                                    p.relationships
                                        .iter()
                                        .cloned()
                                        .map(Value::Relationship)
                                        .collect(),
                                ),
                            );
                        }
                    }
                    row2.insert(spec.path.clone(), path);
                    out.push(row2);
                }
            }
            // Keep rows aligned with raw rows for the next pattern.
            rows = out;
            if si + 1 < part.shortest.len() {
                return Err(CypherError::unsupported(
                    "several shortestPath() patterns in one MATCH",
                ));
            }
        }
        self.found.clear();
        let mut columns: Option<Vec<String>> = None;
        for op in &part.tail {
            match op {
                TailOp::Output(cols) => {
                    columns = Some(cols.clone());
                }
                TailOp::Clause(Clause::Return(p)) => {
                    self.refresh_rows(&mut rows)?;
                    let (r, cols) = self.project(rows, p)?;
                    rows = r;
                    columns = Some(cols);
                }
                TailOp::Clause(c) => {
                    self.refresh_rows(&mut rows)?;
                    rows = self.exec_clause(c, rows)?;
                }
                TailOp::Merge {
                    part: pp,
                    on_create,
                    on_match,
                    lookup,
                } => {
                    self.refresh_rows(&mut rows)?;
                    rows = self.merge(pp, on_create, on_match, lookup.as_deref(), rows)?;
                }
            }
        }
        if let Some(cols) = columns {
            for r in rows {
                let mut out = Vec::with_capacity(cols.len());
                for c in &cols {
                    out.push(r.get(c).cloned().unwrap_or(Value::Null));
                }
                self.outputs.push(out);
            }
        }
        self.part += 1;
        self.start_part()
    }

    // ----- projections in Rust -----

    fn project(&self, rows: Vec<Row>, p: &Projection) -> Result<(Vec<Row>, Vec<String>)> {
        let mut items: Vec<(String, Expr)> = Vec::new();
        if p.star {
            let mut names: BTreeSet<String> = BTreeSet::new();
            for r in &rows {
                names.extend(r.keys().filter(|k| !k.starts_with('\u{1}')).cloned());
            }
            for n in names {
                items.push((n.clone(), Expr::Var(n)));
            }
        }
        for it in &p.items {
            items.push((it.name(), it.expr.clone()));
        }
        let names: Vec<String> = items.iter().map(|(n, _)| n.clone()).collect();
        let params = &self.params;
        let aggregating = items.iter().any(|(_, e)| e.has_aggregate());
        // (projected row, env for ORDER BY)
        let mut out: Vec<(Row, Row)> = Vec::new();
        if aggregating {
            let mut aggs: Vec<AggSpec> = Vec::new();
            let rewritten: Vec<(String, Expr, bool)> = items
                .iter()
                .map(|(n, e)| {
                    if e.has_aggregate() {
                        (n.clone(), extract_aggregates(e, &mut aggs), true)
                    } else {
                        (n.clone(), e.clone(), false)
                    }
                })
                .collect();
            let mut groups: Vec<(Vec<GroupKey>, Row, Vec<Acc>, Vec<HashSet<GroupKey>>)> =
                Vec::new();
            let mut index: HashMap<Vec<GroupKey>, usize> = HashMap::new();
            for r in &rows {
                let env = Env::new(r, params);
                let mut key = Vec::new();
                let mut keyrow = Row::new();
                for (n, e, is_agg) in &rewritten {
                    if !*is_agg {
                        let v = eval(e, &env)?;
                        key.push(v.group_key());
                        keyrow.insert(n.clone(), v);
                    }
                }
                let gi = match index.get(&key) {
                    Some(i) => *i,
                    None => {
                        let accs = aggs
                            .iter()
                            .map(|a| Acc::new(&a.func))
                            .collect::<Result<Vec<_>>>()?;
                        groups.push((key.clone(), keyrow, accs, vec![HashSet::new(); aggs.len()]));
                        index.insert(key, groups.len() - 1);
                        groups.len() - 1
                    }
                };
                for (ai, a) in aggs.iter().enumerate() {
                    if let (Some(e), Acc::Percentile { p: pp, .. }) =
                        (&a.arg2, &mut groups[gi].2[ai])
                    {
                        if let Some(x) = eval(e, &env)?.as_f64() {
                            if !(0.0..=1.0).contains(&x) {
                                return Err(CypherError::runtime(
                                    "percentile must be between 0.0 and 1.0",
                                ));
                            }
                            *pp = x;
                        }
                    }
                    let v = match &a.arg {
                        None => Value::Bool(true),
                        Some(e) => eval(e, &env)?,
                    };
                    if a.distinct && !v.is_null() && !groups[gi].3[ai].insert(v.group_key()) {
                        continue;
                    }
                    groups[gi].2[ai].add(v)?;
                }
            }
            if groups.is_empty() && rewritten.iter().all(|(_, _, a)| *a) {
                let accs = aggs
                    .iter()
                    .map(|a| Acc::new(&a.func))
                    .collect::<Result<Vec<_>>>()?;
                groups.push((Vec::new(), Row::new(), accs, Vec::new()));
            }
            for (_, keyrow, accs, _) in groups {
                let mut env_row = keyrow.clone();
                for (a, acc) in aggs.iter().zip(accs) {
                    env_row.insert(a.placeholder.clone(), acc.finish());
                }
                let mut row = Row::new();
                for (n, e, is_agg) in &rewritten {
                    let v = if *is_agg {
                        eval(e, &Env::new(&env_row, params))?
                    } else {
                        keyrow.get(n).cloned().unwrap_or(Value::Null)
                    };
                    row.insert(n.clone(), v);
                }
                out.push((row.clone(), row));
            }
        } else {
            for r in &rows {
                let env = Env::new(r, params);
                let mut row = Row::new();
                for (n, e) in &items {
                    row.insert(n.clone(), eval(e, &env)?);
                }
                let mut order_env = r.clone();
                order_env.extend(row.clone());
                out.push((row, order_env));
            }
        }
        // WITH … WHERE over variables of the previous scope filters before DISTINCT/ORDER.
        let mut early_where = false;
        if let (Some(w), false, None, None) = (&p.where_, aggregating, &p.skip, &p.limit) {
            let mut old_refs = false;
            w.walk(&mut |x| {
                if let Expr::Var(v) = x {
                    if !names.contains(v) {
                        old_refs = true;
                    }
                }
            });
            if old_refs {
                let mut kept = Vec::new();
                for (r, env_row) in out {
                    if eval_pred(w, &Env::new(&env_row, params))? {
                        kept.push((r, env_row));
                    }
                }
                out = kept;
                early_where = true;
            }
        }
        if p.distinct {
            let mut seen = HashSet::new();
            out.retain(|(r, _)| {
                let key: Vec<GroupKey> = names
                    .iter()
                    .map(|n| r.get(n).map_or(GroupKey::Null, Value::group_key))
                    .collect();
                seen.insert(key)
            });
        }
        if !p.order.is_empty() {
            // Order keys read projected values for the projected expressions they contain.
            let targets: Vec<(Expr, Expr)> = items
                .iter()
                .map(|(n, ie)| (ie.clone(), Expr::Var(n.clone())))
                .collect();
            let order: Vec<Expr> = p
                .order
                .iter()
                .map(|(e, _)| e.replace_subexprs(&targets))
                .collect();
            let mut keyed: Vec<(Vec<Value>, Row)> = Vec::with_capacity(out.len());
            for (r, env_row) in out {
                let env = Env::new(&env_row, params);
                let mut k = Vec::new();
                for e in &order {
                    k.push(eval(e, &env)?);
                }
                keyed.push((k, r));
            }
            keyed.sort_by(|(a, _), (b, _)| {
                for (i, (_, asc)) in p.order.iter().enumerate() {
                    let o = a[i].order(&b[i]);
                    let o = if *asc { o } else { o.reverse() };
                    if o != std::cmp::Ordering::Equal {
                        return o;
                    }
                }
                std::cmp::Ordering::Equal
            });
            out = keyed.into_iter().map(|(_, r)| (r.clone(), r)).collect();
        }
        let empty = Row::new();
        let count = |e: &Option<Expr>| -> Result<Option<usize>> {
            match e {
                None => Ok(None),
                Some(e) => match eval(e, &Env::new(&empty, params))? {
                    Value::Int(i) if i >= 0 => Ok(Some(i as usize)),
                    _ => Err(CypherError::semantic(
                        "SKIP and LIMIT need a non-negative integer",
                    )),
                },
            }
        };
        let skip = count(&p.skip)?.unwrap_or(0);
        let limit = count(&p.limit)?;
        let mut rows: Vec<Row> = out.into_iter().map(|(r, _)| r).skip(skip).collect();
        if let Some(l) = limit {
            rows.truncate(l);
        }
        if let (Some(w), false) = (&p.where_, early_where) {
            let mut kept = Vec::new();
            for r in rows {
                if eval_pred(w, &Env::new(&r, params))? {
                    kept.push(r);
                }
            }
            rows = kept;
        }
        Ok((rows, names))
    }

    // ----- clauses in Rust -----

    fn exec_clause(&mut self, c: &Clause, rows: Vec<Row>) -> Result<Vec<Row>> {
        let params = self.params.clone();
        Ok(match c {
            Clause::With(p) => self.project(rows, p)?.0,
            Clause::Unwind { expr, alias } => {
                let mut out = Vec::new();
                for r in rows {
                    match eval(expr, &Env::new(&r, &params))? {
                        Value::Null => {}
                        Value::List(items) => {
                            for i in items {
                                let mut r2 = r.clone();
                                r2.insert(alias.clone(), i);
                                out.push(r2);
                            }
                        }
                        v => {
                            let mut r2 = r;
                            r2.insert(alias.clone(), v);
                            out.push(r2);
                        }
                    }
                }
                out
            }
            Clause::Create(parts) => {
                let mut out = Vec::with_capacity(rows.len());
                for mut r in rows {
                    for p in parts {
                        self.create_part(p, &mut r)?;
                    }
                    out.push(r);
                }
                out
            }
            Clause::Set(items) => {
                let mut out = Vec::with_capacity(rows.len());
                for mut r in rows {
                    for it in items {
                        self.set_item(it, &mut r)?;
                    }
                    out.push(r);
                }
                out
            }
            Clause::Remove(items) => {
                let mut out = Vec::with_capacity(rows.len());
                for mut r in rows {
                    for it in items {
                        self.remove_item(it, &mut r)?;
                    }
                    out.push(r);
                }
                out
            }
            Clause::Delete { detach, exprs } => {
                for r in &rows {
                    for e in exprs {
                        let v = eval(e, &Env::new(r, &params))?;
                        self.delete_value(v, *detach)?;
                    }
                }
                rows
            }
            other => {
                return Err(CypherError::unsupported(format!(
                    "{} in this position",
                    clause_name(other)
                )))
            }
        })
    }

    fn mint(&mut self, prefix: &str) -> NamedOrBlankNode {
        self.g.counter += 1;
        let seed = self
            .g
            .seed
            .get_or_insert_with(|| oxrdf::BlankNode::default().as_str().to_string())
            .clone();
        NamedNode::new_unchecked(format!("{prefix}{seed}-{}", self.g.counter)).into()
    }

    fn literal_props(&self, e: Option<&Expr>, row: &Row) -> Result<Vec<(NamedNode, Term)>> {
        let Some(e) = e else {
            return Ok(Vec::new());
        };
        let v = eval(e, &Env::new(row, &self.params))?;
        let Value::Map(m) = v else {
            return Err(CypherError::runtime("properties must be a map"));
        };
        let mut out = Vec::new();
        for (k, v) in m {
            if let Some(l) = v.to_literal()? {
                out.push((self.vocab.iri(&k), l.into()));
            }
        }
        Ok(out)
    }

    fn create_node(&mut self, np: &NodePattern, row: &mut Row) -> Result<NamedOrBlankNode> {
        if let Some(name) = &np.var {
            match row.get(name) {
                Some(Value::Node(n)) => {
                    if !np.labels.is_empty() || np.props.is_some() {
                        return Err(CypherError::semantic(format!(
                            "node `{name}` is already bound: it cannot get labels or properties in CREATE"
                        )));
                    }
                    return Ok(n.id.clone());
                }
                Some(Value::Null) => {
                    return Err(CypherError::runtime(format!(
                        "cannot create a relationship with the null node `{name}`"
                    )))
                }
                Some(other) => {
                    return Err(CypherError::semantic(format!(
                        "`{name}` is a {}, not a node",
                        other.type_name()
                    )))
                }
                None => {}
            }
        }
        let id = self.mint(&self.vocab.node_prefix.clone());
        let props = self.literal_props(np.props.as_ref(), row)?;
        let labels: Vec<NamedNode> = np.labels.iter().map(|l| self.vocab.iri(l)).collect();
        if self.opts.node_marker {
            self.g.set(
                Quad::new(
                    id.clone(),
                    rdf::TYPE.into_owned(),
                    NamedNode::new_unchecked(RESOURCE),
                    self.graph.clone(),
                ),
                true,
            );
        }
        for l in &labels {
            self.g.set(
                Quad::new(
                    id.clone(),
                    rdf::TYPE.into_owned(),
                    l.clone(),
                    self.graph.clone(),
                ),
                true,
            );
        }
        for (p, o) in &props {
            self.g.set(
                Quad::new(id.clone(), p.clone(), o.clone(), self.graph.clone()),
                true,
            );
        }
        self.g.stats.nodes_created += 1;
        self.g.stats.labels_added += labels.len() as u64;
        self.g.stats.properties_set += props.len() as u64;
        self.g.nodes.insert(
            id.clone(),
            Entity {
                labels,
                props,
                deleted: false,
            },
        );
        self.g.created_nodes.insert(id.clone());
        self.g.touch(&id);
        if let Some(name) = &np.var {
            row.insert(name.clone(), self.node_value(&id)?);
        }
        Ok(id)
    }

    fn create_part(&mut self, p: &PatternPart, row: &mut Row) -> Result<()> {
        let mut nodes = vec![self.create_node(&p.element.start, row)?];
        let mut rels = Vec::new();
        for (rel, np) in &p.element.chain {
            let end = self.create_node(np, row)?;
            let start = nodes.last().expect("start").clone();
            if rel.length.is_some() {
                return Err(CypherError::semantic(
                    "variable-length relationships cannot be created",
                ));
            }
            let [t] = rel.types.as_slice() else {
                return Err(CypherError::semantic(
                    "a created relationship needs exactly one type",
                ));
            };
            let (s, o) = match rel.dir {
                Direction::Right => (start, end.clone()),
                Direction::Left => (end.clone(), start),
                Direction::Both => {
                    return Err(CypherError::semantic(
                        "a created relationship needs a direction",
                    ))
                }
            };
            if let Some(name) = &rel.var {
                if row.contains_key(name) {
                    return Err(CypherError::semantic(format!(
                        "relationship `{name}` is already bound"
                    )));
                }
            }
            let pred = self.vocab.iri(t);
            let rid = self.mint(&self.vocab.relationship_prefix.clone());
            let props = self.literal_props(rel.props.as_ref(), row)?;
            self.g.stats.relationships_created += 1;
            self.g.stats.properties_set += props.len() as u64;
            self.g.rels.insert(
                rid.clone(),
                Entity {
                    labels: Vec::new(),
                    props,
                    deleted: false,
                },
            );
            self.g.provisional.insert(rid.clone());
            self.g.created_rels.push(RelCreate {
                s: s.clone(),
                p: pred.clone(),
                o: o.clone(),
                rid: rid.clone(),
                dropped: false,
            });
            let v = self.rel_value(s, pred, o, Some(rid))?;
            if let Some(name) = &rel.var {
                row.insert(name.clone(), v.clone());
            }
            if let Value::Relationship(r) = v {
                rels.push(r);
            }
            nodes.push(end);
        }
        if let Some(pv) = &p.var {
            let mut ns = Vec::new();
            for n in &nodes {
                if let Value::Node(x) = self.node_value(n)? {
                    ns.push(x);
                }
            }
            row.insert(
                pv.clone(),
                Value::Path(Path {
                    nodes: ns,
                    relationships: rels,
                }),
            );
        }
        Ok(())
    }

    fn merge_key(&self, p: &PatternPart, row: &Row) -> Result<String> {
        let env = Env::new(row, &self.params);
        let node = |np: &NodePattern| -> Result<String> {
            if let Some(Value::Node(n)) = np.var.as_ref().and_then(|v| row.get(v)) {
                return Ok(format!("#{}", n.element_id()));
            }
            let mut labels = np.labels.clone();
            labels.sort();
            let props = match &np.props {
                Some(e) => eval(e, &env)?.to_string(),
                None => String::new(),
            };
            Ok(format!("({}{props})", labels.join(":")))
        };
        let mut key = node(&p.element.start)?;
        for (r, n) in &p.element.chain {
            let props = match &r.props {
                Some(e) => eval(e, &env)?.to_string(),
                None => String::new(),
            };
            key.push_str(&format!("-[{}{props}|{:?}]-", r.types.join("|"), r.dir));
            key.push_str(&node(n)?);
        }
        Ok(key)
    }

    fn merge(
        &mut self,
        p: &PatternPart,
        on_create: &[SetItem],
        on_match: &[SetItem],
        lookup: Option<&str>,
        rows: Vec<Row>,
    ) -> Result<Vec<Row>> {
        let vars = crate::plan::pattern_vars(p);
        let mut out = Vec::with_capacity(rows.len());
        for mut r in rows {
            let mut matched = lookup.is_some_and(|m| r.remove(m) == Some(Value::Bool(true)));
            // Nodes deleted earlier in the statement no longer match.
            if matched
                && vars.iter().any(|v| {
                    matches!(r.get(v), Some(Value::Node(n)) if self.g.nodes.get(&n.id).is_some_and(|e| e.deleted))
                })
            {
                matched = false;
                for v in &vars {
                    r.remove(v);
                }
            }
            if matched {
                for it in on_match {
                    self.set_item(it, &mut r)?;
                }
                out.push(r);
                continue;
            }
            // New variables of the pattern are unbound when it did not match.
            for v in &vars {
                if r.get(v).is_some_and(Value::is_null) {
                    r.remove(v);
                }
            }
            let key = self.merge_key(p, &r)?;
            if let Some(bound) = self.g.merge_cache.get(&key).cloned() {
                for (k, v) in bound {
                    r.insert(k, v);
                }
                self.refresh_rows(std::slice::from_mut(&mut r))?;
                for it in on_match {
                    self.set_item(it, &mut r)?;
                }
                out.push(r);
                continue;
            }
            if p.element
                .chain
                .iter()
                .any(|(rel, _)| rel.dir == Direction::Both)
            {
                return Err(CypherError::unsupported(
                    "MERGE of an undirected relationship",
                ));
            }
            self.create_part(p, &mut r)?;
            for it in on_create {
                self.set_item(it, &mut r)?;
            }
            let bound: Vec<(String, Value)> = vars
                .iter()
                .filter_map(|v| r.get(v).map(|x| (v.clone(), x.clone())))
                .collect();
            self.g.merge_cache.insert(key, bound);
            out.push(r);
        }
        Ok(out)
    }

    /// The entity map and subject that `SET`/`REMOVE` on `var` modify.
    fn target(&mut self, var: &str, row: &Row) -> Result<Option<(NamedOrBlankNode, bool)>> {
        match row.get(var) {
            None => Err(CypherError::semantic(format!(
                "variable `{var}` not defined"
            ))),
            Some(Value::Null) => Ok(None),
            Some(Value::Node(n)) => Ok(Some((n.id.clone(), false))),
            Some(Value::Relationship(r)) => {
                let rid = match r
                    .reifier
                    .clone()
                    .filter(|x| !self.g.dropped.contains(x))
                    .or_else(|| {
                        self.g
                            .reified
                            .get(&Triple::new(
                                r.start.clone(),
                                r.predicate.clone(),
                                subject_term(&r.end),
                            ))
                            .cloned()
                    }) {
                    Some(rid) => rid,
                    None => {
                        // Properties need a reifier.
                        let rid = self.mint(&self.vocab.relationship_prefix.clone());
                        let t =
                            Triple::new(r.start.clone(), r.predicate.clone(), subject_term(&r.end));
                        self.g.set(
                            Quad::new(
                                rid.clone(),
                                reifies(),
                                Term::Triple(Box::new(t.clone())),
                                self.graph.clone(),
                            ),
                            true,
                        );
                        self.g.reified.insert(t, rid.clone());
                        self.g.rels.insert(rid.clone(), Entity::default());
                        rid
                    }
                };
                Ok(Some((rid, true)))
            }
            Some(other) => Err(CypherError::runtime(format!(
                "cannot set properties on a {}",
                other.type_name()
            ))),
        }
    }

    fn set_prop(
        &mut self,
        subject: &NamedOrBlankNode,
        is_rel: bool,
        key: &NamedNode,
        value: Option<Term>,
    ) {
        let provisional = self.g.provisional.contains(subject);
        let map = if is_rel {
            &mut self.g.rels
        } else {
            &mut self.g.nodes
        };
        let e = map.entry(subject.clone()).or_default();
        let old: Vec<Term> = e
            .props
            .iter()
            .filter(|(p, _)| p == key)
            .map(|(_, o)| o.clone())
            .collect();
        e.props.retain(|(p, _)| p != key);
        if let Some(v) = &value {
            e.props.push((key.clone(), v.clone()));
        }
        if !provisional {
            for o in old {
                self.g.set(
                    Quad::new(subject.clone(), key.clone(), o, self.graph.clone()),
                    false,
                );
            }
            if let Some(v) = value {
                self.g.set(
                    Quad::new(subject.clone(), key.clone(), v, self.graph.clone()),
                    true,
                );
            }
        }
        self.g.stats.properties_set += 1;
        if !is_rel {
            self.g.touch(subject);
        }
    }

    fn set_item(&mut self, it: &SetItem, row: &mut Row) -> Result<()> {
        let params = self.params.clone();
        match it {
            SetItem::Property { var, key, value } => {
                let v = eval(value, &Env::new(row, &params))?;
                if let Some((s, is_rel)) = self.target(var, row)? {
                    let l = v.to_literal()?.map(Term::from);
                    self.set_prop(&s, is_rel, &self.vocab.iri(key), l);
                }
            }
            SetItem::Replace { var, value } | SetItem::Merge { var, value } => {
                let v = eval(value, &Env::new(row, &params))?;
                let m = match v {
                    Value::Map(m) => m,
                    Value::Node(n) => n.properties,
                    Value::Relationship(r) => r.properties,
                    Value::Null => BTreeMap::new(),
                    o => {
                        return Err(CypherError::runtime(format!(
                            "SET … = expects a map, got {}",
                            o.type_name()
                        )))
                    }
                };
                if let Some((s, is_rel)) = self.target(var, row)? {
                    if matches!(it, SetItem::Replace { .. }) {
                        let existing: Vec<NamedNode> = {
                            let map = if is_rel { &self.g.rels } else { &self.g.nodes };
                            map.get(&s)
                                .map(|e| e.props.iter().map(|(p, _)| p.clone()).collect())
                                .unwrap_or_default()
                        };
                        let mut done = BTreeSet::new();
                        for p in existing {
                            if done.insert(p.clone())
                                && !m.contains_key(&self.vocab.name(p.as_str()))
                            {
                                self.set_prop(&s, is_rel, &p, None);
                            }
                        }
                    }
                    for (k, v) in m {
                        let l = v.to_literal()?.map(Term::from);
                        self.set_prop(&s, is_rel, &self.vocab.iri(&k), l);
                    }
                }
            }
            SetItem::Labels { var, labels } => match row.get(var) {
                Some(Value::Node(n)) => {
                    let id = n.id.clone();
                    for l in labels {
                        let iri = self.vocab.iri(l);
                        let e = self.g.nodes.entry(id.clone()).or_default();
                        if !e.labels.contains(&iri) {
                            e.labels.push(iri.clone());
                            self.g.stats.labels_added += 1;
                        }
                        self.g.set(
                            Quad::new(id.clone(), rdf::TYPE.into_owned(), iri, self.graph.clone()),
                            true,
                        );
                    }
                    self.g.touch(&id);
                }
                Some(Value::Null) => {}
                _ => return Err(CypherError::semantic(format!("`{var}` is not a node"))),
            },
        }
        self.refresh_rows(std::slice::from_mut(row))
    }

    fn remove_item(&mut self, it: &RemoveItem, row: &mut Row) -> Result<()> {
        match it {
            RemoveItem::Property { var, key } => {
                if let Some((s, is_rel)) = self.target(var, row)? {
                    self.set_prop(&s, is_rel, &self.vocab.iri(key), None);
                }
            }
            RemoveItem::Labels { var, labels } => match row.get(var) {
                Some(Value::Node(n)) => {
                    let id = n.id.clone();
                    for l in labels {
                        let iri = self.vocab.iri(l);
                        let e = self.g.nodes.entry(id.clone()).or_default();
                        if e.labels.contains(&iri) {
                            e.labels.retain(|x| *x != iri);
                            self.g.stats.labels_removed += 1;
                        }
                        self.g.set(
                            Quad::new(id.clone(), rdf::TYPE.into_owned(), iri, self.graph.clone()),
                            false,
                        );
                    }
                    self.g.touch(&id);
                }
                Some(Value::Null) => {}
                _ => return Err(CypherError::semantic(format!("`{var}` is not a node"))),
            },
        }
        self.refresh_rows(std::slice::from_mut(row))
    }

    fn delete_value(&mut self, v: Value, detach: bool) -> Result<()> {
        match v {
            Value::Null => {}
            Value::Node(n) => {
                let e = self.g.nodes.entry(n.id.clone()).or_default();
                if !e.deleted {
                    e.deleted = true;
                    self.g.node_deletes.push((n.id, detach));
                }
            }
            Value::Relationship(r) => {
                if let Some(rid) = &r.reifier {
                    if self.g.provisional.contains(rid) {
                        for c in &mut self.g.created_rels {
                            if &c.rid == rid && !c.dropped {
                                c.dropped = true;
                                self.g.stats.relationships_deleted += 1;
                            }
                        }
                        return Ok(());
                    }
                }
                let triple =
                    Triple::new(r.start.clone(), r.predicate.clone(), subject_term(&r.end));
                let rid = r
                    .reifier
                    .clone()
                    .or_else(|| self.g.reified.get(&triple).cloned());
                if !self
                    .g
                    .rel_deletes
                    .iter()
                    .any(|d| d.triple == triple && d.rid == rid)
                {
                    self.g.rel_deletes.push(RelDelete { triple, rid });
                    self.g.stats.relationships_deleted += 1;
                }
            }
            Value::Path(p) => {
                for r in p.relationships {
                    self.delete_value(Value::Relationship(r), detach)?;
                }
                for n in p.nodes {
                    self.delete_value(Value::Node(n), detach)?;
                }
            }
            Value::List(items) => {
                for i in items {
                    self.delete_value(i, detach)?;
                }
            }
            other => {
                return Err(CypherError::runtime(format!(
                    "cannot delete a {}",
                    other.type_name()
                )))
            }
        }
        Ok(())
    }

    // ----- fix-up and write -----

    fn has_writes(&self) -> bool {
        !self.g.ops.is_empty()
            || !self.g.created_rels.is_empty()
            || !self.g.node_deletes.is_empty()
            || !self.g.rel_deletes.is_empty()
    }

    fn begin_fixup(&mut self) -> Result<CypherStep> {
        if !self.has_writes() {
            return self.finish();
        }
        let cols = spo_cols(&self.caps, "q");
        let g = self.graph_cond("q");
        let mut stmts = Vec::new();
        let dels: Vec<i64> = self
            .g
            .node_deletes
            .iter()
            .filter(|(n, _)| !self.g.created_nodes.contains(n))
            .map(|(n, _)| subject_id(n.as_ref()))
            .collect();
        for chunk in dels.chunks(CHUNK) {
            let l = id_list(chunk);
            stmts.push(Statement::new(format!(
                "SELECT {cols} FROM quads q WHERE {g} AND (q.s IN ({l}) OR q.o IN ({l}))"
            )));
        }
        // Do the triples of created relationships already exist?
        let mut conds = Vec::new();
        for c in &self.g.created_rels {
            if c.dropped
                || self.g.created_nodes.contains(&c.s)
                || self.g.created_nodes.contains(&c.o)
            {
                continue;
            }
            conds.push(format!(
                "(q.s = {} AND q.p = {} AND q.o = {})",
                subject_id(c.s.as_ref()),
                named_node_id(c.p.as_str()),
                subject_id(c.o.as_ref())
            ));
        }
        conds.sort();
        conds.dedup();
        for chunk in conds.chunks(100) {
            stmts.push(Statement::new(format!(
                "SELECT {cols} FROM quads q WHERE {g} AND ({})",
                chunk.join(" OR ")
            )));
        }
        self.start_fetch(State::Fixup1, stmts)
    }

    fn absorb_fixup1(&mut self, quads: Vec<[Term; 3]>) {
        let dels: HashSet<Term> = self
            .g
            .node_deletes
            .iter()
            .map(|(n, _)| subject_term(n))
            .collect();
        for q in quads {
            let [s, p, o] = &q;
            if dels.contains(s) || dels.contains(o) {
                self.fix.incident.push(q.clone());
            }
            if let (Some(s), Term::NamedNode(p)) = (to_subject(s), p) {
                self.fix
                    .existing
                    .insert(Triple::new(s, p.clone(), o.clone()));
            }
        }
    }

    /// Relationship triples whose reifiers matter for the fix-up.
    fn fix_triples(&self) -> Vec<Triple> {
        let mut out: Vec<Triple> = Vec::new();
        for c in &self.g.created_rels {
            if !c.dropped {
                out.push(Triple::new(c.s.clone(), c.p.clone(), subject_term(&c.o)));
            }
        }
        for d in &self.g.rel_deletes {
            out.push(d.triple.clone());
        }
        for [s, p, o] in &self.fix.incident {
            if let (Some(s), Term::NamedNode(p)) = (to_subject(s), p) {
                if is_rel_triple(p, o) {
                    out.push(Triple::new(s, p.clone(), o.clone()));
                }
            }
        }
        out.sort_by_key(|t| t.to_string());
        out.dedup();
        out
    }

    fn fixup2(&mut self) -> Result<CypherStep> {
        let tids: Vec<i64> = self
            .fix_triples()
            .iter()
            .map(|t| triple_id(t.as_ref()))
            .collect();
        let cols = spo_cols(&self.caps, "q");
        let reifies_id = named_node_id(REIFIES);
        let (gr, gq) = (self.graph_cond("r"), self.graph_cond("q"));
        let mut stmts = Vec::new();
        for chunk in tids.chunks(CHUNK) {
            // Every quad of every reifier of these triples.
            stmts.push(Statement::new(format!(
                "SELECT {cols} FROM quads r JOIN quads q ON q.s = r.s WHERE r.p = {reifies_id} AND r.o IN ({}) AND {gr} AND {gq}",
                id_list(chunk)
            )));
        }
        self.start_fetch(State::Fixup2, stmts)
    }

    fn apply_fixup(&mut self) -> Result<CypherStep> {
        let graph = self.graph.clone();
        // Reifiers of each triple, and all their quads.
        let mut reifiers: HashMap<Triple, Vec<NamedOrBlankNode>> = HashMap::new();
        let mut rid_quads: HashMap<NamedOrBlankNode, Vec<Quad>> = HashMap::new();
        for [s, p, o] in std::mem::take(&mut self.fix.reifier_quads) {
            let (Some(s), Term::NamedNode(p)) = (to_subject(&s), p) else {
                continue;
            };
            if p.as_str() == REIFIES {
                if let Term::Triple(t) = &o {
                    let list = reifiers.entry((**t).clone()).or_default();
                    if !list.contains(&s) {
                        list.push(s.clone());
                    }
                }
            }
            rid_quads
                .entry(s.clone())
                .or_default()
                .push(Quad::new(s, p, o, graph.clone()));
        }
        // Deleted relationships.
        let mut deleted_rids: HashSet<NamedOrBlankNode> = HashSet::new();
        let mut touched_triples: Vec<Triple> = Vec::new();
        for d in self.g.rel_deletes.clone() {
            match &d.rid {
                Some(rid) => {
                    for q in rid_quads.get(rid).cloned().unwrap_or_default() {
                        self.g.set(q, false);
                    }
                    self.g.set(
                        Quad::new(
                            rid.clone(),
                            reifies(),
                            Term::Triple(Box::new(d.triple.clone())),
                            graph.clone(),
                        ),
                        false,
                    );
                    deleted_rids.insert(rid.clone());
                    touched_triples.push(d.triple.clone());
                }
                None => {
                    self.g.set(
                        Quad::new(
                            d.triple.subject.clone(),
                            d.triple.predicate.clone(),
                            d.triple.object.clone(),
                            graph.clone(),
                        ),
                        false,
                    );
                }
            }
        }
        // Deleted nodes.
        let deleted_nodes: HashSet<NamedOrBlankNode> =
            self.g.node_deletes.iter().map(|(n, _)| n.clone()).collect();
        let deleted_triples: HashSet<Triple> = self
            .g
            .rel_deletes
            .iter()
            .filter(|d| d.rid.is_none())
            .map(|d| d.triple.clone())
            .collect();
        for (n, detach) in self.g.node_deletes.clone() {
            let nt = subject_term(&n);
            // Relationships created in this statement.
            for c in &mut self.g.created_rels {
                if !c.dropped && (c.s == n || c.o == n) {
                    if !detach {
                        return Err(CypherError::runtime(format!(
                            "cannot delete node <{}>: it still has relationships (use DETACH DELETE)",
                            crate::value::subject_str(&n)
                        )));
                    }
                    c.dropped = true;
                }
            }
            if self.g.created_nodes.contains(&n) {
                let quads: Vec<Quad> = self
                    .g
                    .order
                    .iter()
                    .filter(|q| q.subject == n)
                    .cloned()
                    .collect();
                for q in quads {
                    self.g.set(q, false);
                }
                self.g.stats.nodes_deleted += 1;
                continue;
            }
            let incident: Vec<[Term; 3]> = self
                .fix
                .incident
                .iter()
                .filter(|[s, _, o]| *s == nt || *o == nt)
                .cloned()
                .collect();
            for [s, p, o] in &incident {
                let (Some(s), Term::NamedNode(p)) = (to_subject(s), p) else {
                    continue;
                };
                let rel = is_rel_triple(p, o)
                    || (*o == nt && p.as_ref() != rdf::TYPE && p.as_str() != REIFIES);
                let triple = Triple::new(s.clone(), p.clone(), o.clone());
                if rel {
                    let rids = reifiers.get(&triple).cloned().unwrap_or_default();
                    let already = deleted_triples.contains(&triple)
                        || (!rids.is_empty() && rids.iter().all(|r| deleted_rids.contains(r)))
                        || (to_subject(o).is_some_and(|x| deleted_nodes.contains(&x))
                            && s == n
                            && detach);
                    if !detach && !already {
                        return Err(CypherError::runtime(format!(
                            "cannot delete node <{}>: it still has relationships (use DETACH DELETE)",
                            crate::value::subject_str(&n)
                        )));
                    }
                    if !already && !deleted_triples.contains(&triple) {
                        self.g.stats.relationships_deleted += 1;
                    }
                    for rid in rids {
                        for q in rid_quads.get(&rid).cloned().unwrap_or_default() {
                            self.g.set(q, false);
                        }
                        deleted_rids.insert(rid);
                    }
                }
                self.g
                    .set(Quad::new(s, p.clone(), o.clone(), graph.clone()), false);
            }
            self.g.stats.nodes_deleted += 1;
        }
        // Asserted triples whose last reifier was deleted.
        for t in touched_triples {
            let remaining = reifiers
                .get(&t)
                .map(|r| r.iter().filter(|x| !deleted_rids.contains(x)).count())
                .unwrap_or(0);
            let recreated = self.g.created_rels.iter().any(|c| {
                !c.dropped
                    && c.s == t.subject
                    && c.p == t.predicate
                    && subject_term(&c.o) == t.object
            });
            if remaining == 0 && !recreated {
                self.g.set(
                    Quad::new(
                        t.subject.clone(),
                        t.predicate.clone(),
                        t.object.clone(),
                        graph.clone(),
                    ),
                    false,
                );
            }
        }
        // Created relationships: a single plain one is just a triple; otherwise every
        // relationship of the triple gets a reifier.
        let mut groups: Vec<(Triple, Vec<RelCreate>)> = Vec::new();
        for c in self.g.created_rels.clone() {
            if c.dropped {
                self.g.dropped.insert(c.rid.clone());
                continue;
            }
            let t = Triple::new(c.s.clone(), c.p.clone(), subject_term(&c.o));
            match groups.iter_mut().find(|(x, _)| *x == t) {
                Some((_, v)) => v.push(c),
                None => groups.push((t, vec![c])),
            }
        }
        for (t, cs) in groups {
            let deleted_now = self.g.ops.get(&Quad::new(
                t.subject.clone(),
                t.predicate.clone(),
                t.object.clone(),
                graph.clone(),
            )) == Some(&false);
            let exists = self.fix.existing.contains(&t) && !deleted_now;
            let existing_rids: Vec<NamedOrBlankNode> = reifiers
                .get(&t)
                .map(|r| {
                    r.iter()
                        .filter(|x| !deleted_rids.contains(x))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let plain = !exists
                && cs.len() == 1
                && self
                    .g
                    .rels
                    .get(&cs[0].rid)
                    .is_none_or(|e| e.props.is_empty());
            self.g.set(
                Quad::new(
                    t.subject.clone(),
                    t.predicate.clone(),
                    t.object.clone(),
                    graph.clone(),
                ),
                true,
            );
            if plain {
                self.g.dropped.insert(cs[0].rid.clone());
                continue;
            }
            if exists && existing_rids.is_empty() && !self.g.reified.contains_key(&t) {
                // The relationship already stored becomes one of several: reify it too.
                let old = self.mint(&self.vocab.relationship_prefix.clone());
                self.g.set(
                    Quad::new(
                        old.clone(),
                        reifies(),
                        Term::Triple(Box::new(t.clone())),
                        graph.clone(),
                    ),
                    true,
                );
                self.g.reified.insert(t.clone(), old);
            }
            for c in cs {
                self.g.set(
                    Quad::new(
                        c.rid.clone(),
                        reifies(),
                        Term::Triple(Box::new(t.clone())),
                        graph.clone(),
                    ),
                    true,
                );
                let props = self
                    .g
                    .rels
                    .get(&c.rid)
                    .map(|e| e.props.clone())
                    .unwrap_or_default();
                for (p, o) in props {
                    self.g
                        .set(Quad::new(c.rid.clone(), p, o, graph.clone()), true);
                }
            }
        }
        // SHACL shapes of every node the statement created or changed.
        if let Some(shapes) = &self.shapes {
            for n in &self.g.touched_nodes {
                let e = self.g.nodes.get(n).cloned().unwrap_or_default();
                if e.deleted {
                    continue;
                }
                shapes.check(n, &e.labels, &e.props, &self.vocab)?;
            }
        }
        let deletes: Vec<Quad> = self
            .g
            .order
            .iter()
            .filter(|q| self.g.ops.get(*q) == Some(&false))
            .cloned()
            .collect();
        let inserts: Vec<Quad> = self
            .g
            .order
            .iter()
            .filter(|q| self.g.ops.get(*q) == Some(&true))
            .cloned()
            .collect();
        if deletes.is_empty() && inserts.is_empty() {
            return self.finish();
        }
        let schema = deletes
            .iter()
            .chain(&inserts)
            .any(|q| oxilite_core::reason::is_schema_quad(q.as_ref()));
        let mut stmts =
            EncodedQuads::new(deletes.iter().map(Quad::as_ref)).delete_statements(&self.caps);
        stmts.extend(
            EncodedQuads::new(inserts.iter().map(Quad::as_ref)).insert_statements(&self.caps),
        );
        if schema {
            stmts.extend(oxilite_core::reason::closure_statements());
        }
        self.schema_changed = schema;
        self.state = State::Write;
        Ok(CypherStep::Write(Request::atomic(stmts)))
    }

    fn finish(&mut self) -> Result<CypherStep> {
        self.state = State::Finished;
        let mut rows = std::mem::take(&mut self.outputs);
        if !self.g.dropped.is_empty() || !self.g.reified.is_empty() {
            for r in &mut rows {
                for v in r.iter_mut() {
                    patch_reifiers(v, &self.g.dropped, &self.g.reified);
                }
            }
        }
        // UNION (without ALL) removes duplicates.
        if self.plan.parts.len() > 1 && self.plan.union_all.iter().any(|a| !*a) {
            let mut seen = HashSet::new();
            rows.retain(|r| seen.insert(r.iter().map(Value::group_key).collect::<Vec<_>>()));
        }
        Ok(CypherStep::Done(CypherResult {
            columns: self.plan.columns.clone(),
            rows,
            stats: std::mem::take(&mut self.g.stats),
            schema_changed: self.schema_changed,
        }))
    }
}

fn solutions(out: QueryOutput) -> Result<Vec<HashMap<Variable, Term>>> {
    match out {
        QueryOutput::Solutions { variables, rows } => Ok(rows
            .into_iter()
            .map(|r| {
                variables
                    .iter()
                    .cloned()
                    .zip(r)
                    .filter_map(|(v, t)| t.map(|t| (v, t)))
                    .collect()
            })
            .collect()),
        _ => Err(CypherError::runtime("unexpected query output")),
    }
}

fn patch_reifiers(
    v: &mut Value,
    dropped: &HashSet<NamedOrBlankNode>,
    reified: &HashMap<Triple, NamedOrBlankNode>,
) {
    match v {
        Value::Relationship(r) => {
            if r.reifier.as_ref().is_some_and(|x| dropped.contains(x)) {
                r.reifier = None;
            }
            if r.reifier.is_none() {
                r.reifier = reified
                    .get(&Triple::new(
                        r.start.clone(),
                        r.predicate.clone(),
                        subject_term(&r.end),
                    ))
                    .cloned();
            }
        }
        Value::Path(p) => {
            for r in &mut p.relationships {
                let mut x = Value::Relationship(r.clone());
                patch_reifiers(&mut x, dropped, reified);
                if let Value::Relationship(x) = x {
                    *r = x;
                }
            }
        }
        Value::List(items) => items
            .iter_mut()
            .for_each(|i| patch_reifiers(i, dropped, reified)),
        Value::Map(m) => m
            .values_mut()
            .for_each(|i| patch_reifiers(i, dropped, reified)),
        _ => {}
    }
}

fn entity_vars(b: &Bind) -> Vec<(Variable, bool)> {
    match b {
        Bind::Node { var, .. } => vec![(var.clone(), false)],
        Bind::Rel { r, .. } => vec![(r.rid.clone(), true)],
        Bind::Path { nodes, rels, .. } => nodes
            .iter()
            .map(|n| (n.clone(), false))
            .chain(rels.iter().map(|r| (r.rid.clone(), true)))
            .collect(),
        Bind::RelList { rels, .. } => rels.iter().map(|r| (r.rid.clone(), true)).collect(),
        _ => Vec::new(),
    }
}

/// All shortest paths (or the first) from `src` to `t`, following the recorded parents.
fn unfold(
    parents: &HashMap<i64, (usize, Vec<(i64, [i64; 3])>)>,
    src: i64,
    t: i64,
    all: bool,
) -> Vec<IdPath> {
    const MAX_PATHS: usize = 10_000;
    let mut out = Vec::new();
    // Depth-first over parent choices, building paths backwards.
    let mut stack: Vec<(i64, Vec<i64>, Vec<[i64; 3]>)> = vec![(t, vec![t], Vec::new())];
    while let Some((n, ns, rs)) = stack.pop() {
        if n == src && rs.len() == parents.get(&t).map_or(0, |(l, _)| *l) {
            let mut ns = ns;
            let mut rs = rs;
            ns.reverse();
            rs.reverse();
            out.push((ns, rs));
            if !all || out.len() >= MAX_PATHS {
                break;
            }
            continue;
        }
        let Some((_, ps)) = parents.get(&n) else {
            continue;
        };
        let choices: &[(i64, [i64; 3])] = if all { ps } else { &ps[..ps.len().min(1)] };
        for (p, e) in choices.iter().rev() {
            if rs.contains(e) {
                continue;
            }
            let mut ns2 = ns.clone();
            ns2.push(*p);
            let mut rs2 = rs.clone();
            rs2.push(*e);
            stack.push((*p, ns2, rs2));
        }
    }
    out
}

struct AggSpec {
    placeholder: String,
    func: String,
    distinct: bool,
    arg: Option<Expr>,
    arg2: Option<Expr>,
}

fn extract_aggregates(e: &Expr, aggs: &mut Vec<AggSpec>) -> Expr {
    match e {
        Expr::CountStar => {
            let placeholder = format!("\u{1}agg{}", aggs.len());
            aggs.push(AggSpec {
                placeholder: placeholder.clone(),
                func: "count".into(),
                distinct: false,
                arg: None,
                arg2: None,
            });
            Expr::Var(placeholder)
        }
        Expr::Func {
            name,
            distinct,
            args,
        } if is_aggregate(name) => {
            let placeholder = format!("\u{1}agg{}", aggs.len());
            aggs.push(AggSpec {
                placeholder: placeholder.clone(),
                func: name.clone(),
                distinct: *distinct,
                arg: args.first().cloned(),
                arg2: args.get(1).cloned(),
            });
            Expr::Var(placeholder)
        }
        Expr::Binary(op, a, b) => Expr::Binary(
            *op,
            Box::new(extract_aggregates(a, aggs)),
            Box::new(extract_aggregates(b, aggs)),
        ),
        Expr::Unary(op, a) => Expr::Unary(*op, Box::new(extract_aggregates(a, aggs))),
        Expr::Func {
            name,
            distinct,
            args,
        } => Expr::Func {
            name: name.clone(),
            distinct: *distinct,
            args: args.iter().map(|a| extract_aggregates(a, aggs)).collect(),
        },
        Expr::List(items) => {
            Expr::List(items.iter().map(|a| extract_aggregates(a, aggs)).collect())
        }
        // The list of a comprehension or quantifier may be an aggregate (not the body).
        Expr::ListComp {
            var,
            list,
            filter,
            map,
        } => Expr::ListComp {
            var: var.clone(),
            list: Box::new(extract_aggregates(list, aggs)),
            filter: filter.clone(),
            map: map.clone(),
        },
        Expr::Quantified { q, var, list, pred } => Expr::Quantified {
            q: *q,
            var: var.clone(),
            list: Box::new(extract_aggregates(list, aggs)),
            pred: pred.clone(),
        },
        Expr::Map(items) => Expr::Map(
            items
                .iter()
                .map(|(k, a)| (k.clone(), extract_aggregates(a, aggs)))
                .collect(),
        ),
        Expr::Index(a, b) => Expr::Index(
            Box::new(extract_aggregates(a, aggs)),
            Box::new(extract_aggregates(b, aggs)),
        ),
        Expr::Prop(a, k) => Expr::Prop(Box::new(extract_aggregates(a, aggs)), k.clone()),
        Expr::Case {
            operand,
            whens,
            else_,
        } => Expr::Case {
            operand: operand
                .as_ref()
                .map(|o| Box::new(extract_aggregates(o, aggs))),
            whens: whens
                .iter()
                .map(|(w, t)| (extract_aggregates(w, aggs), extract_aggregates(t, aggs)))
                .collect(),
            else_: else_
                .as_ref()
                .map(|o| Box::new(extract_aggregates(o, aggs))),
        },
        other => other.clone(),
    }
}

pub(crate) fn clause_name(c: &Clause) -> &'static str {
    match c {
        Clause::Match { optional: true, .. } => "OPTIONAL MATCH",
        Clause::Match { .. } => "MATCH",
        Clause::Unwind { .. } => "UNWIND",
        Clause::With(_) => "WITH",
        Clause::Return(_) => "RETURN",
        Clause::Create(_) => "CREATE",
        Clause::Merge { .. } => "MERGE",
        Clause::Set(_) => "SET",
        Clause::Remove(_) => "REMOVE",
        Clause::Delete { detach: true, .. } => "DETACH DELETE",
        Clause::Delete { .. } => "DELETE",
        Clause::Call { .. } => "CALL",
    }
}
