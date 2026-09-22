//! Semantic checks run before planning: variable scopes and kinds, clause composition and
//! the static rules of openCypher (aliases, aggregation, constant `SKIP`/`LIMIT`…).
//!
// @lat: [[architecture#Property graph frontend]]

use crate::ast::*;
use crate::error::{CypherError, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum K {
    Node,
    Rel,
    Path,
    RelList,
    Any,
}

type Scope = BTreeMap<String, K>;

fn err(code: &str, msg: impl Into<String>) -> CypherError {
    CypherError::Semantic(format!("{code}: {}", msg.into()))
}

pub(crate) fn validate(q: &Query) -> Result<()> {
    if q.union_all.iter().any(|a| *a) && q.union_all.iter().any(|a| !*a) {
        return Err(err(
            "InvalidClauseComposition",
            "UNION and UNION ALL cannot be mixed",
        ));
    }
    for sq in &q.parts {
        single(sq)?;
    }
    Ok(())
}

fn single(sq: &SingleQuery) -> Result<()> {
    let mut scope = Scope::new();
    let n = sq.clauses.len();
    for (i, c) in sq.clauses.iter().enumerate() {
        match c {
            Clause::Match {
                patterns, where_, ..
            } => {
                for p in patterns {
                    bind_pattern(p, &mut scope, Binding::Match)?;
                }
                if let Some(w) = where_ {
                    expr(w, &scope, Ctx::Predicate)?;
                }
            }
            Clause::Unwind { expr: e, alias } => {
                expr(e, &scope, Ctx::Value)?;
                if scope.contains_key(alias) {
                    return Err(err(
                        "VariableAlreadyBound",
                        format!("`{alias}` is already bound"),
                    ));
                }
                scope.insert(alias.clone(), K::Any);
            }
            Clause::With(p) | Clause::Return(p) => {
                let is_with = matches!(c, Clause::With(_));
                scope = projection(p, &scope, is_with)?;
                if !is_with && i + 1 != n {
                    return Err(err(
                        "InvalidClauseComposition",
                        "RETURN must be the last clause",
                    ));
                }
            }
            Clause::Create(parts) => {
                for p in parts {
                    bind_pattern(p, &mut scope, Binding::Create)?;
                }
            }
            Clause::Merge {
                pattern,
                on_create,
                on_match,
            } => {
                bind_pattern(pattern, &mut scope, Binding::Merge)?;
                for it in on_create.iter().chain(on_match) {
                    set_item(it, &scope)?;
                }
            }
            Clause::Set(items) => {
                for it in items {
                    set_item(it, &scope)?;
                }
            }
            Clause::Remove(items) => {
                for it in items {
                    let (RemoveItem::Property { var, .. } | RemoveItem::Labels { var, .. }) = it;
                    defined(var, &scope)?;
                }
            }
            Clause::Delete { exprs, .. } => {
                for e in exprs {
                    if matches!(
                        e,
                        Expr::Int(_)
                            | Expr::Float(_)
                            | Expr::Str(_)
                            | Expr::Bool(_)
                            | Expr::Binary(..)
                            | Expr::List(_)
                            | Expr::Map(_)
                    ) {
                        return Err(err(
                            "InvalidArgumentType",
                            "DELETE expects a node, relationship or path",
                        ));
                    }
                    expr(e, &scope, Ctx::Value)?;
                }
            }
            Clause::Call {
                args,
                yields,
                where_,
                ..
            } => {
                for a in args {
                    expr(a, &scope, Ctx::Value)?;
                }
                if let Some(items) = yields {
                    for (col, alias) in items {
                        scope.insert(alias.clone().unwrap_or_else(|| col.clone()), K::Any);
                    }
                }
                if let Some(w) = where_ {
                    expr(w, &scope, Ctx::Predicate)?;
                }
            }
        }
        if i > 0 && matches!(c, Clause::Match { .. } | Clause::Unwind { .. }) {
            let prev = &sq.clauses[i - 1];
            if prev.is_write() && !matches!(c, Clause::Unwind { .. }) {
                return Err(err(
                    "InvalidClauseComposition",
                    "a reading clause after a write needs WITH in between",
                ));
            }
        }
    }
    Ok(())
}

fn defined(v: &str, scope: &Scope) -> Result<K> {
    scope
        .get(v)
        .copied()
        .ok_or_else(|| err("UndefinedVariable", format!("variable `{v}` not defined")))
}

fn set_item(it: &SetItem, scope: &Scope) -> Result<()> {
    match it {
        SetItem::Property { var, value, .. }
        | SetItem::Replace { var, value }
        | SetItem::Merge { var, value } => {
            defined(var, scope)?;
            expr(value, scope, Ctx::Value)
        }
        SetItem::Labels { var, .. } => defined(var, scope).map(|_| ()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Binding {
    Match,
    Create,
    Merge,
}

fn bind(scope: &mut Scope, name: &str, k: K) -> Result<()> {
    match scope.get(name) {
        None => {
            scope.insert(name.to_string(), k);
            Ok(())
        }
        Some(K::Any) => Ok(()),
        Some(prev) if *prev == k => Ok(()),
        Some(_) => Err(err(
            "VariableTypeConflict",
            format!("`{name}` is bound with a different type"),
        )),
    }
}

fn bind_pattern(p: &PatternPart, scope: &mut Scope, mode: Binding) -> Result<()> {
    let before = scope.clone();
    let el = &p.element;
    let standalone = el.chain.is_empty();
    let mut nodes = vec![&el.start];
    for (_, n) in &el.chain {
        nodes.push(n);
    }
    for np in &nodes {
        if let Some(props) = &np.props {
            expr(props, scope, Ctx::Value)?;
        }
        if let Some(v) = &np.var {
            if mode != Binding::Match
                && before.contains_key(v)
                && (standalone || !np.labels.is_empty() || np.props.is_some())
            {
                return Err(err(
                    "VariableAlreadyBound",
                    format!("`{v}` is already bound"),
                ));
            }
            if matches!(before.get(v), Some(K::Rel | K::Path | K::RelList)) {
                return Err(err("VariableTypeConflict", format!("`{v}` is not a node")));
            }
            bind(scope, v, K::Node)?;
        }
    }
    for (r, _) in &el.chain {
        if let Some(props) = &r.props {
            expr(props, scope, Ctx::Value)?;
        }
        if mode != Binding::Match {
            if r.types.len() != 1 {
                return Err(err(
                    "NoSingleRelationshipType",
                    "a created relationship needs exactly one type",
                ));
            }
            if mode == Binding::Create && r.dir == Direction::Both {
                return Err(err(
                    "RequiresDirectedRelationship",
                    "a created relationship needs a direction",
                ));
            }
            if r.length.is_some() {
                return Err(err(
                    "CreatingVarLength",
                    "variable-length relationships cannot be created",
                ));
            }
        }
        if let Some(v) = &r.var {
            if mode != Binding::Match && before.contains_key(v) {
                return Err(err(
                    "VariableAlreadyBound",
                    format!("`{v}` is already bound"),
                ));
            }
            let k = if r.length.is_some() {
                K::RelList
            } else {
                K::Rel
            };
            if matches!(before.get(v), Some(K::Node | K::Path)) {
                return Err(err(
                    "VariableTypeConflict",
                    format!("`{v}` is not a relationship"),
                ));
            }
            bind(scope, v, k)?;
        }
    }
    if let Some(v) = &p.var {
        if before.contains_key(v) {
            return Err(err(
                "VariableAlreadyBound",
                format!("`{v}` is already bound"),
            ));
        }
        scope.insert(v.clone(), K::Path);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// Where patterns are allowed as predicates.
    Predicate,
    Value,
    /// Inside an aggregate call (no nested aggregates).
    Aggregate,
}

fn literal_non_bool(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::List(_) | Expr::Map(_)
    )
}

fn expr(e: &Expr, scope: &Scope, ctx: Ctx) -> Result<()> {
    let sub = |x: &Expr, c: Ctx| expr(x, scope, c);
    let value_ctx = if ctx == Ctx::Aggregate {
        Ctx::Aggregate
    } else {
        Ctx::Value
    };
    match e {
        Expr::Var(v) => {
            defined(v, scope)?;
        }
        Expr::Prop(t, _) => sub(t, value_ctx)?,
        Expr::List(items) => items.iter().try_for_each(|i| sub(i, value_ctx))?,
        Expr::Map(items) => items.iter().try_for_each(|(_, i)| sub(i, value_ctx))?,
        Expr::Unary(UnOp::Not, x) => {
            if literal_non_bool(x) {
                return Err(err("InvalidArgumentType", "NOT expects a boolean"));
            }
            sub(
                x,
                if ctx == Ctx::Predicate {
                    Ctx::Predicate
                } else {
                    value_ctx
                },
            )?
        }
        Expr::Unary(_, x) | Expr::IsNull(x, _) => sub(x, value_ctx)?,
        Expr::HasLabels(x, _) => sub(x, value_ctx)?,
        Expr::Binary(op, a, b) => {
            let boolean = matches!(op, BinOp::And | BinOp::Or | BinOp::Xor);
            if boolean && (literal_non_bool(a) || literal_non_bool(b)) {
                return Err(err(
                    "InvalidArgumentType",
                    "boolean operators expect booleans",
                ));
            }
            let c = if boolean && ctx == Ctx::Predicate {
                Ctx::Predicate
            } else {
                value_ctx
            };
            sub(a, c)?;
            sub(b, c)?;
        }
        Expr::Func { name, args, .. } => {
            if is_aggregate(name) {
                if ctx == Ctx::Aggregate {
                    return Err(err("NestedAggregation", "aggregates cannot be nested"));
                }
                if name == "count"
                    && args
                        .iter()
                        .any(|a| matches!(a, Expr::Func { name, .. } if name == "rand"))
                {
                    return Err(err("NonConstantExpression", "rand() inside an aggregate"));
                }
                for a in args {
                    sub(a, Ctx::Aggregate)?;
                }
            } else {
                for a in args {
                    sub(a, value_ctx)?;
                }
                let kind = |a: &Expr| match a {
                    Expr::Var(v) => scope.get(v).copied(),
                    _ => None,
                };
                if let Some(a) = args.first() {
                    let k = kind(a);
                    let bad = match name.as_str() {
                        "type" | "startnode" | "endnode" => matches!(k, Some(K::Node | K::Path)),
                        "labels" => matches!(k, Some(K::Rel | K::Path)),
                        "length" => matches!(k, Some(K::Node | K::Rel)),
                        "nodes" | "relationships" => matches!(k, Some(K::Node | K::Rel)),
                        "size" => matches!(k, Some(K::Path)) || matches!(a, Expr::Pattern(_)),
                        _ => false,
                    };
                    if bad {
                        return Err(err(
                            "InvalidArgumentType",
                            format!("wrong argument type for {name}()"),
                        ));
                    }
                }
            }
        }
        Expr::CountStar => {
            if ctx == Ctx::Aggregate {
                return Err(err("NestedAggregation", "aggregates cannot be nested"));
            }
        }
        Expr::Case {
            operand,
            whens,
            else_,
        } => {
            if let Some(o) = operand {
                sub(o, value_ctx)?;
            }
            for (w, t) in whens {
                sub(w, value_ctx)?;
                sub(t, value_ctx)?;
            }
            if let Some(x) = else_ {
                sub(x, value_ctx)?;
            }
        }
        Expr::ListComp {
            var,
            list,
            filter,
            map,
        } => {
            sub(list, value_ctx)?;
            let mut inner = scope.clone();
            inner.insert(var.clone(), K::Any);
            if let Some(f) = filter {
                expr(f, &inner, Ctx::Predicate)?;
            }
            if let Some(m) = map {
                expr(m, &inner, value_ctx)?;
            }
        }
        Expr::Quantified {
            var, list, pred, ..
        } => {
            sub(list, value_ctx)?;
            let mut inner = scope.clone();
            inner.insert(var.clone(), K::Any);
            expr(pred, &inner, Ctx::Predicate)?;
        }
        Expr::Reduce {
            acc,
            init,
            var,
            list,
            expr: body,
        } => {
            sub(init, value_ctx)?;
            sub(list, value_ctx)?;
            let mut inner = scope.clone();
            inner.insert(acc.clone(), K::Any);
            inner.insert(var.clone(), K::Any);
            expr(body, &inner, value_ctx)?;
        }
        Expr::Index(a, b) => {
            sub(a, value_ctx)?;
            sub(b, value_ctx)?;
        }
        Expr::Slice(a, b, c) => {
            sub(a, value_ctx)?;
            if let Some(b) = b {
                sub(b, value_ctx)?;
            }
            if let Some(c) = c {
                sub(c, value_ctx)?;
            }
        }
        Expr::Pattern(el) => {
            if ctx != Ctx::Predicate {
                return Err(err(
                    "UnexpectedSyntax",
                    "a pattern expression is only allowed as a predicate",
                ));
            }
            pattern_refs(el, scope)?;
        }
        Expr::Exists(parts, where_) => {
            let mut inner = scope.clone();
            for p in parts {
                bind_pattern(p, &mut inner, Binding::Match)?;
            }
            if let Some(w) = where_ {
                expr(w, &inner, Ctx::Predicate)?;
            }
        }
        Expr::PatternComp {
            var,
            pattern,
            filter,
            map,
        } => {
            let mut inner = scope.clone();
            bind_pattern(
                &PatternPart {
                    var: var.clone(),
                    shortest: None,
                    element: (**pattern).clone(),
                },
                &mut inner,
                Binding::Match,
            )?;
            if let Some(f) = filter {
                expr(f, &inner, Ctx::Predicate)?;
            }
            expr(map, &inner, value_ctx)?;
        }
        Expr::MapProjection { var, items } => {
            defined(var, scope)?;
            for it in items {
                match it {
                    MapProjItem::Var(v) => {
                        defined(v, scope)?;
                    }
                    MapProjItem::Entry(_, e) => sub(e, value_ctx)?,
                    _ => {}
                }
            }
        }
        Expr::Null
        | Expr::Bool(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Param(_) => {}
    }
    Ok(())
}

/// A pattern used as a predicate cannot introduce named variables.
fn pattern_refs(el: &PatternElement, scope: &Scope) -> Result<()> {
    let mut vars = vec![el.start.var.as_ref()];
    for (r, n) in &el.chain {
        vars.push(r.var.as_ref());
        vars.push(n.var.as_ref());
    }
    for v in vars.into_iter().flatten() {
        defined(v, scope)?;
    }
    for n in std::iter::once(&el.start).chain(el.chain.iter().map(|(_, n)| n)) {
        if let Some(p) = &n.props {
            expr(p, scope, Ctx::Value)?;
        }
    }
    Ok(())
}

fn constant_count(e: &Expr, what: &str) -> Result<()> {
    let mut vars = false;
    let mut rand = false;
    e.walk(&mut |x| match x {
        Expr::Var(_) => vars = true,
        Expr::Func { name, .. } if name == "rand" => rand = true,
        _ => {}
    });
    if vars || rand {
        return Err(err(
            "NonConstantExpression",
            format!("{what} must be constant"),
        ));
    }
    match e {
        Expr::Int(i) if *i < 0 => Err(err(
            "NegativeIntegerArgument",
            format!("{what} must not be negative"),
        )),
        Expr::Float(_) => Err(err(
            "InvalidArgumentType",
            format!("{what} must be an integer"),
        )),
        _ => Ok(()),
    }
}

fn projection(p: &Projection, scope: &Scope, is_with: bool) -> Result<Scope> {
    let mut out = Scope::new();
    if p.star {
        out.extend(scope.iter().map(|(k, v)| (k.clone(), *v)));
    }
    let aggregating = p.items.iter().any(|it| it.expr.has_aggregate());
    let mut keys: Vec<&Expr> = Vec::new();
    for it in &p.items {
        expr(&it.expr, scope, Ctx::Value)?;
        if is_with && it.alias.is_none() && !matches!(it.expr, Expr::Var(_)) {
            return Err(err(
                "NoExpressionAlias",
                "expressions in WITH need an alias",
            ));
        }
        let name = it.name();
        if p.items.iter().filter(|x| x.name() == name).count() > 1 {
            return Err(err(
                "ColumnNameConflict",
                format!("column `{name}` is projected twice"),
            ));
        }
        if !it.expr.has_aggregate() {
            keys.push(&it.expr);
        }
        let k = match &it.expr {
            Expr::Var(v) => scope.get(v).copied().unwrap_or(K::Any),
            _ => K::Any,
        };
        out.insert(name, k);
    }
    if aggregating {
        // Outside aggregate calls, an aggregating item may only read grouping variables.
        let key_vars: Vec<&str> = keys
            .iter()
            .filter_map(|k| match k {
                Expr::Var(v) => Some(v.as_str()),
                _ => None,
            })
            .collect();
        for it in &p.items {
            if it.expr.has_aggregate() {
                let mut bad = false;
                outside_aggregates(&it.expr, &keys, &mut |x| {
                    if let Expr::Var(v) = x {
                        if !key_vars.contains(&v.as_str()) {
                            bad = true;
                        }
                    }
                });
                if bad {
                    return Err(err(
                        "AmbiguousAggregationExpression",
                        "an aggregating expression reads a variable that is not a grouping key",
                    ));
                }
            }
        }
    }
    let mut combined = scope.clone();
    combined.extend(out.iter().map(|(k, v)| (k.clone(), *v)));
    for (e, _) in &p.order {
        let s = if (aggregating || p.distinct) && !e.has_aggregate() {
            &out
        } else {
            &combined
        };
        // ORDER BY may repeat a projected expression (or contain one).
        let projected: Vec<&Expr> = p.items.iter().map(|it| &it.expr).collect();
        let reduced = replace_projected(e, &projected);
        if expr(&reduced, s, Ctx::Value).is_err() {
            expr(&reduced, &combined, Ctx::Value)?;
        }
    }
    if let Some(s) = &p.skip {
        constant_count(s, "SKIP")?;
    }
    if let Some(l) = &p.limit {
        constant_count(l, "LIMIT")?;
    }
    if let Some(w) = &p.where_ {
        let s = if aggregating { &out } else { &combined };
        expr(w, s, Ctx::Predicate)?;
    }
    Ok(out)
}

/// Replaces sub-expressions equal to projected expressions with `null` (they are valid in
/// ORDER BY of an aggregating projection).
fn replace_projected(e: &Expr, projected: &[&Expr]) -> Expr {
    if projected.contains(&e) {
        return Expr::Null;
    }
    match e {
        Expr::Binary(op, a, b) => Expr::Binary(
            *op,
            Box::new(replace_projected(a, projected)),
            Box::new(replace_projected(b, projected)),
        ),
        Expr::Unary(op, a) => Expr::Unary(*op, Box::new(replace_projected(a, projected))),
        Expr::Func {
            name,
            distinct,
            args,
        } if !is_aggregate(name) => Expr::Func {
            name: name.clone(),
            distinct: *distinct,
            args: args
                .iter()
                .map(|a| replace_projected(a, projected))
                .collect(),
        },
        other => other.clone(),
    }
}

fn outside_aggregates(e: &Expr, keys: &[&Expr], f: &mut dyn FnMut(&Expr)) {
    if keys.contains(&e) {
        return;
    }
    match e {
        Expr::Func { name, .. } if is_aggregate(name) => {}
        Expr::CountStar => {}
        Expr::ListComp { .. } | Expr::Quantified { .. } | Expr::Reduce { .. } => {}
        _ => {
            f(e);
            match e {
                Expr::Binary(_, a, b) | Expr::Index(a, b) => {
                    outside_aggregates(a, keys, f);
                    outside_aggregates(b, keys, f);
                }
                Expr::Unary(_, a) | Expr::Prop(a, _) | Expr::IsNull(a, _) => {
                    outside_aggregates(a, keys, f)
                }
                Expr::Func { args, .. } => args.iter().for_each(|a| outside_aggregates(a, keys, f)),
                Expr::List(items) => items.iter().for_each(|a| outside_aggregates(a, keys, f)),
                Expr::Case {
                    operand,
                    whens,
                    else_,
                } => {
                    if let Some(o) = operand {
                        outside_aggregates(o, keys, f);
                    }
                    for (w, t) in whens {
                        outside_aggregates(w, keys, f);
                        outside_aggregates(t, keys, f);
                    }
                    if let Some(x) = else_ {
                        outside_aggregates(x, keys, f);
                    }
                }
                _ => {}
            }
        }
    }
}
