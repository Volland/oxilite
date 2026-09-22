//! SPARQL algebra → SQL.
//!
//! Each graph pattern compiles to a [`Block`]: FROM items, WHERE conditions, variable
//! bindings and solution-modifier state. Blocks stay "plain" (a single SELECT-FROM-WHERE)
//! for as long as possible so that SQLite sees one flat join; they are sealed into
//! subqueries only when SQL semantics require it.
//!
// @lat: [[architecture#SPARQL to SQL compiler]]

pub mod expr;
pub mod plan;

use crate::encoding::{encode_literal, named_node_id, term_id, EncodedRows, DEFAULT_GRAPH_ID};
use crate::error::{Error, Result};
use crate::sql::Capabilities;
use crate::stats::Stats;
use expr::V;
use oxrdf::{Literal, Term, Variable};
use plan::Pos;
use spargebra::algebra::{Expression, GraphPattern, OrderExpression, QueryDataset};
use spargebra::term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

/// Per-query options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryOptions {
    /// Treat the default graph as the union of all graphs (Oxigraph's `union_default_graph`).
    pub union_default_graph: bool,
    /// Let SQLite choose the join order instead of the oxilite planner (for benchmarking).
    pub sqlite_planner: bool,
}

/// How a variable is represented in SQL.
#[derive(Debug, Clone)]
pub(crate) enum Col {
    /// A term id column.
    Id(String),
    /// A computed value (see [`V`]).
    Val(Box<V>),
}

impl Col {
    /// SQL of the term id usable as a join key.
    pub(crate) fn key(&self) -> Option<&str> {
        match self {
            Self::Id(x) => Some(x),
            Self::Val(v) => v.id.as_deref(),
        }
    }

    pub(crate) fn value(&self) -> V {
        match self {
            Self::Id(x) => V::from_id(x),
            Self::Val(v) => (**v).clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Binding {
    pub col: Col,
    pub nullable: bool,
    /// The column is an expression (not a plain column of a FROM item); such bindings must
    /// be sealed before being placed on the right of a LEFT JOIN.
    #[allow(dead_code)]
    pub computed: bool,
    /// Already equated with an outer (EXISTS) binding.
    pub correlated: bool,
}

impl Binding {
    fn id(sql: impl Into<String>) -> Self {
        Self {
            col: Col::Id(sql.into()),
            nullable: false,
            computed: false,
            correlated: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Join {
    First,
    Cross,
    Inner,
    /// `LEFT JOIN … ON (condition)` (OPTIONAL).
    #[allow(dead_code)]
    Left(String),
}

#[derive(Debug, Clone)]
pub(crate) struct FromItem {
    pub join: Join,
    pub item: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Stage {
    #[default]
    Plain,
    Grouped,
    Ordered,
    Distinct,
    Sliced,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Block {
    pub from: Vec<FromItem>,
    pub wheres: Vec<String>,
    pub cols: BTreeMap<usize, Binding>,
    pub group_by: Option<Vec<String>>,
    pub order_by: Vec<String>,
    pub distinct: bool,
    pub limit: Option<usize>,
    pub offset: usize,
    pub stage: Stage,
    /// Extra select-list entries (e.g. the aggregate that makes a bare column meaningful).
    pub extra_select: Vec<String>,
}

pub(crate) const VAL_FIELDS: [&str; 9] = ["i", "k", "l", "d", "g", "n", "t", "s", "b"];

pub(crate) fn val_fields(v: &V) -> [String; 9] {
    [
        v.id.clone().unwrap_or_else(|| "NULL".into()),
        v.kind.clone(),
        if v.computed_num {
            "NULL".into()
        } else {
            v.lex.clone()
        },
        v.dt.clone(),
        v.lang.clone(),
        v.num.clone(),
        v.nt.clone(),
        v.ts.clone(),
        v.boolv.clone(),
    ]
}

impl Block {
    pub(crate) fn is_plain(&self) -> bool {
        self.stage == Stage::Plain
    }

    pub(crate) fn is_unit(&self) -> bool {
        self.from.is_empty() && self.wheres.is_empty() && self.cols.is_empty()
    }

    pub(crate) fn render_from(items: &[FromItem]) -> String {
        let mut out = String::new();
        for (i, f) in items.iter().enumerate() {
            if i == 0 {
                out.push_str(&f.item);
                continue;
            }
            match &f.join {
                Join::First | Join::Inner => {
                    let _ = write!(out, " JOIN {}", f.item);
                }
                Join::Cross => {
                    let _ = write!(out, " CROSS JOIN {}", f.item);
                }
                Join::Left(on) => {
                    let _ = write!(out, " LEFT JOIN {} ON ({on})", f.item);
                }
            }
        }
        out
    }

    /// Renders the select list entries of the given variables.
    fn select_list(&self, vars: &[usize], text_ids: bool) -> Vec<String> {
        let mut sel = Vec::new();
        for idx in vars {
            match self.cols.get(idx).map(|b| &b.col) {
                Some(Col::Id(x)) => sel.push(if text_ids {
                    format!("CAST({x} AS TEXT) AS v{idx}")
                } else {
                    format!("{x} AS v{idx}")
                }),
                Some(Col::Val(v)) => {
                    for (suffix, field) in VAL_FIELDS.iter().zip(val_fields(v)) {
                        if text_ids && *suffix == "i" {
                            sel.push(format!("CAST({field} AS TEXT) AS v{idx}_{suffix}"));
                        } else {
                            sel.push(format!("{field} AS v{idx}_{suffix}"));
                        }
                    }
                }
                None => sel.push(format!("NULL AS v{idx}")),
            }
        }
        sel
    }

    /// Renders the block as a SELECT statement.
    pub(crate) fn to_select(&self, vars: Option<&[usize]>, text_ids: bool) -> String {
        let all: Vec<usize> = self.cols.keys().copied().collect();
        let mut sel = self.select_list(vars.unwrap_or(&all), text_ids);
        sel.extend(self.extra_select.iter().cloned());
        if sel.is_empty() {
            sel.push("1 AS _u".into());
        }
        let mut sql = String::from("SELECT ");
        if self.distinct {
            sql.push_str("DISTINCT ");
        }
        sql.push_str(&sel.join(", "));
        if !self.from.is_empty() {
            sql.push_str(" FROM ");
            sql.push_str(&Self::render_from(&self.from));
        }
        if !self.wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.wheres.join(" AND "));
        }
        if let Some(g) = &self.group_by {
            if !g.is_empty() {
                sql.push_str(" GROUP BY ");
                sql.push_str(&g.join(", "));
            }
        }
        if !self.order_by.is_empty() {
            sql.push_str(" ORDER BY ");
            sql.push_str(&self.order_by.join(", "));
        }
        if self.limit.is_some() || self.offset > 0 {
            let _ = write!(
                sql,
                " LIMIT {}",
                self.limit.map_or_else(|| "-1".into(), |l| l.to_string())
            );
            if self.offset > 0 {
                let _ = write!(sql, " OFFSET {}", self.offset);
            }
        }
        sql
    }
}

#[derive(Debug, Clone)]
enum DefaultGraph {
    Zero,
    Union,
    List(Vec<i64>),
}

#[derive(Debug, Clone)]
struct Dataset {
    default: DefaultGraph,
    named: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Copy)]
enum GraphScope {
    Default,
    Fixed(i64),
    Var(usize),
}

/// The SPARQL → SQL compiler state for one query or update.
pub(crate) struct Compiler<'a> {
    pub stats: &'a Stats,
    pub caps: &'a Capabilities,
    pub options: &'a QueryOptions,
    vars: HashMap<Variable, usize>,
    pub var_names: Vec<Variable>,
    aliases: usize,
    /// Constants appearing in the query: decodable without a lookup.
    pub constants: HashMap<i64, Term>,
    /// Bindings visible from enclosing EXISTS scopes.
    pub outer: Vec<BTreeMap<usize, Binding>>,
    pub now: Literal,
    pub base_iri: Option<String>,
    dataset: Dataset,
    scope: GraphScope,
    /// Rows needed by constants that end up stored (update templates).
    pub rows: EncodedRows,
}

impl<'a> Compiler<'a> {
    pub(crate) fn new(
        stats: &'a Stats,
        caps: &'a Capabilities,
        options: &'a QueryOptions,
        dataset: Option<&QueryDataset>,
        base_iri: Option<String>,
    ) -> Self {
        let dataset = match dataset {
            Some(ds) => Dataset {
                default: DefaultGraph::List(
                    ds.default
                        .iter()
                        .map(|g| named_node_id(g.as_str()))
                        .collect(),
                ),
                named: Some(
                    ds.named
                        .iter()
                        .flatten()
                        .map(|g| named_node_id(g.as_str()))
                        .collect(),
                ),
            },
            None => Dataset {
                default: if options.union_default_graph {
                    DefaultGraph::Union
                } else {
                    DefaultGraph::Zero
                },
                named: None,
            },
        };
        Self {
            stats,
            caps,
            options,
            vars: HashMap::new(),
            var_names: Vec::new(),
            aliases: 0,
            constants: HashMap::new(),
            outer: Vec::new(),
            now: Literal::from(oxsdatatypes::DateTime::now()),
            base_iri,
            dataset,
            scope: GraphScope::Default,
            rows: EncodedRows::default(),
        }
    }

    pub(crate) fn var(&mut self, v: &Variable) -> usize {
        if let Some(i) = self.vars.get(v) {
            return *i;
        }
        let i = self.var_names.len();
        self.vars.insert(v.clone(), i);
        self.var_names.push(v.clone());
        i
    }

    pub(crate) fn fresh_var(&mut self, hint: &str) -> usize {
        let v = Variable::new_unchecked(format!("\u{1}{hint}{}", self.var_names.len()));
        self.var(&v)
    }

    pub(crate) fn alias(&mut self, prefix: &str) -> String {
        self.aliases += 1;
        format!("{prefix}{}", self.aliases)
    }

    /// Encodes a constant term and remembers it for decoding.
    pub(crate) fn constant_id(&mut self, t: &Term) -> Result<i64> {
        let id = match t {
            Term::Triple(tr) => {
                let id = self.rows.triple(tr.as_ref().as_ref());
                self.constants.insert(id, t.clone());
                // Components are decodable too.
                self.constants.insert(
                    term_id(tr.subject.as_ref().into()),
                    tr.subject.clone().into(),
                );
                self.constants.insert(
                    named_node_id(tr.predicate.as_str()),
                    tr.predicate.clone().into(),
                );
                self.constants
                    .insert(term_id(tr.object.as_ref()), tr.object.clone());
                return Ok(id);
            }
            Term::Literal(l) => encode_literal(l.as_ref()).0,
            _ => term_id(t.as_ref()),
        };
        self.constants.insert(id, t.clone());
        Ok(id)
    }

    fn outer_binding(&self, idx: usize) -> Option<&Binding> {
        self.outer.iter().rev().find_map(|s| s.get(&idx))
    }

    /// Wraps a block into a subquery.
    pub(crate) fn seal(&mut self, b: Block) -> Block {
        let alias = self.alias("s");
        let sql = b.to_select(None, false);
        let mut cols = BTreeMap::new();
        for (idx, bind) in &b.cols {
            let col = match &bind.col {
                Col::Id(_) => Col::Id(format!("{alias}.v{idx}")),
                Col::Val(v) => Col::Val(Box::new(V {
                    id: v.id.as_ref().map(|_| format!("{alias}.v{idx}_i")),
                    kind: format!("{alias}.v{idx}_k"),
                    lex: format!("{alias}.v{idx}_l"),
                    dt: format!("{alias}.v{idx}_d"),
                    lang: format!("{alias}.v{idx}_g"),
                    num: format!("{alias}.v{idx}_n"),
                    nt: format!("{alias}.v{idx}_t"),
                    ts: format!("{alias}.v{idx}_s"),
                    boolv: format!("{alias}.v{idx}_b"),
                    stat: v.stat,
                    computed_num: false,
                    decodable: v.decodable,
                })),
            };
            cols.insert(
                *idx,
                Binding {
                    col,
                    nullable: bind.nullable,
                    computed: false,
                    correlated: false,
                },
            );
        }
        // A sealed computed number keeps a NULL lexical form: re-derive it from num/nt.
        for (idx, bind) in &b.cols {
            if let Col::Val(v) = &bind.col {
                if v.computed_num {
                    if let Some(Binding {
                        col: Col::Val(sv), ..
                    }) = cols.get_mut(idx)
                    {
                        let n = V::numeric(sv.num.clone(), sv.nt.clone());
                        sv.lex = n.lex;
                        sv.computed_num = true;
                    }
                }
            }
        }
        Block {
            from: vec![FromItem {
                join: Join::First,
                item: format!("({sql}) AS {alias}"),
            }],
            cols,
            ..Block::default()
        }
    }

    fn plain(&mut self, b: Block) -> Block {
        if b.is_plain() {
            b
        } else {
            self.seal(b)
        }
    }

    /// Compiles a graph pattern.
    pub(crate) fn pattern(&mut self, p: &GraphPattern) -> Result<Block> {
        match p {
            GraphPattern::Bgp { patterns } => self.bgp(patterns),
            GraphPattern::Join { left, right } => {
                let a = self.pattern(left)?;
                let b = self.pattern(right)?;
                self.join(a, b)
            }
            GraphPattern::Filter { expr, inner } => {
                let b = self.pattern(inner)?;
                let mut b = self.plain(b);
                let conds = self.filter_conditions(expr, &b.cols)?;
                b.wheres.extend(conds);
                Ok(b)
            }
            GraphPattern::Graph { name, inner } => {
                let saved = self.scope;
                self.scope = match name {
                    NamedNodePattern::NamedNode(n) => {
                        let id = self.constant_id(&n.clone().into())?;
                        GraphScope::Fixed(id)
                    }
                    NamedNodePattern::Variable(v) => GraphScope::Var(self.var(v)),
                };
                let accesses = self.aliases;
                let r = self.pattern(inner);
                let scope = self.scope;
                self.scope = saved;
                let mut b = r?;
                match scope {
                    GraphScope::Var(v) => {
                        if !b.cols.contains_key(&v) {
                            // No quad access binds the graph: enumerate named graphs.
                            let g = self.graph_list_block(v);
                            b = self.join(b, g)?;
                        }
                    }
                    GraphScope::Fixed(id) => {
                        if self.aliases == accesses || !has_quad_access(p) {
                            // GRAPH <g> {} only matches if the graph exists.
                            b = self.plain(b);
                            b.wheres
                                .push(format!("EXISTS (SELECT 1 FROM graphs WHERE id = {id})"));
                        }
                    }
                    GraphScope::Default => {}
                }
                Ok(b)
            }
            GraphPattern::Extend {
                inner,
                variable,
                expression,
            } => {
                let b = self.pattern(inner)?;
                let mut b = self.plain(b);
                let v = self.expr_term(expression, &b.cols)?;
                let idx = self.var(variable);
                let (nullable, computed) = match expression {
                    Expression::Variable(x) => {
                        let xi = self.var(x);
                        (b.cols.get(&xi).is_none_or(|b| b.nullable), false)
                    }
                    Expression::NamedNode(_) | Expression::Literal(_) => (false, true),
                    _ => (true, true),
                };
                let col = match &v.id {
                    Some(id) if v.decodable => Col::Id(id.clone()),
                    _ => Col::Val(Box::new(v)),
                };
                b.cols.insert(
                    idx,
                    Binding {
                        col,
                        nullable,
                        computed,
                        correlated: false,
                    },
                );
                Ok(b)
            }
            GraphPattern::Values {
                variables,
                bindings,
            } => self.values(variables, bindings),
            GraphPattern::OrderBy { inner, expression } => {
                let mut b = self.pattern(inner)?;
                if b.stage > Stage::Grouped {
                    b = self.seal(b);
                }
                for oe in expression {
                    let (e, desc) = match oe {
                        OrderExpression::Asc(e) => (e, false),
                        OrderExpression::Desc(e) => (e, true),
                    };
                    let v = self.expr_term(e, &b.cols)?;
                    for k in order_keys(&v) {
                        b.order_by.push(if desc { format!("{k} DESC") } else { k });
                    }
                }
                b.stage = Stage::Ordered;
                Ok(b)
            }
            GraphPattern::Project { inner, variables } => {
                // Inside `GRAPH ?g`, a subquery's own ?g (if not projected) is a different
                // variable: bind the graph to a hidden variable and expose it as ?g after
                // projection.
                let graph_var = match self.scope {
                    GraphScope::Var(g) => Some(g),
                    _ => None,
                };
                let hidden = graph_var.map(|_| self.fresh_var("g"));
                let saved = self.scope;
                if let Some(h) = hidden {
                    self.scope = GraphScope::Var(h);
                }
                let r = self.pattern(inner);
                self.scope = saved;
                let mut b = r?;
                if b.stage >= Stage::Distinct {
                    b = self.seal(b);
                }
                let hidden_binding = hidden.and_then(|h| b.cols.get(&h).cloned());
                let mut cols = BTreeMap::new();
                for v in variables {
                    let idx = self.var(v);
                    let bind = b.cols.remove(&idx).unwrap_or(Binding {
                        col: Col::Id("NULL".into()),
                        nullable: true,
                        computed: true,
                        correlated: false,
                    });
                    cols.insert(idx, bind);
                }
                b.cols = cols;
                if let (Some(g), Some(h)) = (graph_var, hidden_binding) {
                    match b.cols.get(&g).and_then(|x| x.col.key().map(str::to_string)) {
                        Some(inner_g) => {
                            let hk = h.col.key().unwrap_or("NULL").to_string();
                            b.wheres.push(format!("{inner_g} = {hk}"));
                        }
                        None => {
                            b.cols.insert(g, h);
                        }
                    }
                }
                Ok(b)
            }
            GraphPattern::Distinct { inner } => {
                let mut b = self.pattern(inner)?;
                if b.stage >= Stage::Distinct {
                    b = self.seal(b);
                }
                b.distinct = true;
                b.stage = Stage::Distinct;
                Ok(b)
            }
            GraphPattern::Reduced { inner } => self.pattern(inner),
            GraphPattern::Slice {
                inner,
                start,
                length,
            } => {
                let mut b = self.pattern(inner)?;
                if b.stage == Stage::Sliced {
                    b = self.seal(b);
                }
                b.offset = *start;
                b.limit = *length;
                b.stage = Stage::Sliced;
                Ok(b)
            }
            other => self.pattern_ext(other),
        }
    }

    /// Operators added after M1 (see `ops.rs`); unsupported ones go to the fallback.
    fn pattern_ext(&mut self, p: &GraphPattern) -> Result<Block> {
        Err(Error::unsupported(format!(
            "graph pattern {}",
            match p {
                GraphPattern::Path { .. } => "property path",
                GraphPattern::LeftJoin { .. } => "OPTIONAL",
                GraphPattern::Union { .. } => "UNION",
                GraphPattern::Minus { .. } => "MINUS",
                GraphPattern::Group { .. } => "GROUP BY",
                GraphPattern::Service { .. } => "SERVICE",
                _ => "operator",
            }
        )))
    }

    fn graph_list_block(&mut self, v: usize) -> Block {
        let a = self.alias("gr");
        let mut b = Block::default();
        b.from.push(FromItem {
            join: Join::First,
            item: format!("graphs {a}"),
        });
        if let Some(named) = &self.dataset.named {
            b.wheres.push(in_list(&format!("{a}.id"), named));
        }
        b.cols.insert(v, Binding::id(format!("{a}.id")));
        b
    }

    fn pos(&mut self, t: &TermPattern, bnodes: &mut HashMap<String, usize>) -> Result<Pos> {
        Ok(match t {
            TermPattern::NamedNode(n) => Pos::Const(self.constant_id(&n.clone().into())?),
            TermPattern::Literal(l) => Pos::Const(self.constant_id(&l.clone().into())?),
            TermPattern::Variable(v) => Pos::Var(self.var(v)),
            TermPattern::BlankNode(b) => {
                let label = b.as_str().to_string();
                if let Some(v) = bnodes.get(&label) {
                    Pos::Var(*v)
                } else {
                    let v = self.fresh_var("b");
                    bnodes.insert(label, v);
                    Pos::Var(v)
                }
            }
            TermPattern::Triple(_) => {
                return Err(Error::unsupported("triple term patterns"));
            }
        })
    }

    fn pos_nn(&mut self, p: &NamedNodePattern) -> Result<Pos> {
        Ok(match p {
            NamedNodePattern::NamedNode(n) => Pos::Const(self.constant_id(&n.clone().into())?),
            NamedNodePattern::Variable(v) => Pos::Var(self.var(v)),
        })
    }

    /// Binds a SQL column to a pattern position.
    pub(crate) fn bind_pos(&mut self, b: &mut Block, colsql: &str, pos: Pos) -> Result<()> {
        match pos {
            Pos::Const(id) => b.wheres.push(format!("{colsql} = {id}")),
            Pos::Var(v) => {
                if let Some(existing) = b.cols.get(&v) {
                    match existing.col.key() {
                        Some(x) if !existing.nullable => b.wheres.push(format!("{colsql} = {x}")),
                        Some(x) => b.wheres.push(format!("({x} IS NULL OR {colsql} = {x})")),
                        None => return Err(Error::unsupported("join on a computed value")),
                    }
                    return Ok(());
                }
                let mut correlated = false;
                if let Some(outer) = self.outer_binding(v) {
                    match outer.col.key() {
                        Some(o) => {
                            b.wheres.push(if outer.nullable {
                                format!("({o} IS NULL OR {colsql} = {o})")
                            } else {
                                format!("{colsql} = {o}")
                            });
                            correlated = true;
                        }
                        None => return Err(Error::unsupported("EXISTS over computed values")),
                    }
                }
                b.cols.insert(
                    v,
                    Binding {
                        correlated,
                        ..Binding::id(colsql)
                    },
                );
            }
        }
        Ok(())
    }

    /// Adds graph conditions for a quad alias according to the current scope and dataset.
    ///
    /// When another position is bound (`selective`), the graph equality is written as
    /// `+q.g = …`: the unary plus keeps SQLite from choosing the `gspo` index on a `g`
    /// equality that nearly every quad satisfies, so it uses the s/p/o permutation instead.
    pub(crate) fn graph_pos(&mut self, b: &mut Block, q: &str, selective: bool) -> Result<()> {
        let plus = if selective { "+" } else { "" };
        match self.scope {
            GraphScope::Default => match self.dataset.default.clone() {
                DefaultGraph::Zero => b.wheres.push(format!("{plus}{q}.g = {DEFAULT_GRAPH_ID}")),
                DefaultGraph::List(l) if l.is_empty() => b.wheres.push("0".into()),
                DefaultGraph::List(l) if l.len() == 1 => {
                    b.wheres.push(format!("{plus}{q}.g = {}", l[0]))
                }
                DefaultGraph::List(l) => {
                    b.wheres.push(in_list(&format!("{q}.g"), &l));
                    let d = self.alias("d");
                    b.wheres.push(format!(
                        "NOT EXISTS (SELECT 1 FROM quads {d} WHERE {d}.s = {q}.s AND {d}.p = {q}.p AND {d}.o = {q}.o AND {d}.g < {q}.g AND {})",
                        in_list(&format!("{d}.g"), &l)
                    ));
                }
                DefaultGraph::Union => {
                    let d = self.alias("d");
                    b.wheres.push(format!(
                        "NOT EXISTS (SELECT 1 FROM quads {d} WHERE {d}.s = {q}.s AND {d}.p = {q}.p AND {d}.o = {q}.o AND {d}.g < {q}.g)"
                    ));
                }
            },
            GraphScope::Fixed(id) => {
                if self
                    .dataset
                    .named
                    .as_ref()
                    .is_some_and(|n| !n.contains(&id))
                {
                    b.wheres.push("0".into());
                }
                b.wheres.push(format!("{plus}{q}.g = {id}"));
            }
            GraphScope::Var(v) => {
                b.wheres.push(format!("{q}.g <> {DEFAULT_GRAPH_ID}"));
                if let Some(named) = self.dataset.named.clone() {
                    b.wheres.push(in_list(&format!("{q}.g"), &named));
                }
                self.bind_pos(b, &format!("{q}.g"), Pos::Var(v))?;
            }
        }
        Ok(())
    }

    /// Adds a `quads` alias bound to the three positions (in the current graph scope).
    pub(crate) fn quad_access(
        &mut self,
        b: &mut Block,
        join: Join,
        s: Pos,
        p: Pos,
        o: Pos,
    ) -> Result<String> {
        let q = self.alias("q");
        let bound = |pos: Pos, b: &Block, me: &Self| match pos {
            Pos::Const(_) => true,
            Pos::Var(v) => b.cols.contains_key(&v) || me.outer_binding(v).is_some(),
        };
        let selective = bound(s, b, self) || bound(p, b, self) || bound(o, b, self);
        b.from.push(FromItem {
            join: if b.from.is_empty() { Join::First } else { join },
            item: format!("quads {q}"),
        });
        self.bind_pos(b, &format!("{q}.s"), s)?;
        self.bind_pos(b, &format!("{q}.p"), p)?;
        self.bind_pos(b, &format!("{q}.o"), o)?;
        self.graph_pos(b, &q, selective)?;
        Ok(q)
    }

    fn bgp(&mut self, patterns: &[TriplePattern]) -> Result<Block> {
        if patterns.is_empty() {
            return Ok(Block::default());
        }
        let mut bnodes = HashMap::new();
        let mut enc = Vec::with_capacity(patterns.len());
        for tp in patterns {
            enc.push([
                self.pos(&tp.subject, &mut bnodes)?,
                self.pos_nn(&tp.predicate)?,
                self.pos(&tp.object, &mut bnodes)?,
            ]);
        }
        let pre: HashSet<usize> = self.outer.iter().flat_map(|m| m.keys().copied()).collect();
        let ord: Vec<usize> = if self.options.sqlite_planner {
            (0..enc.len()).collect()
        } else {
            plan::order(&enc, &pre, self.stats)
        };
        let join = if self.options.sqlite_planner {
            Join::Inner
        } else {
            Join::Cross
        };
        let mut b = Block::default();
        for i in ord {
            let [s, p, o] = enc[i];
            self.quad_access(&mut b, join.clone(), s, p, o)?;
        }
        Ok(b)
    }

    /// Joins two blocks (SPARQL Join).
    pub(crate) fn join(&mut self, a: Block, b: Block) -> Result<Block> {
        let mut a = self.plain(a);
        let b = self.plain(b);
        if a.is_unit() {
            return Ok(b);
        }
        if b.is_unit() {
            return Ok(a);
        }
        for (idx, bb) in b.cols {
            match a.cols.get(&idx).cloned() {
                None => {
                    a.cols.insert(idx, bb);
                }
                Some(ab) => {
                    let (Some(x), Some(y)) = (ab.col.key(), bb.col.key()) else {
                        return Err(Error::unsupported("join on a computed value"));
                    };
                    let (x, y) = (x.to_string(), y.to_string());
                    if !ab.nullable && !bb.nullable {
                        a.wheres.push(format!("{x} = {y}"));
                        continue;
                    }
                    if matches!((&ab.col, &bb.col), (Col::Id(_), Col::Id(_))) {
                        a.wheres
                            .push(format!("({x} = {y} OR {x} IS NULL OR {y} IS NULL)"));
                        a.cols.insert(
                            idx,
                            Binding {
                                col: Col::Id(format!("COALESCE({x}, {y})")),
                                nullable: ab.nullable && bb.nullable,
                                computed: true,
                                correlated: ab.correlated || bb.correlated,
                            },
                        );
                    } else {
                        return Err(Error::unsupported("optional join on a computed value"));
                    }
                }
            }
        }
        for mut item in b.from {
            if a.from.is_empty() {
                item.join = Join::First;
            } else if item.join == Join::First {
                item.join = Join::Inner;
            }
            a.from.push(item);
        }
        a.wheres.extend(b.wheres);
        Ok(a)
    }

    fn values(
        &mut self,
        variables: &[Variable],
        bindings: &[Vec<Option<GroundTerm>>],
    ) -> Result<Block> {
        let mut b = Block::default();
        if variables.is_empty() {
            match bindings.len() {
                0 => b.wheres.push("0".into()),
                1 => {}
                n => b.from.push(FromItem {
                    join: Join::First,
                    item: format!(
                        "({}) AS {}",
                        vec!["SELECT 1"; n].join(" UNION ALL "),
                        self.alias("u")
                    ),
                }),
            }
            return Ok(b);
        }
        let idxs: Vec<usize> = variables.iter().map(|v| self.var(v)).collect();
        if bindings.is_empty() {
            b.wheres.push("0".into());
            for i in idxs {
                b.cols.insert(
                    i,
                    Binding {
                        nullable: true,
                        computed: true,
                        ..Binding::id("NULL")
                    },
                );
            }
            return Ok(b);
        }
        // Constants are not necessarily stored: each variable carries its full value (all
        // `V` fields) so no dictionary lookup is needed; the id remains the join key.
        let mut rows = Vec::new();
        let mut nullable = vec![false; idxs.len()];
        for row in bindings {
            let mut vals = Vec::new();
            for (j, t) in row.iter().enumerate() {
                let fields = match t {
                    None => {
                        nullable[j] = true;
                        val_fields(&V::null())
                    }
                    Some(t) => {
                        let term = ground_to_term(t);
                        let id = self.constant_id(&term)?;
                        val_fields(&V::from_term(&term, id)?)
                    }
                };
                vals.extend(fields);
            }
            rows.push(format!("({})", vals.join(",")));
        }
        let a = self.alias("vals");
        b.from.push(FromItem {
            join: Join::First,
            item: format!("(VALUES {}) AS {a}", rows.join(",")),
        });
        let n = VAL_FIELDS.len();
        for (j, i) in idxs.into_iter().enumerate() {
            let c = |k: usize| format!("{a}.column{}", j * n + k + 1);
            let v = V {
                id: Some(c(0)),
                kind: c(1),
                lex: c(2),
                dt: c(3),
                lang: c(4),
                num: c(5),
                nt: c(6),
                ts: c(7),
                boolv: c(8),
                stat: expr::Stat::Any,
                computed_num: false,
                decodable: false,
            };
            b.cols.insert(
                i,
                Binding {
                    col: Col::Val(Box::new(v)),
                    nullable: nullable[j],
                    computed: false,
                    correlated: false,
                },
            );
        }
        Ok(b)
    }

    /// Compiles a FILTER into WHERE conditions, keeping sargable equalities index-friendly.
    pub(crate) fn filter_conditions(
        &mut self,
        e: &Expression,
        cols: &BTreeMap<usize, Binding>,
    ) -> Result<Vec<String>> {
        if let Expression::And(a, b) = e {
            let mut out = self.filter_conditions(a, cols)?;
            out.extend(self.filter_conditions(b, cols)?);
            return Ok(out);
        }
        if let Expression::Equal(a, b) | Expression::SameTerm(a, b) = e {
            let pair = match (a.as_ref(), b.as_ref()) {
                (Expression::Variable(v), c) | (c, Expression::Variable(v)) => Some((v, c)),
                _ => None,
            };
            if let Some((v, c)) = pair {
                let t: Option<Term> = match c {
                    Expression::NamedNode(n) => Some(n.clone().into()),
                    Expression::Literal(l)
                        if matches!(e, Expression::SameTerm(..))
                            || l.language().is_some()
                            || l.datatype() == oxrdf::vocab::xsd::STRING =>
                    {
                        Some(l.clone().into())
                    }
                    _ => None,
                };
                if let Some(t) = t {
                    let idx = self.var(v);
                    let binding = cols.get(&idx).or_else(|| self.outer_binding(idx)).cloned();
                    if let Some(Binding {
                        col: Col::Id(x), ..
                    }) = binding
                    {
                        // In a FILTER an error and `false` both reject, so term equality is exact.
                        let id = self.constant_id(&t)?;
                        return Ok(vec![format!("{x} = {id}")]);
                    }
                }
            }
        }
        Ok(vec![self.expr_bool(e, cols)?])
    }
}

fn has_quad_access(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::Bgp { patterns } => !patterns.is_empty(),
        GraphPattern::Path { .. } => true,
        GraphPattern::Graph { .. } | GraphPattern::Values { .. } => false,
        GraphPattern::Join { left, right }
        | GraphPattern::LeftJoin { left, right, .. }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => has_quad_access(left) || has_quad_access(right),
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Group { inner, .. } => has_quad_access(inner),
        _ => true,
    }
}

pub(crate) fn in_list(col: &str, ids: &[i64]) -> String {
    if ids.is_empty() {
        return "0".into();
    }
    format!(
        "{col} IN ({})",
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

pub(crate) fn ground_to_term(t: &GroundTerm) -> Term {
    match t {
        GroundTerm::NamedNode(n) => n.clone().into(),
        GroundTerm::Literal(l) => l.clone().into(),
        GroundTerm::Triple(t) => oxrdf::Triple::new(
            t.subject.clone(),
            t.predicate.clone(),
            ground_to_term(&t.object),
        )
        .into(),
    }
}

/// SQL ORDER BY keys implementing Oxigraph's ordering: unbound < blank nodes < IRIs <
/// literals < triple terms; numeric literals first by value, other literals by
/// (lexical form, datatype, language).
pub(crate) fn order_keys(v: &V) -> Vec<String> {
    use expr::{Stat, K_BNODE, K_IRI, K_TRIPLE};
    match v.stat {
        Stat::Numeric => vec![format!("({}) IS NOT NULL", v.kind), v.num.clone()],
        Stat::String => vec![format!("({}) IS NOT NULL", v.kind), v.lex.clone()],
        _ => vec![
            format!(
                "CASE WHEN ({k}) IS NULL THEN 0 WHEN ({k}) = {K_BNODE} THEN 1 WHEN ({k}) = {K_IRI} THEN 2 WHEN ({k}) = {K_TRIPLE} THEN 4 ELSE 3 END",
                k = v.kind
            ),
            format!("({}) IS NULL", v.num),
            v.num.clone(),
            v.lex.clone(),
            v.dt.clone(),
            v.lang.clone(),
        ],
    }
}
