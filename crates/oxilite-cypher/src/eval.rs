//! Evaluation of Cypher expressions in Rust, over rows of Cypher values.
//!
//! The SQL part of a query does the heavy lifting (matching, filtering, grouping, ordering);
//! this evaluator computes what SQL cannot express — lists, maps, `collect()`, node and
//! relationship values, write clauses — with openCypher's null semantics.
//!
// @lat: [[architecture#Property graph frontend#Planning and lowering]]

use crate::ast::{is_aggregate, BinOp, Expr, MapProjItem, Quantifier, UnOp};
use crate::error::{CypherError, Result};
use crate::value::{Params, TemporalKind, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

/// A row: the Cypher variables in scope and their values.
pub type Row = HashMap<String, Value>;

pub(crate) struct Env<'a> {
    pub row: &'a Row,
    pub params: &'a Params,
}

impl<'a> Env<'a> {
    pub fn new(row: &'a Row, params: &'a Params) -> Self {
        Self { row, params }
    }
}

fn truthy(v: &Value) -> Result<Option<bool>> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(*b)),
        other => Err(CypherError::runtime(format!(
            "expected a boolean, got {}",
            other.type_name()
        ))),
    }
}

/// Evaluates a predicate: `null` counts as false.
pub(crate) fn eval_pred(e: &Expr, env: &Env<'_>) -> Result<bool> {
    Ok(truthy(&eval(e, env)?)?.unwrap_or(false))
}

pub(crate) fn eval(e: &Expr, env: &Env<'_>) -> Result<Value> {
    Ok(match e {
        Expr::Null => Value::Null,
        Expr::Bool(b) => Value::Bool(*b),
        Expr::Int(i) => Value::Int(*i),
        Expr::Float(f) => Value::Float(*f),
        Expr::Str(s) => Value::String(s.clone()),
        Expr::Param(p) => env
            .params
            .get(p)
            .cloned()
            .ok_or_else(|| CypherError::semantic(format!("missing parameter ${p}")))?,
        Expr::Var(v) => env
            .row
            .get(v)
            .cloned()
            .ok_or_else(|| CypherError::semantic(format!("variable `{v}` not defined")))?,
        Expr::List(items) => {
            Value::List(items.iter().map(|i| eval(i, env)).collect::<Result<_>>()?)
        }
        Expr::Map(items) => Value::Map(
            items
                .iter()
                .map(|(k, v)| Ok((k.clone(), eval(v, env)?)))
                .collect::<Result<_>>()?,
        ),
        Expr::Prop(target, key) => property(&eval(target, env)?, key)?,
        Expr::Unary(op, inner) => {
            let v = eval(inner, env)?;
            match op {
                UnOp::Not => match truthy(&v)? {
                    None => Value::Null,
                    Some(b) => Value::Bool(!b),
                },
                UnOp::Neg => match v {
                    Value::Null => Value::Null,
                    Value::Int(i) => Value::Int(
                        i.checked_neg()
                            .ok_or_else(|| CypherError::runtime("integer overflow"))?,
                    ),
                    Value::Float(f) => Value::Float(-f),
                    o => {
                        return Err(CypherError::runtime(format!(
                            "cannot negate a {}",
                            o.type_name()
                        )))
                    }
                },
                UnOp::Plus => v,
            }
        }
        Expr::Binary(op, a, b) => binary(*op, a, b, env)?,
        Expr::IsNull(inner, negated) => Value::Bool(eval(inner, env)?.is_null() != *negated),
        Expr::HasLabels(inner, labels) => match eval(inner, env)? {
            Value::Null => Value::Null,
            Value::Node(n) => Value::Bool(labels.iter().all(|l| n.labels.contains(l))),
            o => {
                return Err(CypherError::runtime(format!(
                    "label test on a {}",
                    o.type_name()
                )))
            }
        },
        Expr::Func {
            name,
            distinct: _,
            args,
        } => {
            if is_aggregate(name) {
                return Err(CypherError::semantic(format!(
                    "aggregate {name}() is not allowed here"
                )));
            }
            let vals = args
                .iter()
                .map(|a| eval(a, env))
                .collect::<Result<Vec<_>>>()?;
            function(name, vals)?
        }
        Expr::CountStar => {
            return Err(CypherError::semantic("count(*) is not allowed here"));
        }
        Expr::Case {
            operand,
            whens,
            else_,
        } => {
            let op = operand.as_ref().map(|o| eval(o, env)).transpose()?;
            for (w, t) in whens {
                let hit = match &op {
                    Some(v) => v.cypher_eq(&eval(w, env)?) == Some(true),
                    None => eval_pred(w, env)?,
                };
                if hit {
                    return eval(t, env);
                }
            }
            match else_ {
                Some(e) => eval(e, env)?,
                None => Value::Null,
            }
        }
        Expr::ListComp {
            var,
            list,
            filter,
            map,
        } => {
            let items = match eval(list, env)? {
                Value::Null => return Ok(Value::Null),
                Value::List(items) => items,
                o => {
                    return Err(CypherError::runtime(format!(
                        "list comprehension over a {}",
                        o.type_name()
                    )))
                }
            };
            let mut out = Vec::new();
            for item in items {
                let mut row = env.row.clone();
                row.insert(var.clone(), item.clone());
                let inner = Env {
                    row: &row,
                    params: env.params,
                };
                if let Some(f) = filter {
                    if !eval_pred(f, &inner)? {
                        continue;
                    }
                }
                out.push(match map {
                    Some(m) => eval(m, &inner)?,
                    None => item,
                });
            }
            Value::List(out)
        }
        Expr::Quantified { q, var, list, pred } => {
            let items = match eval(list, env)? {
                Value::Null => return Ok(Value::Null),
                Value::List(items) => items,
                o => {
                    return Err(CypherError::runtime(format!(
                        "quantifier over a {}",
                        o.type_name()
                    )))
                }
            };
            let (mut trues, mut nulls) = (0usize, 0usize);
            for item in &items {
                let mut row = env.row.clone();
                row.insert(var.clone(), item.clone());
                match truthy(&eval(
                    pred,
                    &Env {
                        row: &row,
                        params: env.params,
                    },
                )?)? {
                    Some(true) => trues += 1,
                    None => nulls += 1,
                    Some(false) => {}
                }
            }
            let falses = items.len() - trues - nulls;
            let r = match q {
                Quantifier::All => {
                    if falses > 0 {
                        Some(false)
                    } else if nulls > 0 {
                        None
                    } else {
                        Some(true)
                    }
                }
                Quantifier::Any => {
                    if trues > 0 {
                        Some(true)
                    } else if nulls > 0 {
                        None
                    } else {
                        Some(false)
                    }
                }
                Quantifier::None => {
                    if trues > 0 {
                        Some(false)
                    } else if nulls > 0 {
                        None
                    } else {
                        Some(true)
                    }
                }
                Quantifier::Single => {
                    if trues > 1 {
                        Some(false)
                    } else if nulls > 0 {
                        None
                    } else {
                        Some(trues == 1)
                    }
                }
            };
            r.map_or(Value::Null, Value::Bool)
        }
        Expr::Reduce {
            acc,
            init,
            var,
            list,
            expr,
        } => {
            let mut a = eval(init, env)?;
            let items = match eval(list, env)? {
                Value::Null => return Ok(Value::Null),
                Value::List(items) => items,
                o => {
                    return Err(CypherError::runtime(format!(
                        "reduce over a {}",
                        o.type_name()
                    )))
                }
            };
            for item in items {
                let mut row = env.row.clone();
                row.insert(acc.clone(), a);
                row.insert(var.clone(), item);
                a = eval(
                    expr,
                    &Env {
                        row: &row,
                        params: env.params,
                    },
                )?;
            }
            a
        }
        Expr::Index(target, idx) => index(eval(target, env)?, eval(idx, env)?)?,
        Expr::Slice(target, from, to) => {
            let t = eval(target, env)?;
            let from = from.as_ref().map(|f| eval(f, env)).transpose()?;
            let to = to.as_ref().map(|f| eval(f, env)).transpose()?;
            slice(t, from, to)?
        }
        Expr::MapProjection { var, items } => {
            let base =
                env.row.get(var).cloned().ok_or_else(|| {
                    CypherError::semantic(format!("variable `{var}` not defined"))
                })?;
            if base.is_null() {
                return Ok(Value::Null);
            }
            let props = properties_of(&base)?;
            let mut out = BTreeMap::new();
            for item in items {
                match item {
                    MapProjItem::AllProps => out.extend(props.clone()),
                    MapProjItem::Prop(k) => {
                        out.insert(k.clone(), props.get(k).cloned().unwrap_or(Value::Null));
                    }
                    MapProjItem::Var(v) => {
                        out.insert(
                            v.clone(),
                            env.row.get(v).cloned().ok_or_else(|| {
                                CypherError::semantic(format!("variable `{v}` not defined"))
                            })?,
                        );
                    }
                    MapProjItem::Entry(k, e) => {
                        out.insert(k.clone(), eval(e, env)?);
                    }
                }
            }
            Value::Map(out)
        }
        Expr::Pattern(_) | Expr::Exists(..) | Expr::PatternComp { .. } => {
            return Err(CypherError::unsupported(
                "pattern expressions after a write clause or in a computed projection",
            ))
        }
    })
}

pub(crate) fn properties_of(v: &Value) -> Result<BTreeMap<String, Value>> {
    Ok(match v {
        Value::Node(n) => n.properties.clone(),
        Value::Relationship(r) => r.properties.clone(),
        Value::Map(m) => m.clone(),
        Value::Null => BTreeMap::new(),
        o => {
            return Err(CypherError::runtime(format!(
                "a {} has no properties",
                o.type_name()
            )))
        }
    })
}

pub(crate) fn property(v: &Value, key: &str) -> Result<Value> {
    Ok(match v {
        Value::Null => Value::Null,
        Value::Node(n) => n.properties.get(key).cloned().unwrap_or(Value::Null),
        Value::Relationship(r) => r.properties.get(key).cloned().unwrap_or(Value::Null),
        Value::Map(m) => m.get(key).cloned().unwrap_or(Value::Null),
        Value::Temporal(kind, lex) => {
            crate::temporal::field(&crate::temporal::T::parse(*kind, lex)?, key)?
        }
        o => {
            return Err(CypherError::runtime(format!(
                "cannot access property `{key}` of a {}",
                o.type_name()
            )))
        }
    })
}

fn index(t: Value, i: Value) -> Result<Value> {
    Ok(match (t, i) {
        (Value::Null, _) | (_, Value::Null) => Value::Null,
        (Value::List(items), Value::Int(i)) => {
            let n = items.len() as i64;
            let i = if i < 0 { n + i } else { i };
            if (0..n).contains(&i) {
                items[i as usize].clone()
            } else {
                Value::Null
            }
        }

        (m @ (Value::Map(_) | Value::Node(_) | Value::Relationship(_)), Value::String(k)) => {
            property(&m, &k)?
        }
        (t, i) => {
            return Err(CypherError::runtime(format!(
                "cannot index a {} with a {}",
                t.type_name(),
                i.type_name()
            )))
        }
    })
}

fn slice(t: Value, from: Option<Value>, to: Option<Value>) -> Result<Value> {
    let bound = |v: Option<Value>, n: i64, default: i64| -> Result<Option<i64>> {
        Ok(match v {
            None => Some(default),
            Some(Value::Null) => None,
            Some(Value::Int(i)) => Some(if i < 0 { (n + i).max(0) } else { i.min(n) }),
            Some(o) => {
                return Err(CypherError::runtime(format!(
                    "slice bound must be an integer, got {}",
                    o.type_name()
                )))
            }
        })
    };
    Ok(match t {
        Value::Null => Value::Null,
        Value::List(items) => {
            let n = items.len() as i64;
            let (Some(a), Some(b)) = (bound(from, n, 0)?, bound(to, n, n)?) else {
                return Ok(Value::Null);
            };
            if a >= b {
                Value::List(Vec::new())
            } else {
                Value::List(items[a as usize..b as usize].to_vec())
            }
        }
        o => {
            return Err(CypherError::runtime(format!(
                "cannot slice a {}",
                o.type_name()
            )))
        }
    })
}

fn binary(op: BinOp, a: &Expr, b: &Expr, env: &Env<'_>) -> Result<Value> {
    // Boolean operators short-circuit with three-valued logic.
    match op {
        BinOp::And => {
            let x = truthy(&eval(a, env)?)?;
            if x == Some(false) {
                return Ok(Value::Bool(false));
            }
            let y = truthy(&eval(b, env)?)?;
            return Ok(match (x, y) {
                (_, Some(false)) => Value::Bool(false),
                (Some(true), Some(true)) => Value::Bool(true),
                _ => Value::Null,
            });
        }
        BinOp::Or => {
            let x = truthy(&eval(a, env)?)?;
            if x == Some(true) {
                return Ok(Value::Bool(true));
            }
            let y = truthy(&eval(b, env)?)?;
            return Ok(match (x, y) {
                (_, Some(true)) => Value::Bool(true),
                (Some(false), Some(false)) => Value::Bool(false),
                _ => Value::Null,
            });
        }
        BinOp::Xor => {
            let x = truthy(&eval(a, env)?)?;
            let y = truthy(&eval(b, env)?)?;
            return Ok(match (x, y) {
                (Some(x), Some(y)) => Value::Bool(x != y),
                _ => Value::Null,
            });
        }
        _ => {}
    }
    let x = eval(a, env)?;
    let y = eval(b, env)?;
    binary_values(op, x, y)
}

pub(crate) fn binary_values(op: BinOp, x: Value, y: Value) -> Result<Value> {
    if matches!(x, Value::Temporal(..)) || matches!(y, Value::Temporal(..)) {
        let c = match op {
            BinOp::Add => Some('+'),
            BinOp::Sub => Some('-'),
            BinOp::Mul => Some('*'),
            BinOp::Div => Some('/'),
            _ => None,
        };
        if let Some(c) = c {
            if x.is_null() || y.is_null() {
                return Ok(Value::Null);
            }
            return match crate::temporal::arith(c, &x, &y)? {
                Some(v) => Ok(v),
                None => Err(CypherError::runtime(format!(
                    "cannot apply {c} to {} and {}",
                    x.type_name(),
                    y.type_name()
                ))),
            };
        }
    }
    Ok(match op {
        BinOp::Eq => x.cypher_eq(&y).map_or(Value::Null, Value::Bool),
        BinOp::Ne => x.cypher_eq(&y).map_or(Value::Null, |b| Value::Bool(!b)),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if x.is_null() || y.is_null() {
                return Ok(Value::Null);
            }
            match x.cypher_cmp(&y) {
                None => Value::Null,
                Some(o) => Value::Bool(match op {
                    BinOp::Lt => o == Ordering::Less,
                    BinOp::Le => o != Ordering::Greater,
                    BinOp::Gt => o == Ordering::Greater,
                    _ => o != Ordering::Less,
                }),
            }
        }
        BinOp::In => match y {
            Value::Null => Value::Null,
            Value::List(items) => {
                let mut unknown = false;
                for i in &items {
                    match x.cypher_eq(i) {
                        Some(true) => return Ok(Value::Bool(true)),
                        None => unknown = true,
                        Some(false) => {}
                    }
                }
                if unknown {
                    Value::Null
                } else {
                    Value::Bool(false)
                }
            }
            o => {
                return Err(CypherError::runtime(format!(
                    "IN expects a list, got {}",
                    o.type_name()
                )))
            }
        },
        BinOp::StartsWith | BinOp::EndsWith | BinOp::Contains => match (x, y) {
            (Value::String(s), Value::String(t)) => Value::Bool(match op {
                BinOp::StartsWith => s.starts_with(&t),
                BinOp::EndsWith => s.ends_with(&t),
                _ => s.contains(&t),
            }),
            _ => Value::Null,
        },
        BinOp::Regex => match (x, y) {
            (Value::String(s), Value::String(p)) => {
                let re = regex_lite::Regex::new(&format!("^(?:{p})$"))
                    .map_err(|e| CypherError::runtime(format!("invalid regex: {e}")))?;
                Value::Bool(re.is_match(&s))
            }
            _ => Value::Null,
        },
        BinOp::Add => match (x, y) {
            (Value::Null, _) | (_, Value::Null) => Value::Null,
            (Value::Int(a), Value::Int(b)) => Value::Int(
                a.checked_add(b)
                    .ok_or_else(|| CypherError::runtime("integer overflow"))?,
            ),
            (a @ (Value::Int(_) | Value::Float(_)), b @ (Value::Int(_) | Value::Float(_))) => {
                Value::Float(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0))
            }
            (Value::String(a), Value::String(b)) => Value::String(a + &b),
            (Value::String(a), b @ (Value::Int(_) | Value::Float(_) | Value::Bool(_))) => {
                Value::String(a + &to_string(&b))
            }
            (a @ (Value::Int(_) | Value::Float(_) | Value::Bool(_)), Value::String(b)) => {
                Value::String(to_string(&a) + &b)
            }
            (Value::List(mut a), Value::List(b)) => {
                a.extend(b);
                Value::List(a)
            }
            (Value::List(mut a), b) => {
                a.push(b);
                Value::List(a)
            }
            (a, Value::List(b)) => {
                let mut out = vec![a];
                out.extend(b);
                Value::List(out)
            }
            (a, b) => {
                return Err(CypherError::runtime(format!(
                    "cannot add {} and {}",
                    a.type_name(),
                    b.type_name()
                )))
            }
        },
        BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => match (x, y) {
            (Value::Null, _) | (_, Value::Null) => Value::Null,
            (Value::Int(a), Value::Int(b)) => Value::Int(
                match op {
                    BinOp::Sub => a.checked_sub(b),
                    BinOp::Mul => a.checked_mul(b),
                    BinOp::Div => {
                        if b == 0 {
                            return Err(CypherError::runtime("division by zero"));
                        }
                        a.checked_div(b)
                    }
                    _ => {
                        if b == 0 {
                            return Err(CypherError::runtime("division by zero"));
                        }
                        a.checked_rem(b)
                    }
                }
                .ok_or_else(|| CypherError::runtime("integer overflow"))?,
            ),
            (a @ (Value::Int(_) | Value::Float(_)), b @ (Value::Int(_) | Value::Float(_))) => {
                let (a, b) = (a.as_f64().unwrap_or(0.0), b.as_f64().unwrap_or(0.0));
                Value::Float(match op {
                    BinOp::Sub => a - b,
                    BinOp::Mul => a * b,
                    BinOp::Div => a / b,
                    _ => a % b,
                })
            }
            (a, b) => {
                return Err(CypherError::runtime(format!(
                    "arithmetic on {} and {}",
                    a.type_name(),
                    b.type_name()
                )))
            }
        },
        BinOp::Pow => match (x.as_f64(), y.as_f64()) {
            (Some(a), Some(b)) => Value::Float(a.powf(b)),
            _ if x.is_null() || y.is_null() => Value::Null,
            _ => return Err(CypherError::runtime("^ expects numbers")),
        },
        BinOp::And | BinOp::Or | BinOp::Xor => unreachable!("handled by binary()"),
    })
}

pub(crate) fn to_string(v: &Value) -> String {
    match v {
        Value::String(s) | Value::Temporal(_, s) => s.clone(),
        Value::Float(f) => {
            if f.fract() == 0.0 && f.is_finite() && f.abs() < 1e16 {
                format!("{f:.1}")
            } else {
                f.to_string()
            }
        }
        other => other.to_string(),
    }
}

fn int_arg(v: &Value, f: &str) -> Result<Option<i64>> {
    match v {
        Value::Null => Ok(None),
        Value::Int(i) => Ok(Some(*i)),
        Value::Float(x) if x.fract() == 0.0 => Ok(Some(*x as i64)),
        o => Err(CypherError::runtime(format!(
            "{f}() expects an integer, got {}",
            o.type_name()
        ))),
    }
}

fn str_arg<'v>(v: &'v Value, f: &str) -> Result<Option<&'v str>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s)),
        o => Err(CypherError::runtime(format!(
            "{f}() expects a string, got {}",
            o.type_name()
        ))),
    }
}

fn num_fn(v: &Value, f: &str, op: impl Fn(f64) -> f64) -> Result<Value> {
    match v {
        Value::Null => Ok(Value::Null),
        Value::Int(_) | Value::Float(_) => Ok(Value::Float(op(v.as_f64().unwrap_or(0.0)))),
        o => Err(CypherError::runtime(format!(
            "{f}() expects a number, got {}",
            o.type_name()
        ))),
    }
}

fn arity(name: &str, args: &[Value], min: usize, max: usize) -> Result<()> {
    if args.len() < min || args.len() > max {
        return Err(CypherError::semantic(format!(
            "{name}() takes {} argument(s), got {}",
            if min == max {
                min.to_string()
            } else {
                format!("{min}–{max}")
            },
            args.len()
        )));
    }
    Ok(())
}

/// Scalar functions.
pub(crate) fn function(name: &str, args: Vec<Value>) -> Result<Value> {
    let a0 = args.first().cloned().unwrap_or(Value::Null);
    Ok(match name {
        "coalesce" => args
            .into_iter()
            .find(|v| !v.is_null())
            .unwrap_or(Value::Null),
        "id" | "elementid" => {
            arity(name, &args, 1, 1)?;
            match a0 {
                Value::Null => Value::Null,
                Value::Node(n) => Value::String(n.element_id()),
                Value::Relationship(r) => Value::String(r.element_id()),
                o => {
                    return Err(CypherError::runtime(format!(
                        "{name}() of a {}",
                        o.type_name()
                    )))
                }
            }
        }
        "labels" => match a0 {
            Value::Null => Value::Null,
            Value::Node(n) => Value::List(n.labels.into_iter().map(Value::String).collect()),
            o => {
                return Err(CypherError::runtime(format!(
                    "labels() of a {}",
                    o.type_name()
                )))
            }
        },
        "type" => match a0 {
            Value::Null => Value::Null,
            Value::Relationship(r) => Value::String(r.rel_type),
            o => {
                return Err(CypherError::runtime(format!(
                    "type() of a {}",
                    o.type_name()
                )))
            }
        },
        "keys" => match a0 {
            Value::Null => Value::Null,
            v => Value::List(properties_of(&v)?.into_keys().map(Value::String).collect()),
        },
        "properties" => match a0 {
            Value::Null => Value::Null,
            v => Value::Map(properties_of(&v)?),
        },
        "startnode" | "endnode" => match a0 {
            Value::Null => Value::Null,
            Value::Relationship(r) => {
                let id = if name == "startnode" { r.start } else { r.end };
                Value::Node(crate::value::Node {
                    id,
                    labels: Vec::new(),
                    properties: BTreeMap::new(),
                })
            }
            o => {
                return Err(CypherError::runtime(format!(
                    "{name}() of a {}",
                    o.type_name()
                )))
            }
        },
        "nodes" => match a0 {
            Value::Null => Value::Null,
            Value::Path(p) => Value::List(p.nodes.into_iter().map(Value::Node).collect()),
            o => {
                return Err(CypherError::runtime(format!(
                    "nodes() of a {}",
                    o.type_name()
                )))
            }
        },
        "relationships" | "rels" => match a0 {
            Value::Null => Value::Null,
            Value::Path(p) => Value::List(
                p.relationships
                    .into_iter()
                    .map(Value::Relationship)
                    .collect(),
            ),
            o => {
                return Err(CypherError::runtime(format!(
                    "relationships() of a {}",
                    o.type_name()
                )))
            }
        },
        "size" => match a0 {
            Value::Null => Value::Null,
            Value::List(l) => Value::Int(l.len() as i64),
            Value::String(s) => Value::Int(s.chars().count() as i64),
            Value::Map(m) => Value::Int(m.len() as i64),
            o => {
                return Err(CypherError::runtime(format!(
                    "size() of a {}",
                    o.type_name()
                )))
            }
        },
        "length" => match a0 {
            Value::Null => Value::Null,
            Value::Path(p) => Value::Int(p.relationships.len() as i64),
            Value::String(s) => Value::Int(s.chars().count() as i64),
            Value::List(l) => Value::Int(l.len() as i64),
            o => {
                return Err(CypherError::runtime(format!(
                    "length() of a {}",
                    o.type_name()
                )))
            }
        },
        "isempty" => match a0 {
            Value::Null => Value::Null,
            Value::List(l) => Value::Bool(l.is_empty()),
            Value::String(s) => Value::Bool(s.is_empty()),
            Value::Map(m) => Value::Bool(m.is_empty()),
            o => {
                return Err(CypherError::runtime(format!(
                    "isEmpty() of a {}",
                    o.type_name()
                )))
            }
        },
        "head" => match a0 {
            Value::List(l) => l.into_iter().next().unwrap_or(Value::Null),
            _ => Value::Null,
        },
        "last" => match a0 {
            Value::List(l) => l.into_iter().last().unwrap_or(Value::Null),
            _ => Value::Null,
        },
        "tail" => match a0 {
            Value::List(l) => Value::List(l.into_iter().skip(1).collect()),
            Value::Null => Value::Null,
            o => {
                return Err(CypherError::runtime(format!(
                    "tail() of a {}",
                    o.type_name()
                )))
            }
        },
        "reverse" => match a0 {
            Value::List(mut l) => {
                l.reverse();
                Value::List(l)
            }
            Value::String(s) => Value::String(s.chars().rev().collect()),
            Value::Null => Value::Null,
            o => {
                return Err(CypherError::runtime(format!(
                    "reverse() of a {}",
                    o.type_name()
                )))
            }
        },
        "range" => {
            arity(name, &args, 2, 3)?;
            let strict = |v: &Value| match v {
                Value::Int(i) => Ok(Some(*i)),
                Value::Null => Ok(None),
                o => Err(CypherError::runtime(format!(
                    "range() expects integers, got {}",
                    o.type_name()
                ))),
            };
            let (Some(a), Some(b)) = (strict(&args[0])?, strict(&args[1])?) else {
                return Ok(Value::Null);
            };
            let step = match args.get(2) {
                Some(s) => strict(s)?.unwrap_or(1),
                None => 1,
            };
            if step == 0 {
                return Err(CypherError::runtime("range() step must not be zero"));
            }
            let mut out = Vec::new();
            let mut i = a;
            while (step > 0 && i <= b) || (step < 0 && i >= b) {
                out.push(Value::Int(i));
                if out.len() > 10_000_000 {
                    return Err(CypherError::runtime("range() is too large"));
                }
                i += step;
            }
            Value::List(out)
        }
        "tostring" => match a0 {
            Value::Null => Value::Null,
            Value::Node(_)
            | Value::Relationship(_)
            | Value::Path(_)
            | Value::Map(_)
            | Value::List(_) => {
                return Err(CypherError::runtime(format!(
                    "toString() of a {}",
                    a0.type_name()
                )))
            }
            v => Value::String(to_string(&v)),
        },
        "tointeger" | "toint" | "tointegerornull" => match a0 {
            Value::Int(i) => Value::Int(i),
            Value::Float(f) => {
                if f.is_finite() {
                    Value::Int(f.trunc() as i64)
                } else {
                    Value::Null
                }
            }
            Value::String(s) => s
                .trim()
                .parse::<i64>()
                .ok()
                .or_else(|| s.trim().parse::<f64>().ok().map(|f| f.trunc() as i64))
                .map_or(Value::Null, Value::Int),
            Value::Bool(b) => Value::Int(i64::from(b)),
            _ => Value::Null,
        },
        "tofloat" | "tofloatornull" => match a0 {
            Value::Int(i) => Value::Float(i as f64),
            Value::Float(f) => Value::Float(f),
            Value::String(s) => s.trim().parse::<f64>().map_or(Value::Null, Value::Float),
            _ => Value::Null,
        },
        "toboolean" | "tobooleanornull" => match a0 {
            Value::Bool(b) => Value::Bool(b),
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => Value::Null,
            },
            Value::Int(i) => Value::Bool(i != 0),
            _ => Value::Null,
        },
        "tolower" | "lower" => {
            str_arg(&a0, name)?.map_or(Value::Null, |s| Value::String(s.to_lowercase()))
        }
        "toupper" | "upper" => {
            str_arg(&a0, name)?.map_or(Value::Null, |s| Value::String(s.to_uppercase()))
        }
        "trim" => str_arg(&a0, name)?.map_or(Value::Null, |s| Value::String(s.trim().into())),
        "ltrim" => {
            str_arg(&a0, name)?.map_or(Value::Null, |s| Value::String(s.trim_start().into()))
        }
        "rtrim" => str_arg(&a0, name)?.map_or(Value::Null, |s| Value::String(s.trim_end().into())),
        "substring" => {
            arity(name, &args, 2, 3)?;
            let Some(s) = str_arg(&args[0], name)? else {
                return Ok(Value::Null);
            };
            let Some(start) = int_arg(&args[1], name)? else {
                return Ok(Value::Null);
            };
            let chars: Vec<char> = s.chars().collect();
            let start = (start.max(0) as usize).min(chars.len());
            let end = match args.get(2) {
                Some(l) => match int_arg(l, name)? {
                    Some(l) => (start + l.max(0) as usize).min(chars.len()),
                    None => return Ok(Value::Null),
                },
                None => chars.len(),
            };
            Value::String(chars[start..end].iter().collect())
        }
        "left" | "right" => {
            arity(name, &args, 2, 2)?;
            let (Some(s), Some(n)) = (str_arg(&args[0], name)?, int_arg(&args[1], name)?) else {
                return Ok(Value::Null);
            };
            let chars: Vec<char> = s.chars().collect();
            let n = (n.max(0) as usize).min(chars.len());
            Value::String(if name == "left" {
                chars[..n].iter().collect()
            } else {
                chars[chars.len() - n..].iter().collect()
            })
        }
        "replace" => {
            arity(name, &args, 3, 3)?;
            match (
                str_arg(&args[0], name)?,
                str_arg(&args[1], name)?,
                str_arg(&args[2], name)?,
            ) {
                (Some(s), Some(from), Some(to)) => Value::String(s.replace(from, to)),
                _ => Value::Null,
            }
        }
        "split" => {
            arity(name, &args, 2, 2)?;
            match (str_arg(&args[0], name)?, str_arg(&args[1], name)?) {
                (Some(s), Some(d)) => {
                    Value::List(s.split(d).map(|p| Value::String(p.to_string())).collect())
                }
                _ => Value::Null,
            }
        }
        "abs" => match a0 {
            Value::Int(i) => Value::Int(i.abs()),
            Value::Float(f) => Value::Float(f.abs()),
            Value::Null => Value::Null,
            o => {
                return Err(CypherError::runtime(format!(
                    "abs() of a {}",
                    o.type_name()
                )))
            }
        },
        "sign" => match a0 {
            Value::Int(i) => Value::Int(i.signum()),
            Value::Float(f) => Value::Int(if f > 0.0 {
                1
            } else if f < 0.0 {
                -1
            } else {
                0
            }),
            Value::Null => Value::Null,
            o => {
                return Err(CypherError::runtime(format!(
                    "sign() of a {}",
                    o.type_name()
                )))
            }
        },
        "ceil" => num_fn(&a0, name, f64::ceil)?,
        "floor" => num_fn(&a0, name, f64::floor)?,
        "round" => num_fn(&a0, name, |x| (x + 0.5).floor())?,
        "sqrt" => num_fn(&a0, name, f64::sqrt)?,
        "exp" => num_fn(&a0, name, f64::exp)?,
        "log" => num_fn(&a0, name, f64::ln)?,
        "log10" => num_fn(&a0, name, f64::log10)?,
        "sin" => num_fn(&a0, name, f64::sin)?,
        "cos" => num_fn(&a0, name, f64::cos)?,
        "tan" => num_fn(&a0, name, f64::tan)?,
        "asin" => num_fn(&a0, name, f64::asin)?,
        "acos" => num_fn(&a0, name, f64::acos)?,
        "atan" => num_fn(&a0, name, f64::atan)?,
        "degrees" => num_fn(&a0, name, f64::to_degrees)?,
        "radians" => num_fn(&a0, name, f64::to_radians)?,
        "atan2" => match (
            args.first().and_then(Value::as_f64),
            args.get(1).and_then(Value::as_f64),
        ) {
            (Some(y), Some(x)) => Value::Float(y.atan2(x)),
            _ => Value::Null,
        },
        "rand" => Value::Float(random_f64()),
        "randomuuid" => {
            let h = oxrdf::BlankNode::default().as_str().to_string();
            let h = format!("{h:0>32}");
            Value::String(format!(
                "{}-{}-{}-{}-{}",
                &h[0..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..32]
            ))
        }
        "pi" => Value::Float(std::f64::consts::PI),
        "e" => Value::Float(std::f64::consts::E),
        "date" | "datetime" | "localdatetime" | "time" | "localtime" | "duration" => {
            arity(name, &args, 0, 1)?;
            crate::temporal::construct(temporal_kind(name), args.first())?
        }
        "date.truncate"
        | "datetime.truncate"
        | "localdatetime.truncate"
        | "time.truncate"
        | "localtime.truncate" => {
            arity(name, &args, 2, 3)?;
            let kind = temporal_kind(name.split('.').next().unwrap_or(""));
            let Some(unit) = str_arg(&args[0], name)? else {
                return Ok(Value::Null);
            };
            let Some(t) = crate::temporal::T::from_value(&args[1])? else {
                if args[1].is_null() {
                    return Ok(Value::Null);
                }
                return Err(CypherError::runtime(format!(
                    "{name}() needs a temporal value"
                )));
            };
            let overrides = match args.get(2) {
                Some(Value::Map(m)) => Some(m.clone()),
                Some(Value::Null) | None => None,
                Some(o) => {
                    return Err(CypherError::runtime(format!(
                        "{name}() expects a map, got {}",
                        o.type_name()
                    )))
                }
            };
            crate::temporal::truncate(kind, unit, &t, overrides.as_ref())?
        }
        "duration.between" | "duration.inmonths" | "duration.indays" | "duration.inseconds" => {
            arity(name, &args, 2, 2)?;
            match (
                crate::temporal::T::from_value(&args[0])?,
                crate::temporal::T::from_value(&args[1])?,
            ) {
                (Some(a), Some(b)) => crate::temporal::between(&name[9..], &a, &b)?,
                _ if args[0].is_null() || args[1].is_null() => Value::Null,
                _ => {
                    return Err(CypherError::runtime(format!(
                        "{name}() needs temporal values"
                    )))
                }
            }
        }
        n if n.ends_with(".statement")
            || n.ends_with(".transaction")
            || n.ends_with(".realtime") =>
        {
            let base = n.split('.').next().unwrap_or("");
            if !matches!(
                base,
                "date" | "datetime" | "localdatetime" | "time" | "localtime"
            ) {
                return Err(CypherError::unsupported(format!("function {n}()")));
            }
            if args.first().is_some_and(Value::is_null) {
                return Ok(Value::Null);
            }
            crate::temporal::construct(temporal_kind(base), None)?
        }
        "datetime.fromepoch" => {
            arity(name, &args, 2, 2)?;
            match (int_arg(&args[0], name)?, int_arg(&args[1], name)?) {
                (Some(s), Some(n)) => crate::temporal::from_epoch(s, n)?,
                _ => Value::Null,
            }
        }
        "datetime.fromepochmillis" => {
            arity(name, &args, 1, 1)?;
            match int_arg(&args[0], name)? {
                Some(ms) => crate::temporal::from_epoch(
                    ms.div_euclid(1000),
                    ms.rem_euclid(1000) * 1_000_000,
                )?,
                None => Value::Null,
            }
        }
        "valuetype" => Value::String(a0.type_name().to_string()),
        other => return Err(CypherError::unsupported(format!("function {other}()"))),
    })
}

fn temporal_kind(name: &str) -> TemporalKind {
    match name {
        "date" => TemporalKind::Date,
        "datetime" => TemporalKind::DateTime,
        "localdatetime" => TemporalKind::LocalDateTime,
        "time" => TemporalKind::Time,
        "localtime" => TemporalKind::LocalTime,
        _ => TemporalKind::Duration,
    }
}

fn random_f64() -> f64 {
    use std::cell::Cell;
    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0) };
    }
    STATE.with(|st| {
        let mut x = st.get();
        if x == 0 {
            // Seeded from the platform's randomness (as for fresh blank nodes).
            let h = oxrdf::BlankNode::default();
            x = u64::from_str_radix(&h.as_str()[..h.as_str().len().min(15)], 16)
                .unwrap_or(0x9E37_79B9_7F4A_7C15)
                | 1;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        st.set(x);
        (x >> 11) as f64 / (1u64 << 53) as f64
    })
}

/// A running aggregate.
#[derive(Debug, Clone)]
pub(crate) enum Acc {
    Count(i64),
    Sum {
        int: i64,
        float: f64,
        is_float: bool,
    },
    Avg {
        sum: f64,
        n: i64,
    },
    Min(Option<Value>),
    Max(Option<Value>),
    Collect(Vec<Value>),
    StDev {
        values: Vec<f64>,
        population: bool,
    },
    Percentile {
        values: Vec<f64>,
        p: f64,
        cont: bool,
    },
}

impl Acc {
    pub fn new(name: &str) -> Result<Self> {
        Ok(match name {
            "count" => Self::Count(0),
            "sum" => Self::Sum {
                int: 0,
                float: 0.0,
                is_float: false,
            },
            "avg" => Self::Avg { sum: 0.0, n: 0 },
            "min" => Self::Min(None),
            "max" => Self::Max(None),
            "collect" => Self::Collect(Vec::new()),
            "stdev" => Self::StDev {
                values: Vec::new(),
                population: false,
            },
            "stdevp" => Self::StDev {
                values: Vec::new(),
                population: true,
            },
            "percentilecont" | "percentiledisc" => Self::Percentile {
                values: Vec::new(),
                p: 0.5,
                cont: name == "percentilecont",
            },
            other => return Err(CypherError::unsupported(format!("aggregate {other}()"))),
        })
    }

    pub fn add(&mut self, v: Value) -> Result<()> {
        if v.is_null() {
            return Ok(());
        }
        match self {
            Self::Count(n) => *n += 1,
            Self::Sum {
                int,
                float,
                is_float,
            } => match v {
                Value::Int(i) => {
                    *int = int
                        .checked_add(i)
                        .ok_or_else(|| CypherError::runtime("integer overflow in sum()"))?;
                }
                Value::Float(f) => {
                    *float += f;
                    *is_float = true;
                }
                o => {
                    return Err(CypherError::runtime(format!(
                        "sum() of a {}",
                        o.type_name()
                    )))
                }
            },
            Self::Avg { sum, n } => match v.as_f64() {
                Some(f) => {
                    *sum += f;
                    *n += 1;
                }
                None => {
                    return Err(CypherError::runtime(format!(
                        "avg() of a {}",
                        v.type_name()
                    )))
                }
            },
            Self::Min(cur) => {
                if cur.as_ref().is_none_or(|c| v.order(c) == Ordering::Less) {
                    *cur = Some(v);
                }
            }
            Self::Max(cur) => {
                if cur.as_ref().is_none_or(|c| v.order(c) == Ordering::Greater) {
                    *cur = Some(v);
                }
            }
            Self::Collect(items) => items.push(v),
            Self::Percentile { values, .. } => match v.as_f64() {
                Some(f) => values.push(f),
                None => {
                    return Err(CypherError::runtime(format!(
                        "percentile of a {}",
                        v.type_name()
                    )))
                }
            },
            Self::StDev { values, .. } => match v.as_f64() {
                Some(f) => values.push(f),
                None => {
                    return Err(CypherError::runtime(format!(
                        "stDev() of a {}",
                        v.type_name()
                    )))
                }
            },
        }
        Ok(())
    }

    pub fn finish(self) -> Value {
        match self {
            Self::Count(n) => Value::Int(n),
            Self::Sum {
                int,
                float,
                is_float,
            } => {
                if is_float {
                    Value::Float(float + int as f64)
                } else {
                    Value::Int(int)
                }
            }
            Self::Avg { sum, n } => {
                if n == 0 {
                    Value::Null
                } else {
                    Value::Float(sum / n as f64)
                }
            }
            Self::Min(v) | Self::Max(v) => v.unwrap_or(Value::Null),
            Self::Collect(items) => Value::List(items),
            Self::Percentile {
                mut values,
                p,
                cont,
            } => {
                if values.is_empty() {
                    return Value::Null;
                }
                values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let n = values.len();
                if cont {
                    let pos = p * (n - 1) as f64;
                    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
                    Value::Float(values[lo] + (values[hi] - values[lo]) * (pos - lo as f64))
                } else {
                    let idx = ((p * n as f64).ceil() as usize)
                        .saturating_sub(1)
                        .min(n - 1);
                    let x = values[idx];
                    if x.fract() == 0.0 && x.abs() < 9.0e15 {
                        Value::Int(x as i64)
                    } else {
                        Value::Float(x)
                    }
                }
            }
            Self::StDev { values, population } => {
                let n = values.len() as f64;
                let d = if population { n } else { n - 1.0 };
                if d <= 0.0 {
                    return Value::Float(0.0);
                }
                let mean = values.iter().sum::<f64>() / n;
                Value::Float((values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / d).sqrt())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn ev(src: &str) -> Value {
        let q = parse(&format!("RETURN {src}")).unwrap();
        let crate::ast::Clause::Return(p) = &q.parts[0].clauses[0] else {
            panic!()
        };
        let row = Row::new();
        eval(&p.items[0].expr, &Env::new(&row, &Params::new())).unwrap()
    }

    #[test]
    fn null_semantics() {
        assert_eq!(ev("null = null"), Value::Null);
        assert_eq!(ev("null OR true"), Value::Bool(true));
        assert_eq!(ev("null AND false"), Value::Bool(false));
        assert_eq!(ev("2 IN [1, null]"), Value::Null);
        assert_eq!(ev("1 IN [1, null]"), Value::Bool(true));
        assert_eq!(ev("1 = 1.0"), Value::Bool(true));
    }

    #[test]
    fn arithmetic_and_strings() {
        assert_eq!(ev("7 / 2"), Value::Int(3));
        assert_eq!(ev("7.0 / 2"), Value::Float(3.5));
        assert_eq!(ev("'a' + 1"), Value::String("a1".into()));
        assert_eq!(
            ev("[1] + 2"),
            Value::List(vec![Value::Int(1), Value::Int(2)])
        );
        assert_eq!(ev("substring('hello', 1, 3)"), Value::String("ell".into()));
        assert_eq!(ev("'abc' =~ 'a.*'"), Value::Bool(true));
    }

    #[test]
    fn lists() {
        assert_eq!(
            ev("[x IN range(1, 5) WHERE x % 2 = 1 | x * 10]"),
            Value::List(vec![Value::Int(10), Value::Int(30), Value::Int(50)])
        );
        assert_eq!(ev("reduce(s = 0, x IN [1, 2, 3] | s + x)"), Value::Int(6));
        assert_eq!(ev("[1, 2, 3][-1]"), Value::Int(3));
        assert_eq!(
            ev("[1, 2, 3][1..]"),
            Value::List(vec![Value::Int(2), Value::Int(3)])
        );
        assert_eq!(ev("all(x IN [1, 2] WHERE x > 0)"), Value::Bool(true));
    }
}
