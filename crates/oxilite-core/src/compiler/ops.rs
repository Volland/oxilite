//! OPTIONAL, UNION, MINUS, GROUP BY / aggregates and property paths (milestone M2).
//!
// @lat: [[architecture#SPARQL to SQL compiler#Graph patterns]]

use super::expr::{Stat, INT_BASE, V};
use super::plan::Pos;
use super::{
    val_fields, Binding, Block, Col, Compiler, FromItem, GraphScope, Join, Stage, VAL_FIELDS,
};
use crate::error::{Error, Result};
use oxrdf::Variable;
use spargebra::algebra::{
    AggregateExpression, AggregateFunction, Expression, GraphPattern, PropertyPathExpression,
};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};
use std::collections::{BTreeMap, HashMap};

/// How a variable is rendered in a UNION branch.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Id,
    Val,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Closure {
    ZeroOrMore,
    OneOrMore,
    ZeroOrOne,
}

/// SPARQL `MIN`/`MAX` ordering key as a single SQLite value: numbers first, then dates,
/// then lexical forms (SQLite orders INTEGER/REAL before TEXT).
fn sort_key(v: &V) -> String {
    format!("COALESCE({}, {}, {})", v.num, v.ts, v.lex)
}

/// Highest numeric type rank without the `max()` aggregate (which would change SQLite's
/// bare-column semantics used by `MIN`/`MAX`).
fn max_rank(nt: &str) -> String {
    format!(
        "(CASE WHEN SUM(({nt}) = 4) > 0 THEN 4 WHEN SUM(({nt}) = 3) > 0 THEN 3 WHEN SUM(({nt}) = 2) > 0 THEN 2 WHEN SUM(({nt}) = 1) > 0 THEN 1 END)"
    )
}

impl Compiler<'_> {
    pub(crate) fn pattern_m2(&mut self, p: &GraphPattern) -> Result<Block> {
        match p {
            GraphPattern::LeftJoin {
                left,
                right,
                expression,
            } => self.left_join(left, right, expression.as_ref()),
            GraphPattern::Union { .. } => {
                // Nested unions become one flat N-way UNION ALL (no subquery nesting).
                fn branches<'a>(p: &'a GraphPattern, out: &mut Vec<&'a GraphPattern>) {
                    match p {
                        GraphPattern::Union { left, right } => {
                            branches(left, out);
                            branches(right, out);
                        }
                        other => out.push(other),
                    }
                }
                let mut parts = Vec::new();
                branches(p, &mut parts);
                let blocks = parts
                    .into_iter()
                    .map(|b| self.pattern(b))
                    .collect::<Result<Vec<_>>>()?;
                self.union(blocks)
            }
            GraphPattern::Minus { left, right } => self.minus(left, right),
            GraphPattern::Group {
                inner,
                variables,
                aggregates,
            } => self.group(inner, variables, aggregates),
            GraphPattern::Path {
                subject,
                path,
                object,
            } => self.path(subject, path, object),
            GraphPattern::Service { .. } => Err(Error::unsupported("SERVICE")),
            other => Err(Error::unsupported(format!(
                "graph pattern {}",
                other.to_string().split_whitespace().next().unwrap_or("")
            ))),
        }
    }

    fn left_join(
        &mut self,
        left: &GraphPattern,
        right: &GraphPattern,
        expr: Option<&Expression>,
    ) -> Result<Block> {
        let a = self.pattern(left)?;
        let mut a = self.plain(a);
        self.plan_hint.push(
            a.cols
                .iter()
                .filter(|(_, b)| !b.nullable)
                .map(|(k, _)| *k)
                .collect(),
        );
        let b = self.pattern(right);
        self.plan_hint.pop();
        let mut b = self.plain(b?);
        // Constants computed on the right would survive an unmatched LEFT JOIN: seal them into
        // real columns first.
        if b.from.is_empty() || b.cols.values().any(|x| x.computed) {
            b = self.seal(b);
        }
        if a.from.is_empty() {
            let u = self.alias("u");
            a.from.push(FromItem {
                join: Join::First,
                item: format!("(SELECT 1) AS {u}"),
            });
        }
        let mut on = b.wheres.clone();
        let mut merged = a.cols.clone();
        for (idx, bb) in &b.cols {
            match a.cols.get(idx) {
                None => {
                    merged.insert(
                        *idx,
                        Binding {
                            nullable: true,
                            ..bb.clone()
                        },
                    );
                }
                Some(ab) => {
                    let (cond, m) = Self::unify(ab, bb);
                    on.push(cond);
                    if ab.nullable {
                        merged.insert(
                            *idx,
                            Binding {
                                nullable: true,
                                ..m
                            },
                        );
                    }
                }
            }
        }
        if let Some(e) = expr {
            on.push(self.expr_bool(e, &merged)?);
        }
        let item = if b.from.len() == 1 {
            b.from[0].item.clone()
        } else {
            format!("({})", Block::render_from(&b.from))
        };
        a.from.push(FromItem {
            join: Join::Left(if on.is_empty() {
                "1".into()
            } else {
                on.join(" AND ")
            }),
            item,
        });
        a.cols = merged;
        Ok(a)
    }

    /// UNION ALL of several blocks.
    pub(crate) fn union(&mut self, blocks: Vec<Block>) -> Result<Block> {
        // Branches with solution modifiers (or bare-column aggregates) become subqueries.
        let blocks: Vec<Block> = blocks.into_iter().map(|b| self.plain(b)).collect();
        let mut vars: BTreeMap<usize, (Shape, bool)> = BTreeMap::new();
        for b in &blocks {
            for (idx, bind) in &b.cols {
                let shape = match &bind.col {
                    Col::Id(_) => Shape::Id,
                    Col::Val(_) => Shape::Val,
                };
                let e = vars.entry(*idx).or_insert((shape, bind.nullable));
                if shape == Shape::Val {
                    e.0 = Shape::Val;
                }
                e.1 |= bind.nullable;
            }
        }
        for (idx, (_, nullable)) in &mut vars {
            if blocks.iter().any(|b| !b.cols.contains_key(idx)) {
                *nullable = true;
            }
        }
        let mut branches = Vec::new();
        for b in &blocks {
            let mut sel = Vec::new();
            for (idx, (shape, _)) in &vars {
                match (shape, b.cols.get(idx).map(|x| &x.col)) {
                    (Shape::Id, Some(Col::Id(x))) => sel.push(format!("{x} AS v{idx}")),
                    (Shape::Id, _) => sel.push(format!("NULL AS v{idx}")),
                    (Shape::Val, c) => {
                        let v = match c {
                            Some(c) => c.value(),
                            None => V::null(),
                        };
                        for (suffix, f) in VAL_FIELDS.iter().zip(val_fields(&v)) {
                            sel.push(format!("{f} AS v{idx}_{suffix}"));
                        }
                    }
                }
            }
            if sel.is_empty() {
                sel.push("1 AS _u".into());
            }
            branches.push(format!("SELECT {}{}", sel.join(", "), b.tail()));
        }
        let alias = self.alias("un");
        let mut out = Block::default();
        out.from.push(FromItem {
            join: Join::First,
            item: format!(
                "({}) AS {alias}",
                crate::sql::union_all(branches, self.caps.max_compound_select)
            ),
        });
        for (idx, (shape, nullable)) in vars {
            let col = match shape {
                Shape::Id => Col::Id(format!("{alias}.v{idx}")),
                Shape::Val => {
                    let f = |s: &str| format!("{alias}.v{idx}_{s}");
                    Col::Val(Box::new(V {
                        id: Some(f("i")),
                        kind: f("k"),
                        lex: f("l"),
                        dt: f("d"),
                        lang: f("g"),
                        num: f("n"),
                        nt: f("t"),
                        ts: f("s"),
                        boolv: f("b"),
                        stat: Stat::Any,
                        computed_num: false,
                        decodable: false,
                        aux: f("x"),
                        tz: "NULL".into(),
                    }))
                }
            };
            out.cols.insert(
                idx,
                Binding {
                    col,
                    nullable,
                    computed: false,
                    correlated: false,
                },
            );
        }
        Ok(out)
    }

    fn minus(&mut self, left: &GraphPattern, right: &GraphPattern) -> Result<Block> {
        let a = self.pattern(left)?;
        let mut a = self.plain(a);
        // MINUS does not see outer (EXISTS) bindings.
        let outer = std::mem::take(&mut self.outer);
        let b = self.pattern(right);
        self.outer = outer;
        let mut b = self.plain(b?);
        if b.from.is_empty() {
            b = self.seal(b);
        }
        let mut compat = b.wheres.clone();
        let mut overlap = Vec::new();
        let mut always_overlap = false;
        for (idx, bb) in &b.cols {
            let Some(ab) = a.cols.get(idx) else { continue };
            let (cond, _) = Self::unify(ab, bb);
            compat.push(cond);
            if !ab.nullable && !bb.nullable {
                always_overlap = true;
            } else {
                let not_null = |x: &Binding| match &x.col {
                    Col::Id(i) => format!("{i} IS NOT NULL"),
                    Col::Val(v) => format!("({}) IS NOT NULL", v.kind),
                };
                overlap.push(format!("({} AND {})", not_null(ab), not_null(bb)));
            }
        }
        if !always_overlap && overlap.is_empty() {
            return Ok(a); // disjoint domains: nothing is removed
        }
        if !always_overlap {
            compat.push(format!("({})", overlap.join(" OR ")));
        }
        a.wheres.push(format!(
            "NOT EXISTS (SELECT 1 FROM {} WHERE {})",
            Block::render_from(&b.from),
            compat.join(" AND ")
        ));
        Ok(a)
    }

    fn group(
        &mut self,
        inner: &GraphPattern,
        variables: &[Variable],
        aggregates: &[(Variable, AggregateExpression)],
    ) -> Result<Block> {
        let b = self.pattern(inner)?;
        let mut b = self.plain(b);
        // Aggregate arguments that are expressions are computed once, as columns of an inner
        // subquery: SQL has no common subexpressions, and each aggregate uses its argument's
        // fields several times.
        let mut aggregates: Vec<(Variable, AggregateExpression)> = aggregates.to_vec();
        let mut materialized = false;
        let mut value_of: HashMap<Variable, Variable> = HashMap::new();
        for (_, agg) in &mut aggregates {
            if let AggregateExpression::FunctionCall { name, expr, .. } = agg {
                // A variable read by a value aggregate (SUM, AVG, MIN, MAX…; COUNT only needs
                // ids): its term row is joined once and its fields become inner columns.
                if let Expression::Variable(v) = expr {
                    if matches!(name, AggregateFunction::Count) {
                        continue;
                    }
                    if let Some(h) = value_of.get(v) {
                        *expr = Expression::Variable(h.clone());
                        continue;
                    }
                    let v = v.clone();
                    let restore = self.join_term_vars(&mut b, std::slice::from_ref(&v));
                    let idx = self.var(&v);
                    let joined = b.cols.get(&idx).cloned();
                    Self::restore_terms(&mut b, restore);
                    if let Some(bind) = joined.filter(|j| matches!(j.col, Col::Val(_))) {
                        let hidden = self.fresh_var("val");
                        b.cols.insert(
                            hidden,
                            Binding {
                                col: bind.col,
                                nullable: true,
                                computed: true,
                                correlated: false,
                            },
                        );
                        let h = self.var_names[hidden].clone();
                        value_of.insert(v, h.clone());
                        *expr = Expression::Variable(h);
                        materialized = true;
                    }
                    continue;
                }
                {
                    let restore = self.join_terms(&mut b, expr);
                    let (nb, lifted) = self.shallow(b, expr)?;
                    b = nb;
                    let v = self.expr_term(&lifted, &b.cols)?;
                    Self::restore_terms(&mut b, restore);
                    let hidden = self.fresh_var("agg");
                    let col = match &v.id {
                        Some(id) if v.decodable => Col::Id(id.clone()),
                        _ => Col::Val(Box::new(v)),
                    };
                    b.cols.insert(
                        hidden,
                        Binding {
                            col,
                            nullable: true,
                            computed: true,
                            correlated: false,
                        },
                    );
                    *expr = Expression::Variable(self.var_names[hidden].clone());
                    materialized = true;
                }
            }
        }
        if materialized {
            b.no_flatten = true;
            b = self.seal(b);
        }
        if aggregates
            .iter()
            .any(|(_, a)| matches!(a, AggregateExpression::CountSolutions { distinct: true }))
        {
            if aggregates.len() > 1 {
                return Err(Error::unsupported(
                    "COUNT(DISTINCT *) with other aggregates",
                ));
            }
            b.distinct = true;
            b = self.seal(b);
        }
        let mut group_by = Vec::new();
        let mut cols = BTreeMap::new();
        for v in variables {
            let idx = self.var(v);
            match b.cols.get(&idx) {
                Some(bind) => {
                    match &bind.col {
                        Col::Id(x) => group_by.push(x.clone()),
                        Col::Val(v) => group_by.extend(val_fields(v)),
                    }
                    cols.insert(idx, bind.clone());
                }
                None => {
                    cols.insert(
                        idx,
                        Binding {
                            col: Col::Id("NULL".into()),
                            nullable: true,
                            computed: true,
                            correlated: false,
                        },
                    );
                }
            }
        }
        let extremes = aggregates
            .iter()
            .filter(|(_, a)| {
                matches!(
                    a,
                    AggregateExpression::FunctionCall {
                        name: AggregateFunction::Min | AggregateFunction::Max,
                        ..
                    }
                )
            })
            .count();
        if extremes > 1 {
            return Err(Error::unsupported(
                "several MIN/MAX aggregates in one group",
            ));
        }
        for (var, agg) in &aggregates {
            let bind = self.aggregate(agg, &b.cols, &mut b.extra_select)?;
            let idx = self.var(var);
            cols.insert(idx, bind);
        }
        b.cols = cols;
        b.group_by = Some(group_by);
        b.stage = Stage::Grouped;
        Ok(b)
    }

    fn aggregate(
        &mut self,
        agg: &AggregateExpression,
        cols: &BTreeMap<usize, Binding>,
        extra: &mut Vec<String>,
    ) -> Result<Binding> {
        let computed = |col: Col| Binding {
            col,
            nullable: true,
            computed: true,
            correlated: false,
        };
        Ok(match agg {
            AggregateExpression::CountSolutions { .. } => {
                computed(Col::Id(format!("({INT_BASE} + COUNT(*))")))
            }
            AggregateExpression::FunctionCall {
                name,
                expr,
                distinct,
            } => {
                let v = self.expr_term(expr, cols)?;
                match name {
                    AggregateFunction::Count => {
                        let key = match (&v.id, v.decodable) {
                            (Some(id), true) => id.clone(),
                            _ => format!(
                                "(({}) || '|' || COALESCE({}, '') || '|' || COALESCE({}, '') || '|' || COALESCE({}, ''))",
                                v.kind, v.dt, v.lang, v.lex
                            ),
                        };
                        let count = if *distinct {
                            format!("COUNT(DISTINCT {key})")
                        } else {
                            format!("COUNT({})", v.kind)
                        };
                        computed(Col::Id(format!("({INT_BASE} + {count})")))
                    }
                    AggregateFunction::Sum | AggregateFunction::Avg => {
                        if *distinct {
                            return Err(Error::unsupported("SUM/AVG DISTINCT"));
                        }
                        let all_numeric = format!("COUNT({}) = COUNT({})", v.kind, v.num);
                        let rank = max_rank(&v.nt);
                        let (num, nt) = if matches!(name, AggregateFunction::Sum) {
                            (
                                format!(
                                    "(CASE WHEN {all_numeric} THEN COALESCE(SUM({}), 0) END)",
                                    v.num
                                ),
                                format!("COALESCE({rank}, 1)"),
                            )
                        } else {
                            (
                                format!(
                                    "(CASE WHEN NOT ({all_numeric}) THEN NULL WHEN COUNT({n}) = 0 THEN 0 ELSE AVG({n}) END)",
                                    n = v.num
                                ),
                                format!("(CASE WHEN COUNT({}) = 0 THEN 1 WHEN {rank} < 2 THEN 2 ELSE {rank} END)", v.num),
                            )
                        };
                        let mut out = V::numeric(num, nt);
                        if matches!(name, AggregateFunction::Avg) {
                            // Integer averages are divided exactly by the decoder, with
                            // Oxigraph's decimal precision.
                            out.aux = format!(
                                "(CASE WHEN {rank} = 1 AND COUNT({n}) > 0 THEN 'avg:' || CAST(SUM({n}) AS TEXT) || '/' || COUNT({n}) END)",
                                n = v.num
                            );
                        }
                        computed(Col::Val(Box::new(out)))
                    }
                    AggregateFunction::Min | AggregateFunction::Max => {
                        // SQLite bare-column rule: with exactly one min()/max() aggregate, bare
                        // columns come from the row holding the extreme value, so the actual
                        // stored term (with its lexical form) is returned.
                        let f = if matches!(name, AggregateFunction::Min) {
                            "MIN"
                        } else {
                            "MAX"
                        };
                        extra.push(format!("{f}({}) AS _extreme", sort_key(&v)));
                        match (&v.id, v.decodable) {
                            (Some(id), true) => computed(Col::Id(id.clone())),
                            _ => computed(Col::Val(Box::new(v))),
                        }
                    }
                    AggregateFunction::Sample => match (&v.id, v.decodable) {
                        (Some(id), true) => computed(Col::Id(id.clone())),
                        _ => computed(Col::Val(Box::new(v))),
                    },
                    AggregateFunction::GroupConcat { separator } => {
                        if *distinct {
                            return Err(Error::unsupported("GROUP_CONCAT DISTINCT"));
                        }
                        let sep = crate::sql::sql_str(separator.as_deref().unwrap_or(" "));
                        let mut out = V::null();
                        out.id = None;
                        out.decodable = false;
                        out.lex = format!("COALESCE(group_concat({}, {sep}), '')", v.lex);
                        out.kind = format!("{}", super::expr::K_STRING);
                        out.dt = crate::sql::sql_str(oxrdf::vocab::xsd::STRING.as_str());
                        out.stat = Stat::String;
                        computed(Col::Val(Box::new(out)))
                    }
                    AggregateFunction::Custom(n) => {
                        return Err(Error::unsupported(format!("custom aggregate {n}")))
                    }
                }
            }
        })
    }

    fn hidden(&mut self, hint: &str) -> TermPattern {
        let idx = self.fresh_var(hint);
        TermPattern::Variable(self.var_names[idx].clone())
    }

    /// Compiles a property path pattern.
    fn path(
        &mut self,
        s: &TermPattern,
        path: &PropertyPathExpression,
        o: &TermPattern,
    ) -> Result<Block> {
        match path {
            PropertyPathExpression::NamedNode(p) => self.pattern(&GraphPattern::Bgp {
                patterns: vec![TriplePattern {
                    subject: s.clone(),
                    predicate: NamedNodePattern::NamedNode(p.clone()),
                    object: o.clone(),
                }],
            }),
            PropertyPathExpression::Reverse(inner) => self.path(o, inner, s),
            PropertyPathExpression::Sequence(a, b) => {
                let m = self.hidden("m");
                let l = self.path(s, a, &m)?;
                let r = self.path(&m, b, o)?;
                self.join(l, r)
            }
            PropertyPathExpression::Alternative(a, b) => {
                let l = self.path(s, a, o)?;
                let r = self.path(s, b, o)?;
                self.union(vec![l, r])
            }
            PropertyPathExpression::NegatedPropertySet(ps) => {
                let s = self.path_pos(s)?;
                let o = self.path_pos(o)?;
                let pv = self.fresh_var("np");
                let mut b = Block::default();
                let q = self.quad_access(&mut b, Join::Cross, s, Pos::Var(pv), o)?;
                b.cols.remove(&pv);
                if !ps.is_empty() {
                    let ids: Vec<String> = ps
                        .iter()
                        .map(|p| self.constant_id(&p.clone().into()).map(|i| i.to_string()))
                        .collect::<Result<_>>()?;
                    b.wheres.push(format!("{q}.p NOT IN ({})", ids.join(",")));
                }
                Ok(b)
            }
            PropertyPathExpression::ZeroOrMore(inner) => {
                self.closure(s, inner, o, Closure::ZeroOrMore, None)
            }
            PropertyPathExpression::OneOrMore(inner) => {
                self.closure(s, inner, o, Closure::OneOrMore, None)
            }
            PropertyPathExpression::ZeroOrOne(inner) => {
                self.closure(s, inner, o, Closure::ZeroOrOne, None)
            }
        }
    }

    fn path_pos(&mut self, t: &TermPattern) -> Result<Pos> {
        let mut bnodes = std::collections::HashMap::new();
        self.pos(t, &mut bnodes)
    }

    /// SELECT of all nodes (subjects and objects) of the active graph(s), with the graph id.
    fn node_set(&mut self, gv: Option<usize>) -> Result<String> {
        let (a, b) = (self.fresh_var("ns"), self.fresh_var("no"));
        let pv = self.fresh_var("np");
        let mut blk = Block::default();
        let q = self.quad_access(
            &mut blk,
            Join::Cross,
            Pos::Var(a),
            Pos::Var(pv),
            Pos::Var(b),
        )?;
        let g = if gv.is_some() {
            format!("{q}.g")
        } else {
            "0".into()
        };
        let w = if blk.wheres.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", blk.wheres.join(" AND "))
        };
        let item = &blk.from[0].item; // `quads q`, or the entailed-triple source
        Ok(format!(
            "SELECT {q}.s AS n, {g} AS g FROM {item}{w} UNION SELECT {q}.o, {g} FROM {item}{w}"
        ))
    }

    /// Graph ids a `GRAPH ?g` pattern ranges over.
    fn graph_ids(&mut self) -> String {
        let v = self.fresh_var("gi");
        let b = self.graph_list_block(v);
        let key = b
            .cols
            .get(&v)
            .and_then(|x| x.col.key().map(str::to_string))
            .unwrap_or_default();
        format!("SELECT {key} AS g{}", b.tail())
    }

    /// `*`, `+` and `?` paths as recursive CTEs over the base relation (s, o, g). The graph
    /// column keeps each walk inside one graph under `GRAPH ?g`.
    /// A recursive path joined with a block that binds one of its endpoints: walk only from
    /// the nodes that block produces (magic-set style) instead of the whole closure.
    pub(crate) fn seeded_path(
        &mut self,
        left: &Block,
        s: &TermPattern,
        path: &PropertyPathExpression,
        o: &TermPattern,
    ) -> Result<Option<Block>> {
        let (inner, kind) = match path {
            PropertyPathExpression::ZeroOrMore(i) => (i, Closure::ZeroOrMore),
            PropertyPathExpression::OneOrMore(i) => (i, Closure::OneOrMore),
            _ => return Ok(None),
        };
        if !left.is_plain() || left.from.is_empty() {
            return Ok(None);
        }
        let bound = |t: &TermPattern, me: &mut Self| -> Option<String> {
            let TermPattern::Variable(v) = t else {
                return None;
            };
            let b = left.cols.get(&me.var(v))?;
            if b.nullable {
                return None;
            }
            match &b.col {
                Col::Id(k) => Some(k.clone()),
                Col::Val(_) => None,
            }
        };
        let constant =
            |t: &TermPattern| !matches!(t, TermPattern::Variable(_) | TermPattern::BlankNode(_));
        if constant(s) || constant(o) {
            return Ok(None); // constant seeding is already optimal
        }
        let seed = if let Some(k) = bound(s, self) {
            (format!("SELECT DISTINCT {k} AS n{}", left.tail()), true)
        } else if let Some(k) = bound(o, self) {
            (format!("SELECT DISTINCT {k} AS n{}", left.tail()), false)
        } else {
            return Ok(None);
        };
        self.closure(s, inner, o, kind, Some(seed)).map(Some)
    }

    fn closure(
        &mut self,
        s: &TermPattern,
        inner: &PropertyPathExpression,
        o: &TermPattern,
        kind: Closure,
        seed: Option<(String, bool)>,
    ) -> Result<Block> {
        let gv = match self.scope {
            GraphScope::Var(v) => Some(v),
            _ => None,
        };
        let (a, b) = (self.hidden("pa"), self.hidden("pb"));
        // Walks are per graph: compile the base with its own graph variable.
        let saved = self.scope;
        let hg = gv.map(|_| self.fresh_var("pg"));
        if let Some(h) = hg {
            self.scope = GraphScope::Var(h);
        }
        let base = self.path(&a, inner, &b);
        self.scope = saved;
        let base = self.plain(base?);
        let (ai, bi) = match (&a, &b) {
            (TermPattern::Variable(x), TermPattern::Variable(y)) => (self.var(x), self.var(y)),
            _ => unreachable!(),
        };
        let key = |i: usize| {
            base.cols
                .get(&i)
                .and_then(|x| x.col.key().map(str::to_string))
        };
        let (Some(ak), Some(bk)) = (key(ai), key(bi)) else {
            return Err(Error::unsupported("property path over computed values"));
        };
        let gk = match hg {
            Some(h) => {
                key(h).ok_or_else(|| Error::unsupported("property path without graph binding"))?
            }
            None => "0".into(),
        };
        let base_sql = format!("SELECT {ak} AS s, {bk} AS o, {gk} AS g{}", base.tail());
        let sp = self.path_pos(s)?;
        let op = self.path_pos(o)?;
        let zero = kind != Closure::OneOrMore;
        let rec = kind != Closure::ZeroOrOne;
        let graphs = if gv.is_some() {
            format!("({})", self.graph_ids())
        } else {
            "(SELECT 0 AS g)".into()
        };
        let sub = match (sp, op, seed) {
            (_, _, Some((seed_sql, forward))) => {
                // r(st, n, g): walks starting at each seed node.
                let (from, to) = if forward { ("s", "o") } else { ("o", "s") };
                let mut cte = format!(
                    "SELECT sd.n AS st, b.{to} AS n, b.g AS g FROM ({seed_sql}) AS sd JOIN ({base_sql}) AS b ON b.{from} = sd.n"
                );
                if zero {
                    cte = format!("SELECT sd.n, sd.n, gs.g FROM ({seed_sql}) AS sd CROSS JOIN {graphs} AS gs UNION {cte}");
                }
                cte = format!("{cte} UNION SELECT r.st, b.{to}, b.g FROM r JOIN ({base_sql}) AS b ON b.{from} = r.n AND b.g = r.g");
                let (so, oo) = if forward { ("st", "n") } else { ("n", "st") };
                format!(
                    "(WITH RECURSIVE r(st, n, g) AS ({cte}) SELECT {so} AS s, {oo} AS o, g FROM r)"
                )
            }
            (Pos::Const(sid), _, None) => {
                let mut cte = format!("SELECT o AS n, g FROM ({base_sql}) WHERE s = {sid}");
                if zero {
                    cte = format!("SELECT {sid} AS n, g FROM {graphs} UNION {cte}");
                }
                if rec {
                    cte = format!("{cte} UNION SELECT b.o, b.g FROM r JOIN ({base_sql}) AS b ON b.s = r.n AND b.g = r.g");
                }
                format!("(WITH RECURSIVE r(n, g) AS ({cte}) SELECT {sid} AS s, n AS o, g FROM r)")
            }
            (_, Pos::Const(oid), None) => {
                let mut cte = format!("SELECT s AS n, g FROM ({base_sql}) WHERE o = {oid}");
                if zero {
                    cte = format!("SELECT {oid} AS n, g FROM {graphs} UNION {cte}");
                }
                if rec {
                    cte = format!("{cte} UNION SELECT b.s, b.g FROM r JOIN ({base_sql}) AS b ON b.o = r.n AND b.g = r.g");
                }
                format!("(WITH RECURSIVE r(n, g) AS ({cte}) SELECT n AS s, {oid} AS o, g FROM r)")
            }
            (_, _, None) => {
                self.notes.push(
                    "warning: property path with no bound endpoint computes the whole closure"
                        .into(),
                );
                let mut cte = format!("SELECT s, o, g FROM ({base_sql})");
                if rec {
                    cte = format!("{cte} UNION SELECT r.s, b.o, r.g FROM r JOIN ({base_sql}) AS b ON b.s = r.o AND b.g = r.g");
                }
                if zero {
                    let saved = self.scope;
                    if let Some(h) = hg {
                        self.scope = GraphScope::Var(h);
                    }
                    let nodes = self.node_set(gv);
                    self.scope = saved;
                    cte = format!("SELECT n AS s, n AS o, g FROM ({}) UNION {cte}", nodes?);
                }
                format!("(WITH RECURSIVE r(s, o, g) AS ({cte}) SELECT s, o, g FROM r)")
            }
        };
        let alias = self.alias("pp");
        let mut blk = Block::default();
        blk.from.push(FromItem {
            join: Join::First,
            item: format!("{sub} AS {alias}"),
        });
        self.bind_pos(&mut blk, &format!("{alias}.s"), sp)?;
        self.bind_pos(&mut blk, &format!("{alias}.o"), op)?;
        if let Some(g) = gv {
            self.bind_pos(&mut blk, &format!("{alias}.g"), Pos::Var(g))?;
        }
        Ok(blk)
    }
}
