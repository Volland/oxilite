//! Query planning: which clauses become SQL and which run in Rust.
//!
//! Reading clauses (`MATCH`, `OPTIONAL MATCH`, `WITH`, `UNWIND` of constant lists) are
//! lowered to one SPARQL query, compiled to SQL. The first clause that SQL cannot express —
//! a write, or a projection over lists, maps or `collect()` — starts the *tail*, executed
//! by the Rust interpreter over the rows of that query. `MERGE` look-ups are lowered into
//! the SQL part as optional matches, so a whole statement reads with a single query.
//!
// @lat: [[architecture#Property graph frontend]]

use crate::ast::*;
use crate::error::{CypherError, Result};
use crate::lower::{join, unit, Bind, Lowerer, Scope, Stage};
use crate::value::Value;
use oxrdf::{Literal, Variable};
use spargebra::algebra::{Expression, Function, GraphPattern};
use spargebra::term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub(crate) enum TailOp {
    /// `WITH`, `UNWIND`, `RETURN`, `CREATE`, `SET`, `REMOVE`, `DELETE` evaluated in Rust.
    Clause(Clause),
    /// `MERGE`: the SQL part looked the pattern up (optionally) unless it depends on
    /// variables created in Rust.
    Merge {
        part: PatternPart,
        on_create: Vec<SetItem>,
        on_match: Vec<SetItem>,
        /// The row variable that is `true` when the SQL look-up matched.
        lookup: Option<String>,
    },
    /// Final columns, already computed by SQL.
    Output(Vec<String>),
}

/// The plan of one single query (a `UNION` member).
#[derive(Debug, Clone)]
pub(crate) struct PartPlan {
    pub query: Option<spargebra::Query>,
    pub scope: Scope,
    pub tail: Vec<TailOp>,
    pub columns: Vec<String>,
    pub shortest: Vec<crate::lower::ShortestSpec>,
    pub procedure: Option<Procedure>,
    /// Value types of SPARQL variables (from the schema), for the compiler.
    pub var_types: BTreeMap<String, oxilite_core::ValueType>,
    /// Pattern comprehensions of the projection evaluated in Rust.
    pub comprehensions: Vec<Comprehension>,
}

/// A pattern comprehension `[p = (a)-->(b) WHERE … | expr]`, run as its own query: the
/// pattern joined with the distinct values of the outer variables it reads. The job keys its
/// rows by those values and gives each row of the main query its list.
#[derive(Debug, Clone)]
pub(crate) struct Comprehension {
    /// The row variable that receives the list.
    pub name: String,
    pub query: spargebra::Query,
    /// Bindings of the comprehension's rows: the outer keys and the pattern's variables.
    pub scope: Scope,
    /// SPARQL variables of the outer values.
    pub keys: Vec<Variable>,
    /// Nodes of an enclosing list comprehension: their SPARQL variables and names. With
    /// them, the row gets a map from their ids (`|`-separated) to lists.
    pub free: Vec<Variable>,
    pub free_names: Vec<String>,
    /// A `WHERE` that SQL cannot express, evaluated in Rust.
    pub filter: Option<Expr>,
    pub map: Expr,
}

#[derive(Debug, Clone)]
pub(crate) struct Procedure {
    #[allow(dead_code)]
    pub name: String,
    pub yields: Vec<(String, String)>,
    pub where_: Option<Expr>,
}

#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub parts: Vec<PartPlan>,
    pub union_all: Vec<bool>,
    pub columns: Vec<String>,
    pub writes: bool,
    pub notes: Vec<String>,
}

pub(crate) fn plan(q: &Query, lw: &mut Lowerer<'_>) -> Result<Plan> {
    let mut parts = Vec::new();
    let mut writes = false;
    for sq in &q.parts {
        writes |= sq.clauses.iter().any(Clause::is_write);
        parts.push(plan_single(sq, lw)?);
    }
    let columns = parts[0].columns.clone();
    for p in &parts[1..] {
        if p.columns != columns {
            return Err(CypherError::semantic(
                "all parts of a UNION must return the same columns",
            ));
        }
    }
    Ok(Plan {
        parts,
        union_all: q.union_all.clone(),
        columns,
        writes,
        notes: std::mem::take(&mut lw.notes),
    })
}

fn is_unsupported(e: &CypherError) -> bool {
    matches!(e, CypherError::Unsupported(_))
}

fn plan_single(sq: &SingleQuery, lw: &mut Lowerer<'_>) -> Result<PartPlan> {
    let mut stage = Stage::new();
    let mut in_sql = true;
    let mut tail: Vec<TailOp> = Vec::new();
    let mut columns = Vec::new();
    let mut shortest = Vec::new();
    let mut procedure = None;
    let mut comprehensions = Vec::new();
    // Variables introduced by tail clauses.
    let mut tail_vars: BTreeSet<String> = BTreeSet::new();
    let mut tail_reshaped = false;
    let n = sq.clauses.len();
    for (i, clause) in sq.clauses.iter().enumerate() {
        let last = i + 1 == n;
        if let Clause::Merge {
            pattern,
            on_create,
            on_match,
        } = clause
        {
            let vars = pattern_vars(pattern);
            let mut uses_tail = vars.iter().any(|v| tail_vars.contains(v));
            for np in std::iter::once(&pattern.element.start)
                .chain(pattern.element.chain.iter().map(|(_, n)| n))
            {
                if let Some(props) = &np.props {
                    props.walk(&mut |x| {
                        if let Expr::Var(v) = x {
                            if tail_vars.contains(v) {
                                uses_tail = true;
                            }
                        }
                    });
                }
            }
            let lookup = if uses_tail {
                None
            } else if tail_reshaped {
                return Err(CypherError::unsupported(
                    "MERGE after a WITH/UNWIND evaluated in Rust",
                ));
            } else {
                let mut right = stage.child();
                let mut m =
                    lw.match_patterns(std::slice::from_ref(pattern), &mut right, true, false)?;
                // A marker bound only when the pattern matched.
                let var = lw.fresh("merged");
                m.inner = GraphPattern::Extend {
                    inner: Box::new(m.inner),
                    variable: var.clone(),
                    expression: Expression::Literal(Literal::from(true)),
                };
                stage.pattern =
                    m.left_join_into(std::mem::replace(&mut stage.pattern, unit()), None);
                stage.scope = right.scope;
                let name = format!("\u{1}merge{i}");
                stage.scope.insert(
                    name.clone(),
                    Bind::Value {
                        var,
                        nullable: true,
                        name: false,
                    },
                );
                Some(name)
            };
            tail_vars.extend(vars);
            in_sql = false;
            tail.push(TailOp::Merge {
                part: pattern.clone(),
                on_create: on_create.clone(),
                on_match: on_match.clone(),
                lookup,
            });
            continue;
        }
        if !in_sql {
            match clause {
                Clause::Match { .. } => {
                    return Err(CypherError::unsupported(
                        "MATCH after a write or a clause evaluated in Rust",
                    ))
                }
                Clause::Call { .. } => {
                    return Err(CypherError::unsupported("CALL after other clauses"))
                }
                Clause::Return(p) => {
                    columns = projection_names(p, &stage.scope, &tail_vars);
                    tail.push(TailOp::Clause(clause.clone()));
                }
                Clause::With(p) => {
                    tail_reshaped = true;
                    tail_vars.extend(projection_names(p, &stage.scope, &tail_vars));
                    tail.push(TailOp::Clause(clause.clone()));
                }
                Clause::Unwind { alias, .. } => {
                    tail_reshaped = true;
                    tail_vars.insert(alias.clone());
                    tail.push(TailOp::Clause(clause.clone()));
                }
                Clause::Create(parts) => {
                    for p in parts {
                        tail_vars.extend(pattern_vars(p));
                    }
                    tail.push(TailOp::Clause(clause.clone()));
                }
                _ => tail.push(TailOp::Clause(clause.clone())),
            }
            continue;
        }
        match clause {
            Clause::Match {
                optional,
                patterns,
                where_,
            } => {
                if *optional {
                    let mut right = stage.child();
                    let mut m = lw.match_patterns(patterns, &mut right, true, false)?;
                    // WHERE of OPTIONAL MATCH is part of the optional pattern; its property
                    // reads join inside it.
                    right.pattern = std::mem::replace(&mut m.inner, unit());
                    let expression = match where_ {
                        Some(w) => Some(lw.expr(w, &mut right)?),
                        None => None,
                    };
                    m.inner = right.pattern;
                    stage.pattern =
                        m.left_join_into(std::mem::replace(&mut stage.pattern, unit()), expression);
                    stage.scope = right.scope;
                    for (k, v) in right.labels {
                        stage.labels.entry(k).or_insert(v);
                    }
                } else {
                    let m = lw.match_patterns(patterns, &mut stage, false, false)?;
                    stage.pattern = m.join_into(std::mem::replace(&mut stage.pattern, unit()));
                    // A WHERE over a shortest path is evaluated in Rust, after the search.
                    let rust_where = where_.as_ref().filter(|w| {
                        lw.shortest.iter().any(|sp| {
                            mentions_var(w, &sp.path)
                                || sp.rel.as_ref().is_some_and(|r| mentions_var(w, r))
                        })
                    });
                    if let Some(w) = rust_where {
                        tail.push(TailOp::Clause(Clause::With(Projection {
                            distinct: false,
                            star: true,
                            items: Vec::new(),
                            order: Vec::new(),
                            skip: None,
                            limit: None,
                            where_: Some(w.clone()),
                        })));
                    } else if let Some(w) = where_ {
                        let f = lw.expr(w, &mut stage)?;
                        stage.pattern = GraphPattern::Filter {
                            expr: f,
                            inner: Box::new(std::mem::replace(&mut stage.pattern, unit())),
                        };
                    }
                }
                if !lw.shortest.is_empty() {
                    // Shortest paths are searched in Rust right after this MATCH.
                    shortest = std::mem::take(&mut lw.shortest);
                    in_sql = false;
                }
            }
            Clause::Unwind { expr, alias } => match lw.constant(expr)? {
                Some(v) if unwind_values(v.clone(), alias, &mut stage.clone(), lw).is_ok() => {
                    unwind_values(v, alias, &mut stage, lw)?;
                }
                _ => {
                    in_sql = false;
                    tail_reshaped = true;
                    tail_vars.insert(alias.clone());
                    tail.push(TailOp::Clause(clause.clone()));
                }
            },
            Clause::With(p) => match lw.projection(p, stage.clone(), false) {
                Ok((s, _)) => stage = s,
                Err(e) if is_unsupported(&e) => {
                    in_sql = false;
                    tail_reshaped = true;
                    tail_vars.extend(projection_names(p, &stage.scope, &tail_vars));
                    let p = hoist_patterns(p, &mut stage, lw, &mut comprehensions)?;
                    tail.push(TailOp::Clause(Clause::With(p)));
                }
                Err(e) => return Err(e),
            },
            Clause::Return(p) => {
                columns = projection_names(p, &stage.scope, &tail_vars);
                let aggregating = p.distinct || p.items.iter().any(|it| it.expr.has_aggregate());
                if aggregating {
                    match lw.projection(p, stage.clone(), true) {
                        Ok((s, names)) => {
                            stage = s;
                            tail.push(TailOp::Output(names));
                        }
                        Err(e) if is_unsupported(&e) => {
                            lw.notes.push(format!("RETURN evaluated in Rust: {e}"));
                            let p = hoist_patterns(p, &mut stage, lw, &mut comprehensions)?;
                            tail.push(TailOp::Clause(Clause::Return(p)));
                        }
                        Err(e) => return Err(e),
                    }
                } else {
                    // Items are evaluated in Rust (list semantics for multi-valued
                    // properties); ordering and paging stay in SQL.
                    match sort_in_sql(p, &mut stage, lw) {
                        Ok(true) => {
                            let mut p2 = hoist_patterns(p, &mut stage, lw, &mut comprehensions)?;
                            p2.order.clear();
                            p2.skip = None;
                            p2.limit = None;
                            tail.push(TailOp::Clause(Clause::Return(p2)));
                        }
                        Ok(false) => {
                            let p2 = hoist_patterns(p, &mut stage, lw, &mut comprehensions)?;
                            tail.push(TailOp::Clause(Clause::Return(p2)));
                        }
                        Err(e) => return Err(e),
                    }
                }
                in_sql = false;
            }
            Clause::Call {
                procedure: name,
                args,
                yields,
                where_,
            } => {
                if i != 0 {
                    return Err(CypherError::unsupported("CALL after other clauses"));
                }
                let (q, cols) = crate::schema::procedure_pattern(name, args, lw)?;
                let yields: Vec<(String, String)> = match yields {
                    None => cols.iter().map(|c| (c.clone(), c.clone())).collect(),
                    Some(items) => {
                        let mut out = Vec::new();
                        for (col, alias) in items {
                            if !cols.contains(col) {
                                return Err(CypherError::semantic(format!(
                                    "procedure {name} has no column `{col}`"
                                )));
                            }
                            out.push((col.clone(), alias.clone().unwrap_or_else(|| col.clone())));
                        }
                        out
                    }
                };
                for (_, alias) in &yields {
                    tail_vars.insert(alias.clone());
                }
                if last {
                    columns = yields.iter().map(|(_, a)| a.clone()).collect();
                    tail.push(TailOp::Output(columns.clone()));
                }
                stage.pattern = q;
                procedure = Some(Procedure {
                    name: name.clone(),
                    yields,
                    where_: where_.clone(),
                });
                in_sql = false;
            }
            c if c.is_write() => {
                in_sql = false;
                if let Clause::Create(parts) = c {
                    for p in parts {
                        tail_vars.extend(pattern_vars(p));
                    }
                }
                tail.push(TailOp::Clause(c.clone()));
            }
            _ => unreachable!("all clauses handled"),
        }
    }
    let query = if procedure.is_some() {
        Some(select(
            stage.pattern.clone(),
            procedure_vars(&stage.pattern),
        ))
    } else if matches!(&stage.pattern, GraphPattern::Bgp { patterns } if patterns.is_empty())
        && stage.scope.is_empty()
    {
        None
    } else {
        let vars: Vec<Variable> = stage.scope.values().flat_map(Bind::vars).collect();
        let mut seen = BTreeSet::new();
        let vars: Vec<Variable> = vars
            .into_iter()
            .filter(|v| seen.insert(v.clone()))
            .collect();
        let pattern = match &stage.pattern {
            GraphPattern::Project { .. }
            | GraphPattern::Distinct { .. }
            | GraphPattern::Slice { .. } => stage.pattern.clone(),
            p => GraphPattern::Project {
                inner: Box::new(p.clone()),
                variables: vars.clone(),
            },
        };
        Some(select(pattern, vars))
    };
    Ok(PartPlan {
        query,
        scope: stage.scope,
        tail,
        columns,
        shortest,
        procedure,
        var_types: std::mem::take(&mut lw.var_types),
        comprehensions,
    })
}

fn select(pattern: GraphPattern, vars: Vec<Variable>) -> spargebra::Query {
    let pattern = match pattern {
        p @ (GraphPattern::Project { .. }
        | GraphPattern::Distinct { .. }
        | GraphPattern::Slice { .. }) => p,
        p => GraphPattern::Project {
            inner: Box::new(p),
            variables: vars,
        },
    };
    spargebra::Query::Select {
        dataset: None,
        pattern,
        base_iri: None,
    }
}

fn procedure_vars(p: &GraphPattern) -> Vec<Variable> {
    match p {
        GraphPattern::Project { variables, .. } => variables.clone(),
        GraphPattern::Distinct { inner } | GraphPattern::Slice { inner, .. } => {
            procedure_vars(inner)
        }
        _ => Vec::new(),
    }
}

/// Orders and pages in SQL for a plain `RETURN`; `false` when ordering needs Rust.
fn sort_in_sql(p: &Projection, stage: &mut Stage, lw: &mut Lowerer<'_>) -> Result<bool> {
    if p.order.is_empty() && p.skip.is_none() && p.limit.is_none() {
        return Ok(true);
    }
    // Order keys may name RETURN aliases: substitute the aliased expressions.
    let aliases: BTreeMap<String, Expr> = p
        .items
        .iter()
        .filter_map(|it| it.alias.clone().map(|a| (a, it.expr.clone())))
        .collect();
    let order: Vec<(Expr, bool)> = p
        .order
        .iter()
        .map(|(e, asc)| (substitute(e, &aliases), *asc))
        .collect();
    let sort = Projection {
        distinct: false,
        star: true,
        items: Vec::new(),
        order,
        skip: p.skip.clone(),
        limit: p.limit.clone(),
        where_: None,
    };
    match lw.projection(&sort, stage.clone(), false) {
        Ok((s, _)) => {
            *stage = s;
            Ok(true)
        }
        Err(e) if is_unsupported(&e) => Ok(false),
        Err(e) => Err(e),
    }
}

fn substitute(e: &Expr, aliases: &BTreeMap<String, Expr>) -> Expr {
    e.map_vars(&|v| aliases.get(v).cloned())
}

/// Output column names of a projection.
pub(crate) fn projection_names(
    p: &Projection,
    scope: &Scope,
    tail_vars: &BTreeSet<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    if p.star {
        let mut names: BTreeSet<String> = scope.keys().cloned().collect();
        names.extend(tail_vars.iter().cloned());
        out.extend(names.into_iter().filter(|n| !n.starts_with('\u{1}')));
    }
    out.extend(p.items.iter().map(ProjectionItem::name));
    out
}

/// Replaces pattern predicates (`(a)-->()`, `EXISTS { … }`) in a projection evaluated in
/// Rust with variables computed by the SQL part.
fn hoist_patterns(
    p: &Projection,
    stage: &mut Stage,
    lw: &mut Lowerer<'_>,
    comps: &mut Vec<Comprehension>,
) -> Result<Projection> {
    fn go(
        e: &Expr,
        stage: &mut Stage,
        lw: &mut Lowerer<'_>,
        comps: &mut Vec<Comprehension>,
        locals: &BTreeSet<String>,
    ) -> Result<Expr> {
        Ok(match e {
            Expr::PatternComp {
                var,
                pattern,
                filter,
                map,
            } => {
                let name = format!("\u{1}comprehension{}", comps.len());
                let c = comprehension(
                    var,
                    pattern,
                    filter.as_deref(),
                    map,
                    stage,
                    lw,
                    name.clone(),
                    locals,
                )?;
                let free = c.free_names.clone();
                comps.push(c);
                if free.is_empty() {
                    Expr::Var(name)
                } else {
                    // Nodes of an enclosing list comprehension: look the list up by their ids.
                    let id = |n: &String| Expr::Func {
                        name: "elementid".into(),
                        distinct: false,
                        args: vec![Expr::Var(n.clone())],
                    };
                    let mut key = id(&free[0]);
                    for n in &free[1..] {
                        key = Expr::Binary(
                            BinOp::Add,
                            Box::new(Expr::Binary(
                                BinOp::Add,
                                Box::new(key),
                                Box::new(Expr::Str("|".into())),
                            )),
                            Box::new(id(n)),
                        );
                    }
                    Expr::Func {
                        name: "coalesce".into(),
                        distinct: false,
                        args: vec![
                            Expr::Index(Box::new(Expr::Var(name)), Box::new(key)),
                            Expr::List(Vec::new()),
                        ],
                    }
                }
            }
            Expr::ListComp {
                var,
                list,
                filter,
                map,
            } => {
                let mut inner = locals.clone();
                inner.insert(var.clone());
                Expr::ListComp {
                    var: var.clone(),
                    list: Box::new(go(list, stage, lw, comps, locals)?),
                    filter: match filter {
                        Some(f) => Some(Box::new(go(f, stage, lw, comps, &inner)?)),
                        None => None,
                    },
                    map: match map {
                        Some(m) => Some(Box::new(go(m, stage, lw, comps, &inner)?)),
                        None => None,
                    },
                }
            }
            Expr::Quantified { q, var, list, pred } => {
                let mut inner = locals.clone();
                inner.insert(var.clone());
                Expr::Quantified {
                    q: *q,
                    var: var.clone(),
                    list: Box::new(go(list, stage, lw, comps, locals)?),
                    pred: Box::new(go(pred, stage, lw, comps, &inner)?),
                }
            }
            Expr::Pattern(_) | Expr::Exists(..) => {
                let x = lw.expr(e, stage)?;
                let v = lw.fresh("pattern");
                stage.pattern = GraphPattern::Extend {
                    inner: Box::new(std::mem::replace(&mut stage.pattern, unit())),
                    variable: v.clone(),
                    expression: x,
                };
                let name = format!("\u{1}{}", v.as_str());
                stage.scope.insert(
                    name.clone(),
                    Bind::Value {
                        var: v,
                        nullable: true,
                        name: false,
                    },
                );
                Expr::Var(name)
            }
            Expr::Unary(op, a) => Expr::Unary(*op, Box::new(go(a, stage, lw, comps, locals)?)),
            Expr::Binary(op, a, b) => Expr::Binary(
                *op,
                Box::new(go(a, stage, lw, comps, locals)?),
                Box::new(go(b, stage, lw, comps, locals)?),
            ),
            Expr::Func {
                name,
                distinct,
                args,
            } => Expr::Func {
                name: name.clone(),
                distinct: *distinct,
                args: args
                    .iter()
                    .map(|a| go(a, stage, lw, comps, locals))
                    .collect::<Result<_>>()?,
            },
            Expr::List(items) => Expr::List(
                items
                    .iter()
                    .map(|a| go(a, stage, lw, comps, locals))
                    .collect::<Result<_>>()?,
            ),
            Expr::Case {
                operand,
                whens,
                else_,
            } => Expr::Case {
                operand: match operand {
                    Some(o) => Some(Box::new(go(o, stage, lw, comps, locals)?)),
                    None => None,
                },
                whens: whens
                    .iter()
                    .map(|(w, t)| {
                        Ok((
                            go(w, stage, lw, comps, locals)?,
                            go(t, stage, lw, comps, locals)?,
                        ))
                    })
                    .collect::<Result<_>>()?,
                else_: match else_ {
                    Some(o) => Some(Box::new(go(o, stage, lw, comps, locals)?)),
                    None => None,
                },
            },
            Expr::Index(a, b) => Expr::Index(
                Box::new(go(a, stage, lw, comps, locals)?),
                Box::new(go(b, stage, lw, comps, locals)?),
            ),
            Expr::Prop(a, k) => Expr::Prop(Box::new(go(a, stage, lw, comps, locals)?), k.clone()),
            Expr::Map(items) => Expr::Map(
                items
                    .iter()
                    .map(|(k, a)| Ok((k.clone(), go(a, stage, lw, comps, locals)?)))
                    .collect::<Result<_>>()?,
            ),
            other => other.clone(),
        })
    }
    let mut out = p.clone();
    let none = BTreeSet::new();
    for it in &mut out.items {
        it.expr = go(&it.expr, stage, lw, comps, &none)?;
    }
    if let Some(w) = &p.where_ {
        out.where_ = Some(go(w, stage, lw, comps, &none)?);
    }
    Ok(out)
}

/// Plans one pattern comprehension (see [`Comprehension`]).
#[allow(clippy::too_many_arguments)]
fn comprehension(
    var: &Option<String>,
    pattern: &PatternElement,
    filter: Option<&Expr>,
    map: &Expr,
    stage: &Stage,
    lw: &mut Lowerer<'_>,
    name: String,
    locals: &BTreeSet<String>,
) -> Result<Comprehension> {
    // The outer variables the comprehension reads, and the nodes bound by an enclosing list
    // comprehension (known only in Rust: the lists are looked up by their ids).
    let mut outer: BTreeSet<String> = BTreeSet::new();
    let mut free_names: Vec<String> = Vec::new();
    for np in std::iter::once(&pattern.start).chain(pattern.chain.iter().map(|(_, n)| n)) {
        if let Some(v) = &np.var {
            if locals.contains(v) && !free_names.contains(v) {
                free_names.push(v.clone());
            }
        }
    }
    let mut bad_local = None;
    let mut add = |n: &str| {
        if locals.contains(n) {
            if !free_names.iter().any(|f| f == n) {
                bad_local = Some(n.to_string());
            }
        } else if stage.scope.contains_key(n) && !n.starts_with('\u{1}') {
            outer.insert(n.to_string());
        }
    };
    for np in std::iter::once(&pattern.start).chain(pattern.chain.iter().map(|(_, n)| n)) {
        if let Some(v) = &np.var {
            add(v);
        }
    }
    for (r, _) in &pattern.chain {
        if let Some(v) = &r.var {
            add(v);
        }
    }
    for e in filter.into_iter().chain(std::iter::once(map)) {
        e.walk(&mut |x| {
            if let Expr::Var(v) = x {
                add(v);
            }
        });
    }
    if let Some(n) = bad_local {
        return Err(CypherError::unsupported(format!(
            "pattern comprehension reading the list variable `{n}` outside its pattern"
        )));
    }
    let keys: Scope = outer
        .iter()
        .map(|n| (n.clone(), stage.scope[n].clone()))
        .collect();
    let key_vars: Vec<Variable> = keys.values().flat_map(Bind::vars).collect();
    let mut inner = stage.child();
    inner.scope = keys;
    inner.pattern = GraphPattern::Distinct {
        inner: Box::new(GraphPattern::Project {
            inner: Box::new(stage.pattern.clone()),
            variables: key_vars.clone(),
        }),
    };
    let part = PatternPart {
        var: var.clone(),
        shortest: None,
        element: pattern.clone(),
    };
    let m = lw.match_patterns(std::slice::from_ref(&part), &mut inner, false, false)?;
    inner.pattern = m.join_into(std::mem::replace(&mut inner.pattern, unit()));
    // The WHERE runs in SQL when it can.
    let mut rust_filter = None;
    if let Some(f) = filter {
        let mut attempt = inner.clone();
        match lw.expr(f, &mut attempt) {
            Ok(x) => {
                inner = attempt;
                inner.pattern = GraphPattern::Filter {
                    expr: x,
                    inner: Box::new(std::mem::replace(&mut inner.pattern, unit())),
                };
            }
            Err(e) if is_unsupported(&e) => rust_filter = Some(f.clone()),
            Err(e) => return Err(e),
        }
    }
    let free: Vec<Variable> = free_names
        .iter()
        .filter_map(|n| match inner.scope.get(n) {
            Some(Bind::Node { var, .. }) => Some(var.clone()),
            _ => None,
        })
        .collect();
    let mut seen = BTreeSet::new();
    let vars: Vec<Variable> = inner
        .scope
        .values()
        .flat_map(Bind::vars)
        .filter(|v| seen.insert(v.clone()))
        .collect();
    Ok(Comprehension {
        name,
        query: select(inner.pattern, vars),
        scope: inner.scope,
        keys: key_vars,
        free,
        free_names,
        filter: rust_filter,
        map: map.clone(),
    })
}

fn mentions_var(e: &Expr, name: &str) -> bool {
    let mut found = false;
    e.walk(&mut |x| {
        if matches!(x, Expr::Var(v) if v == name) {
            found = true;
        }
    });
    found
}

pub(crate) fn pattern_vars(p: &PatternPart) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(v) = &p.var {
        out.push(v.clone());
    }
    if let Some(v) = &p.element.start.var {
        out.push(v.clone());
    }
    for (r, n) in &p.element.chain {
        if let Some(v) = &r.var {
            out.push(v.clone());
        }
        if let Some(v) = &n.var {
            out.push(v.clone());
        }
    }
    out
}

/// `UNWIND <constant list> AS x` as a `VALUES` table.
fn unwind_values(v: Value, alias: &str, stage: &mut Stage, lw: &mut Lowerer<'_>) -> Result<()> {
    let items = match v {
        Value::List(items) => items,
        Value::Null => Vec::new(),
        other => vec![other],
    };
    let maps = !items.is_empty() && items.iter().all(|i| matches!(i, Value::Map(_)));
    if maps {
        let mut keys: BTreeSet<String> = BTreeSet::new();
        for i in &items {
            if let Value::Map(m) = i {
                keys.extend(m.keys().cloned());
            }
        }
        let fields: BTreeMap<String, Variable> = keys
            .iter()
            .map(|k| (k.clone(), lw.fresh(&format!("{alias}{k}"))))
            .collect();
        let mut bindings = Vec::new();
        for i in &items {
            let Value::Map(m) = i else { unreachable!() };
            let mut row = Vec::new();
            for k in &keys {
                row.push(match m.get(k) {
                    Some(v) => ground(v)?,
                    None => None,
                });
            }
            bindings.push(row);
        }
        stage.pattern = join(
            std::mem::replace(&mut stage.pattern, unit()),
            GraphPattern::Values {
                variables: fields.values().cloned().collect(),
                bindings,
            },
        );
        stage.scope.insert(
            alias.to_string(),
            Bind::Map {
                fields,
                nullable: false,
            },
        );
        return Ok(());
    }
    let var = lw.fresh(alias);
    let bindings = items
        .iter()
        .map(|i| Ok(vec![ground(i)?]))
        .collect::<Result<Vec<_>>>()?;
    stage.pattern = join(
        std::mem::replace(&mut stage.pattern, unit()),
        GraphPattern::Values {
            variables: vec![var.clone()],
            bindings,
        },
    );
    stage.scope.insert(
        alias.to_string(),
        Bind::Value {
            var,
            nullable: true,
            name: false,
        },
    );
    Ok(())
}

fn ground(v: &Value) -> Result<Option<GroundTerm>> {
    match v {
        Value::List(_) | Value::Map(_) => Err(CypherError::unsupported(
            "UNWIND of nested lists or maps in SQL",
        )),
        v => Ok(v.to_literal()?.map(GroundTerm::Literal)),
    }
}

#[allow(dead_code)]
pub(crate) fn literal(v: i64) -> Expression {
    Expression::Literal(Literal::from(v))
}

#[allow(dead_code)]
fn is_iri(v: &Variable) -> Expression {
    Expression::FunctionCall(Function::IsIri, vec![Expression::Variable(v.clone())])
}

#[allow(dead_code)]
fn triple(s: &Variable, p: &Variable, o: &Variable) -> TriplePattern {
    TriplePattern {
        subject: TermPattern::Variable(s.clone()),
        predicate: NamedNodePattern::Variable(p.clone()),
        object: TermPattern::Variable(o.clone()),
    }
}
