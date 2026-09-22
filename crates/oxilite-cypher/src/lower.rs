//! Lowering of Cypher patterns, expressions and projections to SPARQL algebra.
//!
//! The produced `spargebra` algebra runs through oxilite's SPARQL-to-SQL compiler, planner,
//! reasoning rewrites and fallback unchanged. Property-graph specifics are expressed with
//! plain SPARQL 1.2:
//!
//! - a relationship is a triple; its identity and properties live on an optional reifier
//!   (`?r rdf:reifies <<( ?a :T ?b )>>`);
//! - variable-length patterns expand into one `UNION` branch per length (and direction),
//!   with pairwise inequality filters giving trail semantics;
//! - variables that may be null (from `OPTIONAL MATCH`) are renamed and tied back with an
//!   equality filter, so SPARQL's "unbound joins with anything" never leaks into Cypher.
//!
// @lat: [[architecture#Property graph frontend#Planning and lowering]]

use crate::ast::*;
use crate::error::{CypherError, Result};
use crate::value::{Params, Value, REIFIES};
use crate::vocab::Vocabulary;
use crate::CypherOptions;
use oxrdf::vocab::{rdf, xsd};
use oxrdf::{Literal, NamedNode, Variable};
use spargebra::algebra::{
    AggregateExpression, AggregateFunction, Expression, Function, GraphPattern, OrderExpression,
    PropertyPathExpression,
};
use spargebra::term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The SPARQL variables of a relationship.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RelVars {
    pub s: Variable,
    pub p: Variable,
    pub o: Variable,
    pub rid: Variable,
}

impl RelVars {
    fn vars(&self) -> [Variable; 4] {
        [
            self.s.clone(),
            self.p.clone(),
            self.o.clone(),
            self.rid.clone(),
        ]
    }
}

/// How a Cypher variable is represented by SPARQL variables.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Bind {
    Node {
        var: Variable,
        nullable: bool,
    },
    Rel {
        r: RelVars,
        nullable: bool,
    },
    /// A path (fixed or variable length): node and relationship slots plus the length.
    Path {
        len: Variable,
        nodes: Vec<Variable>,
        rels: Vec<RelVars>,
        nullable: bool,
    },
    /// The list bound by a variable-length relationship `[r*1..3]`.
    RelList {
        len: Variable,
        rels: Vec<RelVars>,
        nullable: bool,
    },
    Value {
        var: Variable,
        nullable: bool,
        name: bool,
    },
    /// A map unwound from a parameter: one SPARQL variable per key.
    Map {
        fields: BTreeMap<String, Variable>,
        nullable: bool,
    },
}

impl Bind {
    pub fn vars(&self) -> Vec<Variable> {
        match self {
            Self::Node { var, .. } | Self::Value { var, .. } => vec![var.clone()],
            Self::Rel { r, .. } => r.vars().to_vec(),
            Self::Path {
                len, nodes, rels, ..
            } => std::iter::once(len.clone())
                .chain(nodes.iter().cloned())
                .chain(rels.iter().flat_map(RelVars::vars))
                .collect(),
            Self::RelList { len, rels, .. } => std::iter::once(len.clone())
                .chain(rels.iter().flat_map(RelVars::vars))
                .collect(),
            Self::Map { fields, .. } => fields.values().cloned().collect(),
        }
    }

    fn nullable(&self) -> bool {
        match self {
            Self::Node { nullable, .. }
            | Self::Rel { nullable, .. }
            | Self::Path { nullable, .. }
            | Self::RelList { nullable, .. }
            | Self::Value { nullable, .. }
            | Self::Map { nullable, .. } => *nullable,
        }
    }

    fn with_nullable(mut self, n: bool) -> Self {
        match &mut self {
            Self::Node { nullable, .. }
            | Self::Rel { nullable, .. }
            | Self::Path { nullable, .. }
            | Self::RelList { nullable, .. }
            | Self::Value { nullable, .. }
            | Self::Map { nullable, .. } => *nullable = *nullable || n,
        }
        self
    }
}

pub(crate) type Scope = BTreeMap<String, Bind>;

/// A `shortestPath` / `allShortestPaths` pattern, computed by breadth-first search in Rust
/// between the ends the SQL part binds.
#[derive(Debug, Clone)]
pub(crate) struct ShortestSpec {
    pub path: String,
    pub rel: Option<String>,
    pub from: Variable,
    pub to: Variable,
    pub types: Vec<NamedNode>,
    pub dir: Direction,
    pub min: usize,
    pub max: usize,
    pub all: bool,
}

/// The SPARQL pattern built so far and the Cypher variables it binds.
#[derive(Debug, Clone)]
pub(crate) struct Stage {
    pub pattern: GraphPattern,
    pub scope: Scope,
    pub(crate) props: HashMap<(Variable, String), Variable>,
    /// Labels a node variable is known to have (from its pattern), for the SHACL schema.
    pub(crate) labels: HashMap<Variable, Vec<NamedNode>>,
}

impl Stage {
    pub fn new() -> Self {
        Self {
            pattern: unit(),
            scope: Scope::new(),
            props: HashMap::new(),
            labels: HashMap::new(),
        }
    }

    /// A stage for the right side of an optional pattern.
    pub fn child(&self) -> Self {
        Self {
            scope: self.scope.clone(),
            labels: self.labels.clone(),
            ..Self::new()
        }
    }
}

pub(crate) fn unit() -> GraphPattern {
    GraphPattern::Bgp {
        patterns: Vec::new(),
    }
}

fn is_unit(p: &GraphPattern) -> bool {
    matches!(p, GraphPattern::Bgp { patterns } if patterns.is_empty())
}

pub(crate) fn join(a: GraphPattern, b: GraphPattern) -> GraphPattern {
    if is_unit(&a) {
        return b;
    }
    if is_unit(&b) {
        return a;
    }
    // Merge adjacent BGPs so the planner orders all their patterns together.
    match (a, b) {
        (GraphPattern::Bgp { patterns: mut x }, GraphPattern::Bgp { patterns: y }) => {
            x.extend(y);
            GraphPattern::Bgp { patterns: x }
        }
        (a, b) => GraphPattern::Join {
            left: Box::new(a),
            right: Box::new(b),
        },
    }
}

fn filter(inner: GraphPattern, expr: Expression) -> GraphPattern {
    GraphPattern::Filter {
        expr,
        inner: Box::new(inner),
    }
}

fn and_all(mut exprs: Vec<Expression>) -> Option<Expression> {
    let first = exprs.pop()?;
    Some(
        exprs
            .into_iter()
            .rev()
            .fold(first, |acc, e| Expression::And(Box::new(e), Box::new(acc))),
    )
}

fn var_expr(v: &Variable) -> Expression {
    Expression::Variable(v.clone())
}

fn tp(s: TermPattern, p: NamedNodePattern, o: TermPattern) -> TriplePattern {
    TriplePattern {
        subject: s,
        predicate: p,
        object: o,
    }
}

fn tv(v: &Variable) -> TermPattern {
    TermPattern::Variable(v.clone())
}

fn rdf_type() -> NamedNode {
    rdf::TYPE.into_owned()
}

fn reifies() -> NamedNode {
    NamedNode::new_unchecked(REIFIES)
}

fn fcall(f: Function, args: Vec<Expression>) -> Expression {
    Expression::FunctionCall(f, args)
}

fn xsd_fn(dt: oxrdf::NamedNodeRef<'_>, arg: Expression) -> Expression {
    fcall(Function::Custom(dt.into_owned()), vec![arg])
}

fn bool_lit(b: bool) -> Expression {
    Expression::Literal(Literal::from(b))
}

/// A predicate position: a constant type or a variable restricted to relationship triples.
#[derive(Debug, Clone)]
enum Pred {
    Const(NamedNode),
    Var(Variable),
}

impl Pred {
    fn pattern(&self) -> NamedNodePattern {
        match self {
            Self::Const(n) => NamedNodePattern::NamedNode(n.clone()),
            Self::Var(v) => NamedNodePattern::Variable(v.clone()),
        }
    }

    fn expr(&self) -> Expression {
        match self {
            Self::Const(n) => Expression::NamedNode(n.clone()),
            Self::Var(v) => var_expr(v),
        }
    }
}

/// One relationship occurrence inside a branch, for uniqueness filters.
#[derive(Debug, Clone)]
struct Hop {
    s: Variable,
    p: Pred,
    o: Variable,
    rid: Option<Variable>,
}

/// One alternative of a `MATCH` (different lengths or directions of its relationships).
#[derive(Debug, Clone, Default)]
struct Branch {
    triples: Vec<TriplePattern>,
    joins: Vec<GraphPattern>,
    optionals: Vec<GraphPattern>,
    extends: Vec<(Variable, Expression)>,
    filters: Vec<Expression>,
    hops: Vec<Hop>,
    /// Node variables that must be bound (enumerated) if no triple binds them.
    nodes: Vec<Variable>,
    /// Variables bound by `joins`.
    bound: Vec<Variable>,
}

impl Branch {
    /// The branch's own pattern, plus the bindings and filters that reference variables it
    /// does not bind: SPARQL evaluates a sub-pattern on its own, so those are applied after
    /// the branch is joined with the rest of the query.
    /// Variables whose bindings this branch must apply after the join (they read variables
    /// the branch does not bind).
    fn lifted_vars(&self) -> BTreeSet<Variable> {
        let mut bound: BTreeSet<Variable> = self.bound.iter().cloned().collect();
        for t in &self.triples {
            triple_vars(t, &mut bound);
        }
        for j in &self.joins {
            gp_vars(j, &mut bound);
        }
        bound.extend(self.nodes.iter().cloned());
        for o in &self.optionals {
            gp_vars(o, &mut bound);
        }
        let mut out = BTreeSet::new();
        for (v, e) in &self.extends {
            let mut refs = BTreeSet::new();
            ex_vars(e, &mut refs);
            if refs.is_subset(&bound) && refs.is_disjoint(&out) {
                bound.insert(v.clone());
            } else {
                out.insert(v.clone());
            }
        }
        out
    }

    fn into_parts(
        self,
        lw: &Lowerer<'_>,
    ) -> (GraphPattern, Vec<(Variable, Expression)>, Vec<Expression>) {
        self.into_parts_with(lw, &BTreeSet::new())
    }

    fn into_parts_with(
        self,
        lw: &Lowerer<'_>,
        force: &BTreeSet<Variable>,
    ) -> (GraphPattern, Vec<(Variable, Expression)>, Vec<Expression>) {
        let mut bound: BTreeSet<Variable> = self.bound.iter().cloned().collect();
        for t in &self.triples {
            triple_vars(t, &mut bound);
        }
        for j in &self.joins {
            gp_vars(j, &mut bound);
        }
        let mut p = GraphPattern::Bgp {
            patterns: self.triples,
        };
        for j in self.joins {
            p = join(p, j);
        }
        for n in self.nodes {
            if !bound.contains(&n) && !self.extends.iter().any(|(v, _)| *v == n) {
                p = join(p, lw.node_enumeration(&n));
                bound.insert(n);
            }
        }
        for o in self.optionals {
            gp_vars(&o, &mut bound);
            p = GraphPattern::LeftJoin {
                left: Box::new(p),
                right: Box::new(o),
                expression: None,
            };
        }
        let mut outer_ext = Vec::new();
        let mut lifted: BTreeSet<Variable> = BTreeSet::new();
        for (v, e) in self.extends {
            let mut refs = BTreeSet::new();
            ex_vars(&e, &mut refs);
            if refs.is_subset(&bound) && !force.contains(&v) && refs.is_disjoint(&lifted) {
                bound.insert(v.clone());
                p = GraphPattern::Extend {
                    inner: Box::new(p),
                    variable: v,
                    expression: e,
                };
            } else {
                lifted.insert(v.clone());
                outer_ext.push((v, e));
            }
        }
        // Trail semantics: no relationship is used twice.
        let mut filters = self.filters;
        for (i, a) in self.hops.iter().enumerate() {
            for b in &self.hops[i + 1..] {
                if let (Pred::Const(x), Pred::Const(y)) = (&a.p, &b.p) {
                    if x != y {
                        continue;
                    }
                }
                filters.push(Expression::Not(Box::new(same_edge(a, b))));
            }
        }
        let mut outer_filters = Vec::new();
        let outer_bound: BTreeSet<Variable> = outer_ext.iter().map(|(v, _)| v.clone()).collect();
        for f in filters {
            let mut refs = BTreeSet::new();
            ex_vars(&f, &mut refs);
            if refs.is_subset(&bound) && refs.is_disjoint(&outer_bound) {
                // Flat filters keep the SQL shallow.
                p = filter(p, f);
            } else {
                outer_filters.push(f);
            }
        }
        (p, outer_ext, outer_filters)
    }
}

/// A lowered `MATCH`: its pattern, and what must be applied after joining it with the
/// patterns of earlier clauses.
#[derive(Debug, Clone)]
pub(crate) struct Matched {
    pub inner: GraphPattern,
    pub extends: Vec<(Variable, Expression)>,
    pub filters: Vec<Expression>,
}

impl Matched {
    /// `MATCH`: joined with the earlier patterns.
    pub fn join_into(self, outer: GraphPattern) -> GraphPattern {
        let mut p = join(outer, self.inner);
        for (v, e) in self.extends {
            p = GraphPattern::Extend {
                inner: Box::new(p),
                variable: v,
                expression: e,
            };
        }
        for f in self.filters {
            p = filter(p, f);
        }
        p
    }

    /// `OPTIONAL MATCH`: left-joined; the lifted filters (and `extra`, the WHERE clause)
    /// become the join condition.
    pub fn left_join_into(self, outer: GraphPattern, extra: Option<Expression>) -> GraphPattern {
        let mut conds = self.filters;
        conds.extend(extra);
        let mut p = GraphPattern::LeftJoin {
            left: Box::new(outer),
            right: Box::new(self.inner),
            expression: and_all(conds),
        };
        for (v, e) in self.extends {
            p = GraphPattern::Extend {
                inner: Box::new(p),
                variable: v,
                expression: e,
            };
        }
        p
    }
}

fn triple_vars(t: &TriplePattern, out: &mut BTreeSet<Variable>) {
    for x in [&t.subject, &t.object] {
        match x {
            TermPattern::Variable(v) => {
                out.insert(v.clone());
            }
            TermPattern::Triple(t) => triple_vars(t, out),
            _ => {}
        }
    }
    if let NamedNodePattern::Variable(v) = &t.predicate {
        out.insert(v.clone());
    }
}

/// Variables a pattern may bind.
fn gp_vars(p: &GraphPattern, out: &mut BTreeSet<Variable>) {
    match p {
        GraphPattern::Bgp { patterns } => patterns.iter().for_each(|t| triple_vars(t, out)),
        GraphPattern::Path {
            subject, object, ..
        } => {
            for x in [subject, object] {
                if let TermPattern::Variable(v) = x {
                    out.insert(v.clone());
                }
            }
        }
        GraphPattern::Join { left, right }
        | GraphPattern::LeftJoin { left, right, .. }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => {
            gp_vars(left, out);
            gp_vars(right, out);
        }
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Graph { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Service { inner, .. } => gp_vars(inner, out),
        GraphPattern::Extend {
            inner, variable, ..
        } => {
            gp_vars(inner, out);
            out.insert(variable.clone());
        }
        GraphPattern::Values { variables, .. } => out.extend(variables.iter().cloned()),
        GraphPattern::Project { variables, .. } => out.extend(variables.iter().cloned()),
        GraphPattern::Group {
            variables,
            aggregates,
            ..
        } => {
            out.extend(variables.iter().cloned());
            out.extend(aggregates.iter().map(|(v, _)| v.clone()));
        }
        #[allow(unreachable_patterns)]
        _ => {}
    }
}

/// Variables an expression reads (from the enclosing solution).
fn ex_vars(e: &Expression, out: &mut BTreeSet<Variable>) {
    match e {
        Expression::Variable(v) | Expression::Bound(v) => {
            out.insert(v.clone());
        }
        Expression::Or(a, b)
        | Expression::And(a, b)
        | Expression::Equal(a, b)
        | Expression::SameTerm(a, b)
        | Expression::Greater(a, b)
        | Expression::GreaterOrEqual(a, b)
        | Expression::Less(a, b)
        | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b)
        | Expression::Subtract(a, b)
        | Expression::Multiply(a, b)
        | Expression::Divide(a, b) => {
            ex_vars(a, out);
            ex_vars(b, out);
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => {
            ex_vars(a, out)
        }
        Expression::In(a, items) => {
            ex_vars(a, out);
            items.iter().for_each(|i| ex_vars(i, out));
        }
        Expression::If(a, b, c) => {
            ex_vars(a, out);
            ex_vars(b, out);
            ex_vars(c, out);
        }
        Expression::Coalesce(items) | Expression::FunctionCall(_, items) => {
            items.iter().for_each(|i| ex_vars(i, out))
        }
        Expression::Exists(p) => gp_vars(p, out),
        Expression::NamedNode(_) | Expression::Literal(_) => {}
    }
}

fn same_edge(a: &Hop, b: &Hop) -> Expression {
    let mut parts = vec![
        Expression::SameTerm(Box::new(var_expr(&a.s)), Box::new(var_expr(&b.s))),
        Expression::SameTerm(Box::new(var_expr(&a.o)), Box::new(var_expr(&b.o))),
    ];
    if !matches!((&a.p, &b.p), (Pred::Const(_), Pred::Const(_))) {
        parts.push(Expression::SameTerm(
            Box::new(a.p.expr()),
            Box::new(b.p.expr()),
        ));
    }
    if let (Some(x), Some(y)) = (&a.rid, &b.rid) {
        // Parallel relationships differ by reifier; unreified ones are the triple itself.
        parts.push(Expression::Coalesce(vec![
            Expression::SameTerm(Box::new(var_expr(x)), Box::new(var_expr(y))),
            Expression::And(
                Box::new(Expression::Not(Box::new(Expression::Bound(x.clone())))),
                Box::new(Expression::Not(Box::new(Expression::Bound(y.clone())))),
            ),
        ]));
    }
    and_all(parts).expect("non-empty")
}

pub(crate) struct Lowerer<'a> {
    pub vocab: &'a Vocabulary,
    pub params: &'a Params,
    pub opts: &'a CypherOptions,
    n: usize,
    pub notes: Vec<String>,
    /// Paths bound by `shortestPath` / `allShortestPaths` in the last lowered MATCH.
    pub shortest: Vec<ShortestSpec>,
    /// Lowering a pattern bound to a path variable (every hop must be materialized).
    in_path: bool,
    /// Value types of property variables, from the SHACL schema.
    pub var_types: BTreeMap<String, oxilite_core::ValueType>,
}

/// A pattern element after the first expansion step (for paths).
#[derive(Debug, Clone)]
struct Walk {
    nodes: Vec<Variable>,
    rels: Vec<RelVars>,
}

impl<'a> Lowerer<'a> {
    pub fn new(vocab: &'a Vocabulary, params: &'a Params, opts: &'a CypherOptions) -> Self {
        Self {
            vocab,
            params,
            opts,
            n: 0,
            notes: Vec::new(),
            shortest: Vec::new(),
            in_path: false,
            var_types: BTreeMap::new(),
        }
    }

    pub fn fresh(&mut self, hint: &str) -> Variable {
        self.n += 1;
        let clean: String = hint
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(12)
            .collect();
        Variable::new_unchecked(format!("c{}_{clean}", self.n))
    }

    fn fresh_rel(&mut self, hint: &str) -> RelVars {
        RelVars {
            s: self.fresh(&format!("{hint}s")),
            p: self.fresh(&format!("{hint}p")),
            o: self.fresh(&format!("{hint}o")),
            rid: self.fresh(&format!("{hint}r")),
        }
    }

    /// All nodes: subjects that are not reifiers, and objects of relationship triples.
    fn node_enumeration(&self, v: &Variable) -> GraphPattern {
        let (p, o, s) = (
            Variable::new_unchecked(format!("{}_ep", v.as_str())),
            Variable::new_unchecked(format!("{}_eo", v.as_str())),
            Variable::new_unchecked(format!("{}_es", v.as_str())),
        );
        let t = Variable::new_unchecked(format!("{}_et", v.as_str()));
        let out = GraphPattern::Bgp {
            patterns: vec![tp(tv(v), NamedNodePattern::Variable(p.clone()), tv(&o))],
        };
        let inc = filter(
            GraphPattern::Bgp {
                patterns: vec![tp(tv(&s), NamedNodePattern::Variable(p.clone()), tv(v))],
            },
            and_all(vec![
                rel_object_filter(v),
                Expression::Not(Box::new(Expression::In(
                    Box::new(var_expr(&p)),
                    vec![
                        Expression::NamedNode(rdf_type()),
                        Expression::NamedNode(reifies()),
                    ],
                ))),
            ])
            .expect("two"),
        );
        let union = GraphPattern::Union {
            left: Box::new(out),
            right: Box::new(inc),
        };
        let not_reifier =
            Expression::Not(Box::new(Expression::Exists(Box::new(GraphPattern::Bgp {
                patterns: vec![tp(tv(v), NamedNodePattern::NamedNode(reifies()), tv(&t))],
            }))));
        GraphPattern::Distinct {
            inner: Box::new(GraphPattern::Project {
                inner: Box::new(filter(union, not_reifier)),
                variables: vec![v.clone()],
            }),
        }
    }

    // ----- MATCH -----

    /// Lowers the patterns of a `MATCH` into a graph pattern; new variables are added to
    /// `scope` (nullable when `optional`).
    pub fn match_patterns(
        &mut self,
        parts: &[PatternPart],
        stage: &mut Stage,
        optional: bool,
        forbid_new_rel_reuse: bool,
    ) -> Result<Matched> {
        let _ = forbid_new_rel_reuse;
        let mut branches = vec![Branch::default()];
        let mut local: BTreeMap<String, Bind> = BTreeMap::new();
        self.shortest.clear();
        for part in parts {
            let mut next = Vec::new();
            if let Some(kind) = part.shortest {
                let Some(pv) = &part.var else {
                    return Err(CypherError::unsupported(
                        "shortestPath() without a path variable (write `p = shortestPath(…)`)",
                    ));
                };
                let [(rel, end)] = part.element.chain.as_slice() else {
                    return Err(CypherError::unsupported(
                        "shortestPath() over more than one relationship pattern",
                    ));
                };
                if rel.props.is_some() {
                    return Err(CypherError::unsupported(
                        "shortestPath() with relationship properties",
                    ));
                }
                let (min, max) = rel.length.unwrap_or((Some(1), Some(1)));
                let min = min.unwrap_or(1) as usize;
                if min > 1 {
                    return Err(CypherError::unsupported(
                        "shortestPath() with a minimum length above 1",
                    ));
                }
                let max = max.map_or(self.opts.shortest_path_cap, |m| m as usize);
                let mut ends = None;
                for br in &mut branches {
                    let from = self.node_var(&part.element.start, br, stage, &mut local)?;
                    let to = self.node_var(end, br, stage, &mut local)?;
                    if ends.as_ref().is_some_and(|(f, t)| *f != from || *t != to) {
                        return Err(CypherError::unsupported(
                            "shortestPath() between anonymous nodes of an alternative pattern",
                        ));
                    }
                    ends = Some((from, to));
                }
                let (from, to) = ends.expect("one branch at least");
                let mut uniq: Vec<NamedNode> = Vec::new();
                for t in &rel.types {
                    let iri = self.vocab.iri(t);
                    if !uniq.contains(&iri) {
                        uniq.push(iri);
                    }
                }
                for name in std::iter::once(pv).chain(rel.var.as_ref()) {
                    let placeholder = self.fresh(name);
                    local.insert(
                        name.clone(),
                        Bind::Value {
                            var: placeholder,
                            nullable: false,
                            name: false,
                        },
                    );
                }
                self.shortest.push(ShortestSpec {
                    path: pv.clone(),
                    rel: rel.var.clone(),
                    from,
                    to,
                    types: uniq,
                    dir: rel.dir,
                    min,
                    max,
                    all: kind == Shortest::All,
                });
                continue;
            }
            for br in branches {
                self.in_path = part.var.is_some();
                let walks = self.element(&part.element, br, stage, &mut local);
                self.in_path = false;
                let mut walks = walks?;
                if walks.len() * next.len().max(1) > self.opts.max_branches {
                    return Err(CypherError::unsupported(format!(
                        "pattern expands to more than {} alternatives; bound the variable-length relationships",
                        self.opts.max_branches
                    )));
                }
                if let Some(pv) = &part.var {
                    let max_rels = walks.iter().map(|(_, w)| w.rels.len()).max().unwrap_or(0);
                    let len = self.fresh(&format!("{pv}len"));
                    let nodes: Vec<Variable> = (0..=max_rels)
                        .map(|i| self.fresh(&format!("{pv}n{i}")))
                        .collect();
                    let rels: Vec<RelVars> = (0..max_rels).map(|_| self.fresh_rel(pv)).collect();
                    for (b, w) in &mut walks {
                        b.extends.push((
                            len.clone(),
                            Expression::Literal(Literal::from(w.rels.len() as i64)),
                        ));
                        for (i, n) in w.nodes.iter().enumerate() {
                            b.extends.push((nodes[i].clone(), var_expr(n)));
                        }
                        for (i, r) in w.rels.iter().enumerate() {
                            for (to, from) in rels[i].vars().iter().zip(r.vars()) {
                                b.extends.push((to.clone(), var_expr(&from)));
                            }
                        }
                    }
                    if stage.scope.contains_key(pv) || local.contains_key(pv) {
                        return Err(CypherError::semantic(format!(
                            "path variable `{pv}` is already defined"
                        )));
                    }
                    local.insert(
                        pv.clone(),
                        Bind::Path {
                            len,
                            nodes,
                            rels,
                            nullable: false,
                        },
                    );
                }
                next.extend(walks.into_iter().map(|(b, _)| b));
            }
            branches = next;
        }
        for (name, b) in local {
            stage.scope.insert(name, b.with_nullable(optional));
        }
        if branches.is_empty() {
            // An empty length interval (`*3..2`): nothing matches.
            return Ok(Matched {
                inner: GraphPattern::Values {
                    variables: Vec::new(),
                    bindings: Vec::new(),
                },
                extends: Vec::new(),
                filters: Vec::new(),
            });
        }
        // A variable bound after the join in one branch is bound after the join in all.
        let force: BTreeSet<Variable> = branches.iter().flat_map(Branch::lifted_vars).collect();
        let parts: Vec<_> = branches
            .into_iter()
            .map(|b| b.into_parts_with(self, &force))
            .collect();
        if parts.len() == 1 {
            let (inner, extends, filters) = parts.into_iter().next().expect("one");
            return Ok(Matched {
                inner,
                extends,
                filters,
            });
        }
        let lifted = parts.iter().any(|(_, e, f)| !e.is_empty() || !f.is_empty());
        let marker = self.fresh("branch");
        let mut inner: Option<GraphPattern> = None;
        let mut ext_vars: Vec<Variable> = Vec::new();
        let mut conds = Vec::new();
        let mut per_branch_ext: Vec<Vec<(Variable, Expression)>> = Vec::new();
        for (k, (p, e, f)) in parts.into_iter().enumerate() {
            let p = if lifted {
                GraphPattern::Extend {
                    inner: Box::new(p),
                    variable: marker.clone(),
                    expression: Expression::Literal(Literal::from(k as i64)),
                }
            } else {
                p
            };
            inner = Some(match inner {
                None => p,
                Some(prev) => GraphPattern::Union {
                    left: Box::new(prev),
                    right: Box::new(p),
                },
            });
            let is_k = Expression::Equal(
                Box::new(var_expr(&marker)),
                Box::new(Expression::Literal(Literal::from(k as i64))),
            );
            let mut c = vec![is_k];
            c.extend(f);
            conds.push(and_all(c).expect("non-empty"));
            for (v, _) in &e {
                if !ext_vars.contains(v) {
                    ext_vars.push(v.clone());
                }
            }
            per_branch_ext.push(e);
        }
        let mut extends = Vec::new();
        if lifted {
            // One IF chain per lifted binding, keyed by the branch marker; a variable that is
            // never bound makes the other branches unbound.
            let never = self.fresh("unbound");
            for v in ext_vars {
                let mut expr = var_expr(&never);
                for (k, e) in per_branch_ext.iter().enumerate().rev() {
                    if let Some((_, x)) = e.iter().find(|(w, _)| *w == v) {
                        expr = Expression::If(
                            Box::new(Expression::Equal(
                                Box::new(var_expr(&marker)),
                                Box::new(Expression::Literal(Literal::from(k as i64))),
                            )),
                            Box::new(x.clone()),
                            Box::new(expr),
                        );
                    }
                }
                extends.push((v, expr));
            }
        }
        let filters = if lifted {
            let mut it = conds.into_iter();
            let first = it.next().expect("branches");
            vec![it.fold(first, |a, b| Expression::Or(Box::new(a), Box::new(b)))]
        } else {
            Vec::new()
        };
        Ok(Matched {
            inner: inner.expect("branches"),
            extends,
            filters,
        })
    }

    /// The SPARQL variable standing for a node pattern in this MATCH.
    fn node_var(
        &mut self,
        np: &NodePattern,
        br: &mut Branch,
        stage: &mut Stage,
        local: &mut BTreeMap<String, Bind>,
    ) -> Result<Variable> {
        let v = match &np.var {
            Some(name) => match local.get(name).or_else(|| stage.scope.get(name)).cloned() {
                Some(Bind::Node { var, nullable }) | Some(Bind::Value { var, nullable, .. }) => {
                    let nullable =
                        nullable || matches!(stage.scope.get(name), Some(Bind::Value { .. }));
                    if nullable && !local.contains_key(name) {
                        // A null node never matches: compare against a fresh variable.
                        let f = self.fresh(name);
                        br.filters.push(Expression::SameTerm(
                            Box::new(var_expr(&f)),
                            Box::new(var_expr(&var)),
                        ));
                        br.nodes.push(f.clone());
                        f
                    } else {
                        var
                    }
                }
                Some(other) => {
                    return Err(CypherError::semantic(format!(
                        "`{name}` is not a node (it is {})",
                        kind_name(&other)
                    )))
                }
                None => {
                    let var = self.fresh(name);
                    local.insert(
                        name.clone(),
                        Bind::Node {
                            var: var.clone(),
                            nullable: false,
                        },
                    );
                    br.nodes.push(var.clone());
                    var
                }
            },
            None => {
                let var = self.fresh("anon");
                br.nodes.push(var.clone());
                var
            }
        };
        if !np.labels.is_empty() {
            let known = stage.labels.entry(v.clone()).or_default();
            for l in &np.labels {
                let iri = self.vocab.iri(l);
                if !known.contains(&iri) {
                    known.push(iri);
                }
            }
        }
        for l in &np.labels {
            br.triples.push(tp(
                tv(&v),
                NamedNodePattern::NamedNode(rdf_type()),
                TermPattern::NamedNode(self.vocab.iri(l)),
            ));
        }
        if let Some(props) = &np.props {
            self.property_constraints(&v, props, br, stage)?;
        }
        Ok(v)
    }

    fn property_constraints(
        &mut self,
        subject: &Variable,
        props: &Expr,
        br: &mut Branch,
        stage: &mut Stage,
    ) -> Result<()> {
        let items: Vec<(String, Expr)> = match props {
            Expr::Map(items) => items.clone(),
            Expr::Param(p) => match self.params.get(p) {
                Some(Value::Map(m)) => m.iter().map(|(k, v)| (k.clone(), value_expr(v))).collect(),
                Some(Value::Null) | None => {
                    return Err(CypherError::semantic(format!(
                        "parameter ${p} must be a map"
                    )))
                }
                Some(_) => {
                    return Err(CypherError::semantic(format!(
                        "parameter ${p} must be a map"
                    )))
                }
            },
            _ => return Err(CypherError::semantic("property constraints must be a map")),
        };
        for (k, e) in items {
            let pred = self.vocab.iri(&k);
            match self.constant(&e)? {
                Some(Value::Null) => br.filters.push(bool_lit(false)),
                Some(v) => match v.to_literal()? {
                    Some(l) => br.triples.push(tp(
                        tv(subject),
                        NamedNodePattern::NamedNode(pred),
                        TermPattern::Literal(l),
                    )),
                    None => br.filters.push(bool_lit(false)),
                },
                None => {
                    let x = self.fresh(&k);
                    br.triples
                        .push(tp(tv(subject), NamedNodePattern::NamedNode(pred), tv(&x)));
                    let rhs = self.expr(&e, stage)?;
                    br.filters
                        .push(Expression::Equal(Box::new(var_expr(&x)), Box::new(rhs)));
                }
            }
        }
        Ok(())
    }

    /// Expands a pattern element into branches (one per length/direction combination),
    /// each with the walk it binds.
    fn element(
        &mut self,
        el: &PatternElement,
        br: Branch,
        stage: &mut Stage,
        local: &mut BTreeMap<String, Bind>,
    ) -> Result<Vec<(Branch, Walk)>> {
        let mut br = br;
        let start = self.node_var(&el.start, &mut br, stage, local)?;
        let mut out = vec![(
            br,
            Walk {
                nodes: vec![start],
                rels: Vec::new(),
            },
        )];
        for (rel, node) in &el.chain {
            let mut next = Vec::new();
            for (mut b, w) in out {
                let end = self.node_var(node, &mut b, stage, local)?;
                let from = w.nodes.last().expect("start").clone();
                for (b2, hops) in self.relationship(rel, &from, &end, b, stage, local)? {
                    let mut w2 = w.clone();
                    for (i, (r, to)) in hops.into_iter().enumerate() {
                        let _ = i;
                        w2.rels.push(r);
                        w2.nodes.push(to);
                    }
                    next.push((b2, w2));
                }
                if next.len() > self.opts.max_branches {
                    return Err(CypherError::unsupported(format!(
                        "pattern expands to more than {} alternatives",
                        self.opts.max_branches
                    )));
                }
            }
            out = next;
        }
        Ok(out)
    }

    fn predicate(
        &mut self,
        types: &[String],
        br: &mut Branch,
        object: &Variable,
        hint: &str,
    ) -> Pred {
        let mut uniq: Vec<String> = Vec::new();
        for t in types {
            if !uniq.contains(t) {
                uniq.push(t.clone());
            }
        }
        match uniq.as_slice() {
            [t] => Pred::Const(self.vocab.iri(t)),
            [] => {
                let p = self.fresh(&format!("{hint}t"));
                br.filters.push(rel_object_filter(object));
                br.filters.push(Expression::Not(Box::new(Expression::In(
                    Box::new(var_expr(&p)),
                    vec![
                        Expression::NamedNode(rdf_type()),
                        Expression::NamedNode(reifies()),
                    ],
                ))));
                Pred::Var(p)
            }
            many => {
                let p = self.fresh(&format!("{hint}t"));
                br.joins.push(GraphPattern::Values {
                    variables: vec![p.clone()],
                    bindings: many
                        .iter()
                        .map(|t| vec![Some(GroundTerm::NamedNode(self.vocab.iri(t)))])
                        .collect(),
                });
                Pred::Var(p)
            }
        }
    }

    /// Lowers one relationship pattern between `from` and `to`; returns the branches with
    /// the hops (relationship variables and the node reached) each one binds.
    #[allow(clippy::too_many_arguments)]
    fn relationship(
        &mut self,
        rel: &RelPattern,
        from: &Variable,
        to: &Variable,
        br: Branch,
        stage: &mut Stage,
        local: &mut BTreeMap<String, Bind>,
    ) -> Result<Vec<(Branch, Vec<(RelVars, Variable)>)>> {
        let hint = rel.var.clone().unwrap_or_else(|| "r".into());
        let prop_items = rel.props.clone();
        match rel.length {
            None => {
                let existing = rel
                    .var
                    .as_ref()
                    .and_then(|n| local.get(n).or_else(|| stage.scope.get(n)).cloned());
                let needs_rid = rel.var.is_some() || prop_items.is_some() || self.in_path;
                let mut out = Vec::new();
                let dirs: &[bool] = match rel.dir {
                    Direction::Right => &[true],
                    Direction::Left => &[false],
                    Direction::Both => &[true, false],
                };
                let rv = self.fresh_rel(&hint);
                for &forward in dirs {
                    let mut b = br.clone();
                    let (s, o) = if forward {
                        (from.clone(), to.clone())
                    } else {
                        (to.clone(), from.clone())
                    };
                    if !forward && rel.dir == Direction::Both {
                        // A self-loop matches an undirected pattern once.
                        b.filters
                            .push(Expression::Not(Box::new(Expression::SameTerm(
                                Box::new(var_expr(&s)),
                                Box::new(var_expr(&o)),
                            ))));
                    }
                    let pred = self.predicate(&rel.types, &mut b, &o, &hint);
                    self.hop_triple(
                        &mut b,
                        &s,
                        &pred,
                        &o,
                        needs_rid,
                        prop_items.as_ref(),
                        &rv,
                        stage,
                    )?;
                    let rvars = RelVars {
                        s: rv.s.clone(),
                        p: rv.p.clone(),
                        o: rv.o.clone(),
                        rid: rv.rid.clone(),
                    };
                    b.extends.push((rv.s.clone(), var_expr(&s)));
                    b.extends.push((rv.o.clone(), var_expr(&o)));
                    b.extends.push((rv.p.clone(), pred.expr()));
                    if let Some(Bind::Rel { r, .. }) = &existing {
                        // Re-matching an already bound relationship.
                        for (x, y) in [(&r.s, &s), (&r.o, &o)] {
                            b.filters.push(Expression::SameTerm(
                                Box::new(var_expr(x)),
                                Box::new(var_expr(y)),
                            ));
                        }
                        b.filters.push(Expression::SameTerm(
                            Box::new(var_expr(&r.p)),
                            Box::new(pred.expr()),
                        ));
                    }
                    out.push((b, vec![(rvars, to.clone())]));
                }
                match (&rel.var, existing) {
                    (Some(_), Some(Bind::Rel { .. })) => {}
                    (Some(n), Some(other)) => {
                        return Err(CypherError::semantic(format!(
                            "`{n}` is not a relationship (it is {})",
                            kind_name(&other)
                        )))
                    }
                    (Some(n), None) => {
                        local.insert(
                            n.clone(),
                            Bind::Rel {
                                r: rv,
                                nullable: false,
                            },
                        );
                    }
                    (None, _) => {}
                }
                Ok(out)
            }
            Some((min, None))
                if rel.var.is_none()
                    && prop_items.is_none()
                    && !self.in_path
                    && rel.dir != Direction::Both
                    && min.unwrap_or(1) <= 1 =>
            {
                // Unbounded and unnamed: reachability through a property path (a recursive CTE
                // seeded from a bound end), instead of expanding every trail.
                let min = min.unwrap_or(1);
                self.notes.push(
                    "unbounded variable-length relationship evaluated as reachability (one row per end pair, not per trail)".into(),
                );
                let preds: Vec<NamedNode> = {
                    let mut u: Vec<NamedNode> = Vec::new();
                    for t in &rel.types {
                        let iri = self.vocab.iri(t);
                        if !u.contains(&iri) {
                            u.push(iri);
                        }
                    }
                    u
                };
                let step = if preds.is_empty() {
                    PropertyPathExpression::NegatedPropertySet(vec![rdf_type(), reifies()])
                } else {
                    let mut it = preds.into_iter().map(PropertyPathExpression::NamedNode);
                    let first = it.next().expect("one");
                    it.fold(first, |a, b| {
                        PropertyPathExpression::Alternative(Box::new(a), Box::new(b))
                    })
                };
                let step = match rel.dir {
                    Direction::Right => step,
                    Direction::Left => PropertyPathExpression::Reverse(Box::new(step)),
                    Direction::Both => PropertyPathExpression::Alternative(
                        Box::new(step.clone()),
                        Box::new(PropertyPathExpression::Reverse(Box::new(step))),
                    ),
                };
                let mut path = match min {
                    0 => PropertyPathExpression::ZeroOrMore(Box::new(step.clone())),
                    _ => PropertyPathExpression::OneOrMore(Box::new(step.clone())),
                };
                for _ in 1..min.max(1) {
                    path = PropertyPathExpression::Sequence(Box::new(step.clone()), Box::new(path));
                }
                let mut b = br;
                b.joins.push(GraphPattern::Path {
                    subject: tv(from),
                    path,
                    object: tv(to),
                });
                if rel.types.is_empty() {
                    b.filters.push(rel_object_filter(to));
                }
                b.bound.push(from.clone());
                b.bound.push(to.clone());
                Ok(vec![(b, Vec::new())])
            }
            Some((min, max)) => {
                let min = min.unwrap_or(1) as usize;
                let max = match max {
                    Some(m) => m as usize,
                    None => {
                        self.notes.push(format!(
                            "unbounded variable-length relationship capped at {} hops",
                            self.opts.var_length_cap
                        ));
                        self.opts.var_length_cap
                    }
                };
                if max < min {
                    return Ok(Vec::new());
                }
                if let Some(n) = &rel.var {
                    if local.contains_key(n) || stage.scope.contains_key(n) {
                        return Err(CypherError::unsupported(format!(
                            "re-using the variable-length relationship variable `{n}`"
                        )));
                    }
                }
                let needs_rid = rel.var.is_some() || prop_items.is_some() || self.in_path;
                // Positional variables shared by every branch.
                let slots: Vec<RelVars> = (0..max)
                    .map(|i| self.fresh_rel(&format!("{hint}{i}")))
                    .collect();
                let inner: Vec<Variable> = (0..max)
                    .map(|i| self.fresh(&format!("{hint}n{i}")))
                    .collect();
                let len = self.fresh(&format!("{hint}len"));
                let mut out = Vec::new();
                for k in min..=max {
                    // Undirected hops are a UNION of both directions each, so the number of
                    // alternatives stays one per length.
                    let undirected = rel.dir == Direction::Both;
                    let combos: Vec<Vec<bool>> = match rel.dir {
                        Direction::Left => vec![vec![false; k]],
                        _ => vec![vec![true; k]],
                    };
                    for dirs in combos {
                        let mut b = br.clone();
                        b.extends
                            .push((len.clone(), Expression::Literal(Literal::from(k as i64))));
                        if k == 0 {
                            b.filters.push(Expression::SameTerm(
                                Box::new(var_expr(from)),
                                Box::new(var_expr(to)),
                            ));
                            out.push((b, Vec::new()));
                            continue;
                        }
                        let mut hops = Vec::new();
                        let mut cur = from.clone();
                        for (i, forward) in dirs.iter().enumerate() {
                            let next = if i + 1 == k {
                                to.clone()
                            } else {
                                inner[i].clone()
                            };
                            if i + 1 < k {
                                b.nodes.push(next.clone());
                            }
                            let slot = &slots[i];
                            if undirected {
                                let mut sides = Vec::new();
                                for (side, (s, o)) in
                                    [(&cur, &next), (&next, &cur)].into_iter().enumerate()
                                {
                                    let mut hb = Branch::default();
                                    if side == 1 {
                                        hb.filters.push(Expression::Not(Box::new(
                                            Expression::SameTerm(
                                                Box::new(var_expr(s)),
                                                Box::new(var_expr(o)),
                                            ),
                                        )));
                                    }
                                    let pred = self.predicate(&rel.types, &mut hb, o, &hint);
                                    self.hop_triple(
                                        &mut hb,
                                        s,
                                        &pred,
                                        o,
                                        needs_rid,
                                        prop_items.as_ref(),
                                        slot,
                                        stage,
                                    )?;
                                    hb.hops.clear();
                                    hb.extends.push((slot.s.clone(), var_expr(s)));
                                    hb.extends.push((slot.o.clone(), var_expr(o)));
                                    hb.extends.push((slot.p.clone(), pred.expr()));
                                    let (side_p, ext, fil) = hb.into_parts(self);
                                    let mut side_p = side_p;
                                    for (v, e) in ext {
                                        side_p = GraphPattern::Extend {
                                            inner: Box::new(side_p),
                                            variable: v,
                                            expression: e,
                                        };
                                    }
                                    for f in fil {
                                        side_p = filter(side_p, f);
                                    }
                                    sides.push(side_p);
                                }
                                let right = sides.pop().expect("two");
                                let left = sides.pop().expect("two");
                                b.joins.push(GraphPattern::Union {
                                    left: Box::new(left),
                                    right: Box::new(right),
                                });
                                b.bound.push(cur.clone());
                                b.bound.push(next.clone());
                                b.bound.extend(slot.vars());
                                b.hops.push(Hop {
                                    s: slot.s.clone(),
                                    p: Pred::Var(slot.p.clone()),
                                    o: slot.o.clone(),
                                    rid: (needs_rid || self.opts.reifier_uniqueness)
                                        .then(|| slot.rid.clone()),
                                });
                                hops.push((slot.clone(), next.clone()));
                                cur = next;
                                continue;
                            }
                            let (s, o) = if *forward {
                                (cur.clone(), next.clone())
                            } else {
                                (next.clone(), cur.clone())
                            };
                            let pred = self.predicate(&rel.types, &mut b, &o, &hint);
                            self.hop_triple(
                                &mut b,
                                &s,
                                &pred,
                                &o,
                                needs_rid,
                                prop_items.as_ref(),
                                slot,
                                stage,
                            )?;
                            b.extends.push((slot.s.clone(), var_expr(&s)));
                            b.extends.push((slot.o.clone(), var_expr(&o)));
                            b.extends.push((slot.p.clone(), pred.expr()));
                            hops.push((slot.clone(), next.clone()));
                            cur = next;
                        }
                        out.push((b, hops));
                    }
                }
                if let Some(n) = &rel.var {
                    local.insert(
                        n.clone(),
                        Bind::RelList {
                            len,
                            rels: slots,
                            nullable: false,
                        },
                    );
                }
                Ok(out)
            }
        }
    }

    /// Adds the triple of one hop, its reifier and its property constraints.
    #[allow(clippy::too_many_arguments)]
    fn hop_triple(
        &mut self,
        b: &mut Branch,
        s: &Variable,
        pred: &Pred,
        o: &Variable,
        needs_rid: bool,
        props: Option<&Expr>,
        rv: &RelVars,
        stage: &mut Stage,
    ) -> Result<()> {
        b.triples.push(tp(tv(s), pred.pattern(), tv(o)));
        let triple_term = TermPattern::Triple(Box::new(tp(tv(s), pred.pattern(), tv(o))));
        let rid = if needs_rid || self.opts.reifier_uniqueness {
            Some(rv.rid.clone())
        } else {
            None
        };
        if let Some(props) = props {
            // Relationship properties live on the reifier: it must exist.
            b.triples.push(tp(
                tv(&rv.rid),
                NamedNodePattern::NamedNode(reifies()),
                triple_term,
            ));
            self.property_constraints(&rv.rid, props, b, stage)?;
        } else if rid.is_some() {
            b.optionals.push(GraphPattern::Bgp {
                patterns: vec![tp(
                    tv(&rv.rid),
                    NamedNodePattern::NamedNode(reifies()),
                    triple_term,
                )],
            });
        }
        b.hops.push(Hop {
            s: s.clone(),
            p: pred.clone(),
            o: o.clone(),
            rid,
        });
        Ok(())
    }

    // ----- expressions -----

    /// A compile-time constant value, if the expression is one.
    pub fn constant(&self, e: &Expr) -> Result<Option<Value>> {
        Ok(Some(match e {
            Expr::Null => Value::Null,
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Int(i) => Value::Int(*i),
            Expr::Float(f) => Value::Float(*f),
            Expr::Str(s) => Value::String(s.clone()),
            Expr::Param(p) => self
                .params
                .get(p)
                .cloned()
                .ok_or_else(|| CypherError::semantic(format!("missing parameter ${p}")))?,
            Expr::Unary(UnOp::Neg, inner) => match self.constant(inner)? {
                Some(Value::Int(i)) => Value::Int(-i),
                Some(Value::Float(f)) => Value::Float(-f),
                _ => return Ok(None),
            },
            Expr::List(items) => {
                let mut out = Vec::new();
                for i in items {
                    match self.constant(i)? {
                        Some(v) => out.push(v),
                        None => return Ok(None),
                    }
                }
                Value::List(out)
            }
            Expr::Map(items) => {
                let mut out = BTreeMap::new();
                for (k, i) in items {
                    match self.constant(i)? {
                        Some(v) => {
                            out.insert(k.clone(), v);
                        }
                        None => return Ok(None),
                    }
                }
                Value::Map(out)
            }
            Expr::Func { name, args, .. }
                if matches!(
                    name.as_str(),
                    "date" | "datetime" | "localdatetime" | "time" | "localtime" | "duration"
                ) && args.len() == 1 =>
            {
                match self.constant(&args[0])? {
                    Some(v) => crate::eval::function(name, vec![v])?,
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        }))
    }

    fn literal_expr(&self, v: &Value) -> Result<Expression> {
        match v.to_literal()? {
            Some(l) => Ok(Expression::Literal(l)),
            None => Err(CypherError::unsupported("null in a SQL expression")),
        }
    }

    /// The SPARQL variable holding `subject.key`, joining the property optionally.
    fn property_var(
        &mut self,
        subject: &Variable,
        key: &str,
        nullable: bool,
        stage: &mut Stage,
    ) -> Variable {
        if let Some(v) = stage.props.get(&(subject.clone(), key.to_string())) {
            return v.clone();
        }
        let v = self.fresh(key);
        let iri = self.vocab.iri(key);
        let pred = NamedNodePattern::NamedNode(iri.clone());
        // A shape's `sh:datatype` gives the compiler the property's value type.
        if let (Some(s), Some(l)) = (&self.opts.schema, stage.labels.get(subject)) {
            if let Some(t) = s.value_type(l, &iri) {
                self.var_types.insert(v.as_str().to_string(), t);
            }
        }
        let required = !nullable
            && self.opts.schema.as_ref().is_some_and(|s| {
                stage
                    .labels
                    .get(subject)
                    .is_some_and(|l| s.is_required(l, &iri))
            });
        if required {
            // A mandatory property (SHACL `sh:minCount 1`) joins without OPTIONAL.
            stage.pattern = join(
                std::mem::replace(&mut stage.pattern, unit()),
                GraphPattern::Bgp {
                    patterns: vec![tp(tv(subject), pred, tv(&v))],
                },
            );
            stage
                .props
                .insert((subject.clone(), key.to_string()), v.clone());
            return v;
        }
        let (right, expression) = if nullable {
            // An unbound subject must not join with every triple of the property.
            let x = self.fresh("subj");
            (
                GraphPattern::Bgp {
                    patterns: vec![tp(tv(&x), pred, tv(&v))],
                },
                Some(Expression::SameTerm(
                    Box::new(var_expr(&x)),
                    Box::new(var_expr(subject)),
                )),
            )
        } else {
            (
                GraphPattern::Bgp {
                    patterns: vec![tp(tv(subject), pred, tv(&v))],
                },
                None,
            )
        };
        stage.pattern = GraphPattern::LeftJoin {
            left: Box::new(std::mem::replace(&mut stage.pattern, unit())),
            right: Box::new(right),
            expression,
        };
        stage
            .props
            .insert((subject.clone(), key.to_string()), v.clone());
        v
    }

    fn is_stringy(&self, e: &Expr) -> bool {
        match e {
            Expr::Str(_) => true,
            Expr::Param(p) => matches!(self.params.get(p), Some(Value::String(_))),
            Expr::Func { name, .. } => matches!(
                name.as_str(),
                "tostring"
                    | "toupper"
                    | "tolower"
                    | "upper"
                    | "lower"
                    | "trim"
                    | "ltrim"
                    | "rtrim"
                    | "substring"
                    | "left"
                    | "right"
                    | "replace"
            ),
            Expr::Binary(BinOp::Add, a, b) => self.is_stringy(a) || self.is_stringy(b),
            _ => false,
        }
    }

    /// Lowers an expression to SPARQL. Property accesses add optional joins to the stage.
    pub fn expr(&mut self, e: &Expr, stage: &mut Stage) -> Result<Expression> {
        if let Some(v) = self.constant(e)? {
            return match v {
                Value::List(_) | Value::Map(_) => Err(CypherError::unsupported(
                    "list or map values in a SQL expression",
                )),
                // `null` is an unbound variable: SPARQL's error, which SQL evaluates to NULL.
                Value::Null => Ok(var_expr(&self.fresh("null"))),
                v => self.literal_expr(&v),
            };
        }
        Ok(match e {
            Expr::Var(name) => match stage.scope.get(name) {
                Some(Bind::Node { var, .. })
                | Some(Bind::Value {
                    var, name: false, ..
                }) => var_expr(var),
                Some(Bind::Rel { r, .. }) => {
                    return Err(CypherError::unsupported(format!(
                        "relationship `{name}` used as a value in SQL (vars {})",
                        r.rid
                    )))
                }
                Some(other) => {
                    return Err(CypherError::unsupported(format!(
                        "{} `{name}` in a SQL expression",
                        kind_name(other)
                    )))
                }
                None => {
                    return Err(CypherError::semantic(format!(
                        "variable `{name}` not defined"
                    )))
                }
            },
            Expr::Prop(target, key) => match target.as_ref() {
                Expr::Var(name) => match stage.scope.get(name).cloned() {
                    Some(Bind::Node { var, nullable }) => {
                        var_expr(&self.property_var(&var, key, nullable, stage))
                    }
                    Some(Bind::Rel { r, .. }) => {
                        var_expr(&self.property_var(&r.rid, key, true, stage))
                    }
                    Some(Bind::Map { fields, .. }) => match fields.get(key) {
                        Some(v) => var_expr(v),
                        None => {
                            return Err(CypherError::unsupported(format!(
                                "key `{key}` is not present in the unwound maps"
                            )))
                        }
                    },
                    Some(other) => {
                        return Err(CypherError::unsupported(format!(
                            "property access on {} `{name}` in SQL",
                            kind_name(&other)
                        )))
                    }
                    None => {
                        return Err(CypherError::semantic(format!(
                            "variable `{name}` not defined"
                        )))
                    }
                },
                _ => return Err(CypherError::unsupported("nested property access in SQL")),
            },
            Expr::Unary(UnOp::Not, inner) => Expression::Not(Box::new(self.expr(inner, stage)?)),
            Expr::Unary(UnOp::Neg, inner) => {
                Expression::UnaryMinus(Box::new(self.expr(inner, stage)?))
            }
            Expr::Unary(UnOp::Plus, inner) => self.expr(inner, stage)?,
            Expr::Binary(op, a, b) => self.binary(*op, a, b, stage)?,
            Expr::IsNull(inner, negated) => {
                let bound = match self.expr(inner, stage)? {
                    Expression::Variable(v) => Expression::Bound(v),
                    _ => {
                        return Err(CypherError::unsupported(
                            "IS NULL on a computed expression in SQL",
                        ))
                    }
                };
                if *negated {
                    bound
                } else {
                    Expression::Not(Box::new(bound))
                }
            }
            Expr::HasLabels(inner, labels) => {
                let Expr::Var(name) = inner.as_ref() else {
                    return Err(CypherError::unsupported("label test on an expression"));
                };
                let Some(Bind::Node { var, nullable }) = stage.scope.get(name).cloned() else {
                    return Err(CypherError::semantic(format!("`{name}` is not a node")));
                };
                let pats = labels
                    .iter()
                    .map(|l| {
                        tp(
                            tv(&var),
                            NamedNodePattern::NamedNode(rdf_type()),
                            TermPattern::NamedNode(self.vocab.iri(l)),
                        )
                    })
                    .collect();
                let ex = Expression::Exists(Box::new(GraphPattern::Bgp { patterns: pats }));
                if nullable {
                    Expression::And(Box::new(Expression::Bound(var)), Box::new(ex))
                } else {
                    ex
                }
            }
            Expr::Func {
                name,
                args,
                distinct,
            } => {
                if is_aggregate(name) {
                    return Err(CypherError::semantic(format!(
                        "aggregate {name}() in WHERE or a non-projection expression"
                    )));
                }
                let _ = distinct;
                self.function(name, args, stage)?
            }
            Expr::Case {
                operand,
                whens,
                else_,
            } => {
                let mut out = match else_ {
                    Some(e) => self.expr(e, stage)?,
                    None => return Err(CypherError::unsupported("CASE without ELSE in SQL")),
                };
                for (w, t) in whens.iter().rev() {
                    let cond = match operand {
                        Some(o) => Expression::Equal(
                            Box::new(self.expr(o, stage)?),
                            Box::new(self.expr(w, stage)?),
                        ),
                        None => self.expr(w, stage)?,
                    };
                    let cond = Expression::Coalesce(vec![cond, bool_lit(false)]);
                    out = Expression::If(
                        Box::new(cond),
                        Box::new(self.expr(t, stage)?),
                        Box::new(out),
                    );
                }
                out
            }
            Expr::Pattern(el) => self.exists(
                &[PatternPart {
                    var: None,
                    shortest: None,
                    element: (**el).clone(),
                }],
                None,
                stage,
            )?,
            Expr::Exists(parts, where_) => self.exists(parts, where_.as_deref(), stage)?,
            _ => return Err(CypherError::unsupported("this expression in SQL")),
        })
    }

    fn exists(
        &mut self,
        parts: &[PatternPart],
        where_: Option<&Expr>,
        stage: &mut Stage,
    ) -> Result<Expression> {
        let mut inner = stage.child();
        let p = self.match_patterns(parts, &mut inner, false, false)?;
        inner.pattern = p.join_into(unit());
        if let Some(w) = where_ {
            let f = self.expr(w, &mut inner)?;
            inner.pattern = filter(std::mem::replace(&mut inner.pattern, unit()), f);
        }
        // Outer variables that may be null never match.
        let mut guards = Vec::new();
        for (name, b) in &stage.scope {
            if b.nullable() && mentions(parts, name) {
                if let Bind::Node { var, .. } = b {
                    guards.push(Expression::Bound(var.clone()));
                }
            }
        }
        guards.push(Expression::Exists(Box::new(inner.pattern)));
        Ok(and_all(guards).expect("non-empty"))
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr, stage: &mut Stage) -> Result<Expression> {
        // Relationship (in)equality compares the triple and the reifier.
        if let (BinOp::Eq | BinOp::Ne, Expr::Var(x), Expr::Var(y)) = (op, a, b) {
            if let (Some(Bind::Rel { r: rx, .. }), Some(Bind::Rel { r: ry, .. })) =
                (stage.scope.get(x), stage.scope.get(y))
            {
                let same = same_edge(
                    &Hop {
                        s: rx.s.clone(),
                        p: Pred::Var(rx.p.clone()),
                        o: rx.o.clone(),
                        rid: Some(rx.rid.clone()),
                    },
                    &Hop {
                        s: ry.s.clone(),
                        p: Pred::Var(ry.p.clone()),
                        o: ry.o.clone(),
                        rid: Some(ry.rid.clone()),
                    },
                );
                return Ok(if op == BinOp::Eq {
                    same
                } else {
                    Expression::Not(Box::new(same))
                });
            }
        }
        // `type(r) = 'T'` compares the predicate IRI.
        if let (BinOp::Eq | BinOp::Ne, Expr::Func { name, args, .. }, Some(Value::String(t))) =
            (op, a, self.constant(b)?)
        {
            if name == "type" && args.len() == 1 {
                if let Expr::Var(r) = &args[0] {
                    if let Some(Bind::Rel { r, .. }) = stage.scope.get(r) {
                        let eq = Expression::SameTerm(
                            Box::new(var_expr(&r.p)),
                            Box::new(Expression::NamedNode(self.vocab.iri(&t))),
                        );
                        return Ok(if op == BinOp::Eq {
                            eq
                        } else {
                            Expression::Not(Box::new(eq))
                        });
                    }
                }
            }
        }
        if op == BinOp::In {
            let x = self.expr(a, stage)?;
            return match self.constant(b)? {
                Some(Value::List(items)) => {
                    let mut out = Vec::new();
                    for i in items {
                        if !i.is_null() {
                            out.push(self.literal_expr(&i)?);
                        }
                    }
                    Ok(Expression::In(Box::new(x), out))
                }
                Some(Value::Null) => Err(CypherError::unsupported("IN null in SQL")),
                _ => match b {
                    Expr::Func { name, args, .. } if name == "labels" && args.len() == 1 => {
                        // `'L' IN labels(n)`
                        let Some(Value::String(l)) = self.constant(a)? else {
                            return Err(CypherError::unsupported(
                                "IN labels() with a computed label",
                            ));
                        };
                        self.expr(&Expr::HasLabels(Box::new(args[0].clone()), vec![l]), stage)
                    }
                    _ => Err(CypherError::unsupported(
                        "IN over a non-constant list in SQL",
                    )),
                },
            };
        }
        let x = self.expr(a, stage)?;
        let y = self.expr(b, stage)?;
        let bx = Box::new(x.clone());
        let by = Box::new(y.clone());
        Ok(match op {
            BinOp::Or => Expression::Or(bx, by),
            BinOp::And => Expression::And(bx, by),
            BinOp::Xor => Expression::And(
                Box::new(Expression::Or(bx.clone(), by.clone())),
                Box::new(Expression::Not(Box::new(Expression::And(bx, by)))),
            ),
            BinOp::Eq => Expression::Equal(bx, by),
            BinOp::Ne => Expression::Not(Box::new(Expression::Equal(bx, by))),
            BinOp::Lt => Expression::Less(bx, by),
            BinOp::Le => Expression::LessOrEqual(bx, by),
            BinOp::Gt => Expression::Greater(bx, by),
            BinOp::Ge => Expression::GreaterOrEqual(bx, by),
            BinOp::Add => {
                if self.is_stringy(a) || self.is_stringy(b) {
                    fcall(
                        Function::Concat,
                        vec![fcall(Function::Str, vec![x]), fcall(Function::Str, vec![y])],
                    )
                } else {
                    Expression::Add(bx, by)
                }
            }
            BinOp::Sub => Expression::Subtract(bx, by),
            BinOp::Mul => Expression::Multiply(bx, by),
            BinOp::Div => div_expr(x, y),
            BinOp::Mod => Expression::Subtract(
                bx.clone(),
                Box::new(Expression::Multiply(
                    by.clone(),
                    Box::new(xsd_fn(xsd::INTEGER, Expression::Divide(bx, by))),
                )),
            ),
            BinOp::StartsWith => fcall(Function::StrStarts, vec![x, y]),
            BinOp::EndsWith => fcall(Function::StrEnds, vec![x, y]),
            BinOp::Contains => fcall(Function::Contains, vec![x, y]),
            BinOp::Regex => {
                let Some(Value::String(pat)) = self.constant(b)? else {
                    return Err(CypherError::unsupported(
                        "=~ with a computed pattern in SQL",
                    ));
                };
                let (flags, pat) = match pat.strip_prefix("(?i)") {
                    Some(rest) => (Some("i"), rest.to_string()),
                    None => (None, pat),
                };
                let mut args = vec![
                    x,
                    Expression::Literal(Literal::new_simple_literal(format!("^(?:{pat})$"))),
                ];
                if let Some(f) = flags {
                    args.push(Expression::Literal(Literal::new_simple_literal(f)));
                }
                fcall(Function::Regex, args)
            }
            BinOp::Pow => return Err(CypherError::unsupported("^ in SQL")),
            BinOp::In => unreachable!("handled above"),
        })
    }

    fn function(&mut self, name: &str, args: &[Expr], stage: &mut Stage) -> Result<Expression> {
        let mut lowered = Vec::new();
        let lower_all =
            |me: &mut Self, stage: &mut Stage, out: &mut Vec<Expression>| -> Result<()> {
                for a in args {
                    out.push(me.expr(a, stage)?);
                }
                Ok(())
            };
        Ok(match name {
            "id" | "elementid" => match args {
                [Expr::Var(n)] => match stage.scope.get(n) {
                    Some(Bind::Node { var, .. }) => fcall(Function::Str, vec![var_expr(var)]),
                    Some(Bind::Rel { r, .. }) => fcall(Function::Str, vec![var_expr(&r.rid)]),
                    _ => return Err(CypherError::unsupported("id() of a non-entity")),
                },
                _ => return Err(CypherError::unsupported("id() of an expression")),
            },
            "exists" => match args {
                [Expr::Prop(..)] => match self.expr(&args[0], stage)? {
                    Expression::Variable(v) => Expression::Bound(v),
                    _ => return Err(CypherError::unsupported("exists() of an expression")),
                },
                [Expr::Pattern(_)] => self.expr(&args[0], stage)?,
                _ => return Err(CypherError::unsupported("exists() of an expression")),
            },
            "coalesce" => {
                lower_all(self, stage, &mut lowered)?;
                Expression::Coalesce(lowered)
            }
            "toupper" | "upper" => {
                lower_all(self, stage, &mut lowered)?;
                fcall(Function::UCase, lowered)
            }
            "tolower" | "lower" => {
                lower_all(self, stage, &mut lowered)?;
                fcall(Function::LCase, lowered)
            }
            "length" | "size"
                if args.len() == 1
                    && matches!(&args[0], Expr::Var(v) if matches!(stage.scope.get(v.as_str()), Some(Bind::Path { .. } | Bind::RelList { .. }))) =>
            {
                let Expr::Var(v) = &args[0] else {
                    unreachable!()
                };
                match stage.scope.get(v.as_str()) {
                    Some(Bind::Path { len, .. } | Bind::RelList { len, .. }) => var_expr(len),
                    _ => unreachable!(),
                }
            }
            "size" | "length" if args.len() == 1 && !matches!(args[0], Expr::Var(_)) => {
                lower_all(self, stage, &mut lowered)?;
                fcall(Function::StrLen, lowered)
            }
            "trim" => {
                lower_all(self, stage, &mut lowered)?;
                lowered.push(Expression::Literal(Literal::new_simple_literal(
                    r"^\s+|\s+$",
                )));
                lowered.push(Expression::Literal(Literal::new_simple_literal("")));
                fcall(Function::Replace, lowered)
            }
            "substring" if args.len() >= 2 => {
                lower_all(self, stage, &mut lowered)?;
                lowered[1] = Expression::Add(
                    Box::new(lowered[1].clone()),
                    Box::new(Expression::Literal(Literal::from(1_i64))),
                );
                fcall(Function::SubStr, lowered)
            }
            "left" if args.len() == 2 => {
                lower_all(self, stage, &mut lowered)?;
                fcall(
                    Function::SubStr,
                    vec![
                        lowered[0].clone(),
                        Expression::Literal(Literal::from(1_i64)),
                        lowered[1].clone(),
                    ],
                )
            }
            "tostring" => {
                lower_all(self, stage, &mut lowered)?;
                fcall(Function::Str, lowered)
            }
            "tointeger" | "toint" => {
                lower_all(self, stage, &mut lowered)?;
                xsd_fn(xsd::INTEGER, lowered.remove(0))
            }
            "tofloat" => {
                lower_all(self, stage, &mut lowered)?;
                xsd_fn(xsd::DOUBLE, lowered.remove(0))
            }
            "abs" => {
                lower_all(self, stage, &mut lowered)?;
                fcall(Function::Abs, lowered)
            }
            "ceil" => {
                lower_all(self, stage, &mut lowered)?;
                fcall(Function::Ceil, lowered)
            }
            "floor" => {
                lower_all(self, stage, &mut lowered)?;
                fcall(Function::Floor, lowered)
            }
            "round" => {
                lower_all(self, stage, &mut lowered)?;
                fcall(Function::Round, lowered)
            }
            "rand" => fcall(Function::Rand, vec![]),
            other => return Err(CypherError::unsupported(format!("{other}() in SQL"))),
        })
    }

    // ----- projections -----

    /// Lowers `WITH` / `RETURN` to SPARQL. Returns the new stage and the projected columns.
    /// `plain_only` rejects aggregation (used when the Rust tail evaluates the items).
    pub fn projection(
        &mut self,
        proj: &Projection,
        mut stage: Stage,
        final_items: bool,
    ) -> Result<(Stage, Vec<String>)> {
        let _ = final_items;
        let mut items: Vec<(String, Expr)> = Vec::new();
        if proj.star {
            for name in stage.scope.keys() {
                if !name.starts_with('\u{1}') {
                    items.push((name.clone(), Expr::Var(name.clone())));
                }
            }
        }
        for it in &proj.items {
            items.push((it.name(), it.expr.clone()));
        }
        let names: Vec<String> = items.iter().map(|(n, _)| n.clone()).collect();
        let aggregating = items.iter().any(|(_, e)| e.has_aggregate());
        let mut min_max = 0;
        for (_, e) in &items {
            e.walk(&mut |x| {
                if let Expr::Func { name, .. } = x {
                    if name == "min" || name == "max" {
                        min_max += 1;
                    }
                }
            });
        }
        if min_max > 1 {
            // Only one MIN/MAX per group compiles to SQL, and SPARQL's fallback turns a null
            // into an error: the Rust aggregation ignores nulls like Cypher.
            return Err(CypherError::unsupported(
                "several min()/max() in one projection",
            ));
        }
        let mut new_scope = Scope::new();
        let mut project_vars: Vec<Variable> = Vec::new();
        let mut alias_vars: HashMap<String, Expression> = HashMap::new();
        if aggregating {
            let mut group_vars: Vec<Variable> = Vec::new();
            let mut aggregates: Vec<(Variable, AggregateExpression)> = Vec::new();
            let mut post: Vec<(Variable, Expression)> = Vec::new();
            for (name, e) in &items {
                if !e.has_aggregate() {
                    let b = self.bind_of_expr(e, &mut stage)?;
                    group_vars.extend(b.vars());
                    for v in b.vars() {
                        project_vars.push(v);
                    }
                    if let Bind::Value { var, .. } | Bind::Node { var, .. } = &b {
                        alias_vars.insert(name.clone(), var_expr(var));
                    }
                    new_scope.insert(name.clone(), b);
                } else {
                    let (out, aggs) = self.aggregate_item(e, &mut stage)?;
                    aggregates.extend(aggs);
                    let var = match out {
                        Expression::Variable(v) => v,
                        other => {
                            let v = self.fresh(name);
                            post.push((v.clone(), other));
                            v
                        }
                    };
                    project_vars.push(var.clone());
                    alias_vars.insert(name.clone(), var_expr(&var));
                    new_scope.insert(
                        name.clone(),
                        Bind::Value {
                            var,
                            nullable: true,
                            name: false,
                        },
                    );
                }
            }
            // Group keys must be variables: they are.
            stage.pattern = GraphPattern::Group {
                inner: Box::new(std::mem::replace(&mut stage.pattern, unit())),
                variables: dedup(group_vars),
                aggregates,
            };
            for (v, e) in post {
                stage.pattern = GraphPattern::Extend {
                    inner: Box::new(std::mem::replace(&mut stage.pattern, unit())),
                    variable: v,
                    expression: e,
                };
            }
            // Seal the aggregated rows into a subquery before ordering.
            stage.pattern = GraphPattern::Project {
                inner: Box::new(std::mem::replace(&mut stage.pattern, unit())),
                variables: dedup(project_vars.clone()),
            };
            // After grouping only the projected names are visible.
            stage.scope = new_scope.clone();
            stage.props.clear();
        } else {
            for (name, e) in &items {
                let b = self.bind_of_expr(e, &mut stage)?;
                project_vars.extend(b.vars());
                if let Bind::Value { var, .. } | Bind::Node { var, .. } = &b {
                    alias_vars.insert(name.clone(), var_expr(var));
                }
                new_scope.insert(name.clone(), b);
            }
        }
        // Without aggregation, WITH … WHERE may read variables of the previous scope: it
        // filters before the projection.
        let mut early_where = false;
        if let Some(w) = &proj.where_ {
            let mut old_refs = false;
            w.walk(&mut |x| {
                if let Expr::Var(v) = x {
                    if !new_scope.contains_key(v) {
                        old_refs = true;
                    }
                }
            });
            if old_refs && !aggregating && proj.skip.is_none() && proj.limit.is_none() {
                let mut where_scope = stage.clone();
                for (k, b) in &new_scope {
                    where_scope.scope.insert(k.clone(), b.clone());
                }
                let f = self.expr(w, &mut where_scope)?;
                stage.pattern = filter(where_scope.pattern, f);
                early_where = true;
            }
        }
        // SQL orders temporal values by their lexical form: sort them in Rust.
        for (e, _) in &proj.order {
            let mut temporal = false;
            e.walk(&mut |x| {
                if let Expr::Func { name, .. } = x {
                    if matches!(
                        name.split('.').next(),
                        Some(
                            "date"
                                | "datetime"
                                | "localdatetime"
                                | "time"
                                | "localtime"
                                | "duration"
                        )
                    ) {
                        temporal = true;
                    }
                }
            });
            if temporal {
                return Err(CypherError::unsupported(
                    "ORDER BY over temporal values in SQL",
                ));
            }
        }
        // ORDER BY sees the projected aliases (and, without aggregation, the old variables).
        if !proj.order.is_empty() {
            let mut order_scope = stage.clone();
            for (k, b) in &new_scope {
                order_scope.scope.insert(k.clone(), b.clone());
            }
            let mut keys = Vec::new();
            // Projected expressions inside order keys read the projected columns.
            let targets: Vec<(Expr, Expr)> = items
                .iter()
                .filter(|(n, _)| alias_vars.contains_key(n))
                .map(|(n, ie)| (ie.clone(), Expr::Var(n.clone())))
                .collect();
            for (e, asc) in &proj.order {
                let e = &e.replace_subexprs(&targets);
                let key = match e {
                    Expr::Var(n) if alias_vars.contains_key(n) => alias_vars[n].clone(),
                    _ => self.expr(e, &mut order_scope)?,
                };
                // Cypher sorts nulls last (ascending) and first (descending).
                let (null_key, key) = match &key {
                    Expression::Variable(v) => (
                        Expression::Not(Box::new(Expression::Bound(v.clone()))),
                        key.clone(),
                    ),
                    _ => {
                        let v = self.fresh("ord");
                        order_scope.pattern = GraphPattern::Extend {
                            inner: Box::new(std::mem::replace(&mut order_scope.pattern, unit())),
                            variable: v.clone(),
                            expression: key,
                        };
                        (
                            Expression::Not(Box::new(Expression::Bound(v.clone()))),
                            var_expr(&v),
                        )
                    }
                };
                if *asc {
                    keys.push(OrderExpression::Asc(null_key));
                    keys.push(OrderExpression::Asc(key));
                } else {
                    keys.push(OrderExpression::Desc(null_key));
                    keys.push(OrderExpression::Desc(key));
                }
            }
            stage.pattern = GraphPattern::OrderBy {
                inner: Box::new(order_scope.pattern),
                expression: keys,
            };
        }
        stage.pattern = GraphPattern::Project {
            inner: Box::new(std::mem::replace(&mut stage.pattern, unit())),
            variables: dedup(project_vars),
        };
        if proj.distinct {
            stage.pattern = GraphPattern::Distinct {
                inner: Box::new(std::mem::replace(&mut stage.pattern, unit())),
            };
        }
        let skip = match &proj.skip {
            Some(e) => Some(self.count(e, "SKIP")?),
            None => None,
        };
        let limit = match &proj.limit {
            Some(e) => Some(self.count(e, "LIMIT")?),
            None => None,
        };
        if skip.is_some() || limit.is_some() {
            stage.pattern = GraphPattern::Slice {
                inner: Box::new(std::mem::replace(&mut stage.pattern, unit())),
                start: skip.unwrap_or(0),
                length: limit,
            };
        }
        stage.scope = new_scope;
        stage.props.clear();
        if let (Some(w), false) = (&proj.where_, early_where) {
            let f = self.expr(w, &mut stage)?;
            stage.pattern = filter(std::mem::replace(&mut stage.pattern, unit()), f);
        }
        Ok((stage, names))
    }

    fn count(&self, e: &Expr, what: &str) -> Result<usize> {
        match self.constant(e)? {
            Some(Value::Int(i)) if i >= 0 => Ok(i as usize),
            _ => Err(CypherError::semantic(format!(
                "{what} must be a non-negative integer constant or parameter"
            ))),
        }
    }

    /// How a projected (non-aggregate) expression is bound: entity variables keep their
    /// SPARQL variables, other expressions become a computed variable.
    fn bind_of_expr(&mut self, e: &Expr, stage: &mut Stage) -> Result<Bind> {
        if let Expr::Var(n) = e {
            return stage
                .scope
                .get(n)
                .cloned()
                .ok_or_else(|| CypherError::semantic(format!("variable `{n}` not defined")));
        }
        if matches!(e, Expr::Null) {
            let v = self.fresh("null");
            return Ok(Bind::Value {
                var: v,
                nullable: true,
                name: false,
            });
        }
        let ex = self.expr(e, stage)?;
        let var = match ex {
            Expression::Variable(v) => v,
            other => {
                let v = self.fresh("x");
                stage.pattern = GraphPattern::Extend {
                    inner: Box::new(std::mem::replace(&mut stage.pattern, unit())),
                    variable: v.clone(),
                    expression: other,
                };
                v
            }
        };
        Ok(Bind::Value {
            var,
            nullable: true,
            name: false,
        })
    }

    /// Replaces aggregate calls with fresh variables; returns the rewritten expression and
    /// the aggregates to compute.
    fn aggregate_item(
        &mut self,
        e: &Expr,
        stage: &mut Stage,
    ) -> Result<(Expression, Vec<(Variable, AggregateExpression)>)> {
        let mut aggs = Vec::new();
        let out = self.agg_rewrite(e, stage, &mut aggs)?;
        Ok((out, aggs))
    }

    fn agg_rewrite(
        &mut self,
        e: &Expr,
        stage: &mut Stage,
        aggs: &mut Vec<(Variable, AggregateExpression)>,
    ) -> Result<Expression> {
        match e {
            Expr::CountStar => {
                let v = self.fresh("count");
                aggs.push((
                    v.clone(),
                    AggregateExpression::CountSolutions { distinct: false },
                ));
                Ok(var_expr(&v))
            }
            Expr::Func {
                name,
                distinct,
                args,
            } if is_aggregate(name) => {
                let f = match name.as_str() {
                    "count" => AggregateFunction::Count,
                    "sum" => AggregateFunction::Sum,
                    "avg" => AggregateFunction::Avg,
                    "min" => AggregateFunction::Min,
                    "max" => AggregateFunction::Max,
                    other => {
                        return Err(CypherError::unsupported(format!(
                            "aggregate {other}() in SQL"
                        )))
                    }
                };
                let [arg] = args.as_slice() else {
                    return Err(CypherError::semantic(format!(
                        "{name}() takes one argument"
                    )));
                };
                let expr = match arg {
                    Expr::Var(n) => match stage.scope.get(n) {
                        Some(Bind::Rel { r, .. }) => {
                            // Relationship identity: the triple and its reifier.
                            fcall(
                                Function::Concat,
                                vec![
                                    fcall(Function::Str, vec![var_expr(&r.s)]),
                                    fcall(Function::Str, vec![var_expr(&r.p)]),
                                    fcall(Function::Str, vec![var_expr(&r.o)]),
                                    Expression::Coalesce(vec![
                                        fcall(Function::Str, vec![var_expr(&r.rid)]),
                                        Expression::Literal(Literal::new_simple_literal("")),
                                    ]),
                                ],
                            )
                        }
                        _ => self.expr(arg, stage)?,
                    },
                    _ => self.expr(arg, stage)?,
                };
                let v = self.fresh(name);
                aggs.push((
                    v.clone(),
                    AggregateExpression::FunctionCall {
                        name: f,
                        expr,
                        distinct: *distinct,
                    },
                ));
                if name == "avg" {
                    // SPARQL's AVG of nothing is 0; Cypher's is null.
                    let c = self.fresh("n");
                    aggs.push((
                        c.clone(),
                        AggregateExpression::FunctionCall {
                            name: AggregateFunction::Count,
                            expr: match &aggs.last().expect("pushed") {
                                (_, AggregateExpression::FunctionCall { expr, .. }) => expr.clone(),
                                _ => unreachable!(),
                            },
                            distinct: *distinct,
                        },
                    ));
                    return Ok(Expression::If(
                        Box::new(Expression::Greater(
                            Box::new(var_expr(&c)),
                            Box::new(Expression::Literal(Literal::from(0_i64))),
                        )),
                        Box::new(var_expr(&v)),
                        Box::new(fcall(
                            Function::StrDt,
                            vec![
                                Expression::Literal(Literal::new_simple_literal("")),
                                Expression::NamedNode(xsd::INTEGER.into_owned()),
                            ],
                        )),
                    ));
                }
                Ok(var_expr(&v))
            }
            Expr::Binary(op, a, b) => {
                let x = self.agg_rewrite(a, stage, aggs)?;
                let y = self.agg_rewrite(b, stage, aggs)?;
                let (bx, by) = (Box::new(x), Box::new(y));
                Ok(match op {
                    BinOp::Add => Expression::Add(bx, by),
                    BinOp::Sub => Expression::Subtract(bx, by),
                    BinOp::Mul => Expression::Multiply(bx, by),
                    BinOp::Div => div_expr(*bx, *by),
                    BinOp::Eq => Expression::Equal(bx, by),
                    BinOp::Lt => Expression::Less(bx, by),
                    BinOp::Gt => Expression::Greater(bx, by),
                    _ => {
                        return Err(CypherError::unsupported(
                            "this operator around an aggregate in SQL",
                        ))
                    }
                })
            }
            e if !e.has_aggregate() => match self.constant(e)? {
                Some(v) => self.literal_expr(&v),
                None => Err(CypherError::unsupported(
                    "a non-constant expression next to an aggregate in the same item",
                )),
            },
            _ => Err(CypherError::unsupported("this aggregate expression in SQL")),
        }
    }
}

/// Cypher division: integers divide with truncation.
fn div_expr(x: Expression, y: Expression) -> Expression {
    let is_int = |e: &Expression| {
        Expression::Equal(
            Box::new(fcall(Function::Datatype, vec![e.clone()])),
            Box::new(Expression::NamedNode(xsd::INTEGER.into_owned())),
        )
    };
    Expression::If(
        Box::new(Expression::And(Box::new(is_int(&x)), Box::new(is_int(&y)))),
        Box::new(xsd_fn(
            xsd::INTEGER,
            Expression::Divide(Box::new(x.clone()), Box::new(y.clone())),
        )),
        Box::new(Expression::Divide(Box::new(x), Box::new(y))),
    )
}

fn dedup(vars: Vec<Variable>) -> Vec<Variable> {
    let mut seen = BTreeSet::new();
    vars.into_iter()
        .filter(|v| seen.insert(v.clone()))
        .collect()
}

/// Relationship objects are IRIs or blank nodes (literals are properties).
fn rel_object_filter(o: &Variable) -> Expression {
    Expression::Or(
        Box::new(fcall(Function::IsIri, vec![var_expr(o)])),
        Box::new(fcall(Function::IsBlank, vec![var_expr(o)])),
    )
}

pub(crate) fn value_expr(v: &Value) -> Expr {
    match v {
        Value::Null => Expr::Null,
        Value::Bool(b) => Expr::Bool(*b),
        Value::Int(i) => Expr::Int(*i),
        Value::Float(f) => Expr::Float(*f),
        Value::String(s) => Expr::Str(s.clone()),
        Value::List(items) => Expr::List(items.iter().map(value_expr).collect()),
        Value::Map(m) => Expr::Map(m.iter().map(|(k, v)| (k.clone(), value_expr(v))).collect()),
        Value::Temporal(k, s) => Expr::Func {
            name: match k {
                crate::value::TemporalKind::Date => "date",
                crate::value::TemporalKind::DateTime => "datetime",
                crate::value::TemporalKind::LocalDateTime => "localdatetime",
                crate::value::TemporalKind::Time => "time",
                crate::value::TemporalKind::LocalTime => "localtime",
                crate::value::TemporalKind::Duration => "duration",
            }
            .into(),
            distinct: false,
            args: vec![Expr::Str(s.clone())],
        },
        Value::Node(_) | Value::Relationship(_) | Value::Path(_) => Expr::Null,
    }
}

pub(crate) fn kind_name(b: &Bind) -> &'static str {
    match b {
        Bind::Node { .. } => "a node",
        Bind::Rel { .. } => "a relationship",
        Bind::Path { .. } => "a path",
        Bind::RelList { .. } => "a relationship list",
        Bind::Value { .. } => "a value",
        Bind::Map { .. } => "a map",
    }
}

fn mentions(parts: &[PatternPart], name: &str) -> bool {
    parts.iter().any(|p| {
        p.element.start.var.as_deref() == Some(name)
            || p.element
                .chain
                .iter()
                .any(|(r, n)| r.var.as_deref() == Some(name) || n.var.as_deref() == Some(name))
    })
}
