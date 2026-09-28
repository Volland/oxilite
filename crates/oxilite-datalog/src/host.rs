//! Host functions in Datalog: rules that call application code.
//!
//! Rules compile to SQL, and SQL cannot call Rust on every backend (Turso keeps scalar-function
//! registration private, D1 has none). A rule that calls a host function — as an expression
//! (`?s = fn:slugify(?n)`, `fn:score(?x) > 0.5`) or as an atom whose predicate is the function
//! (`fn:slugify(?n, ?s)`: the last argument is the result; `fn:isLong(?n)`: a filter) — is split:
//!
//! 1. its host-free part becomes an auxiliary rule over the variables the store binds, run as
//!    SQL with the rules it depends on;
//! 2. the calls are applied to each row in Rust, left to right, binding and filtering;
//! 3. the head tuples that survive replace the rule as facts, so later rules, negation and
//!    aggregation read them like any relation.
//!
//! Relations are resolved in dependency order, one request per host rule. A host rule that
//! depends on its own head is rejected: its facts would have to exist before it runs. A goal's
//! host constraints filter its rows after the program has run.
//!
// @lat: [[architecture#Host functions#Datalog]]

use crate::ast::{
    atom_vars, expr_vars, Arg, Atom, BinOp, BodyItem, Expr, Goal, Head, HeadArg, Pred, Program,
    Rule,
};
use crate::error::{DatalogError, Result};
use crate::exec::{DatalogJob, DatalogResult};
use crate::sql::Options;
use oxilite_core::functions::Functions;
use oxilite_core::job::{Job, Step};
use oxilite_core::sql::{Capabilities, Response};
use oxrdf::vocab::xsd;
use oxrdf::{Literal, Term};
use std::collections::{BTreeSet, HashMap, VecDeque};

/// Does the program call a host function (or name an IRI function that is not registered)?
pub fn uses_host_functions(program: &Program, functions: &Functions) -> Result<bool> {
    let mut any = false;
    let visit = |e: &Expr, any: &mut bool| -> Result<()> {
        walk_calls(e, &mut |name| {
            if is_iri(name) {
                if functions.get(name).is_none() {
                    return Err(DatalogError::Unsupported(format!(
                        "unknown function <{name}>: register it as a host function"
                    )));
                }
                *any = true;
            }
            Ok(())
        })
    };
    for rule in &program.rules {
        for item in &rule.body {
            match item {
                BodyItem::Constraint(e) => visit(e, &mut any)?,
                BodyItem::Atom(a) => any |= is_host_atom(a, functions),
                BodyItem::Negated(a) if is_host_atom(a, functions) => {
                    return Err(DatalogError::Unsupported(format!(
                        "host function {} cannot be negated; negate a constraint that calls it",
                        a.pred
                    )))
                }
                BodyItem::Negated(_) => {}
            }
        }
    }
    if let Some(g) = &program.goal {
        for c in &g.constraints {
            visit(c, &mut any)?;
        }
    }
    Ok(any)
}

fn is_iri(name: &str) -> bool {
    name.contains(':')
}

fn walk_calls(e: &Expr, f: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
    match e {
        Expr::Call { name, args } => {
            f(name)?;
            for a in args {
                walk_calls(a, f)?;
            }
        }
        Expr::Binary { left, right, .. } => {
            walk_calls(left, f)?;
            walk_calls(right, f)?;
        }
        Expr::Not(x) | Expr::Neg(x) => walk_calls(x, f)?,
        Expr::Var(_) | Expr::Const(_) => {}
    }
    Ok(())
}

fn calls_host(e: &Expr) -> bool {
    let mut found = false;
    let _ = walk_calls(e, &mut |n| {
        found |= is_iri(n);
        Ok(())
    });
    found
}

fn is_host_atom(a: &Atom, functions: &Functions) -> bool {
    matches!(&a.pred, Pred::Edb(iri) if functions.get(iri.as_str()).is_some())
}

fn is_host_rule(rule: &Rule, functions: &Functions) -> bool {
    rule.body.iter().any(|i| match i {
        BodyItem::Atom(a) => is_host_atom(a, functions),
        BodyItem::Constraint(e) => calls_host(e),
        BodyItem::Negated(_) => false,
    })
}

/// The derived relations a rule body reads.
fn body_preds(rule: &Rule) -> Vec<Pred> {
    rule.body
        .iter()
        .filter_map(|i| match i {
            BodyItem::Atom(a) | BodyItem::Negated(a) if matches!(a.pred, Pred::Idb(_)) => {
                Some(a.pred.clone())
            }
            _ => None,
        })
        .collect()
}

/// Every derived relation `preds` read, transitively (themselves included).
fn closure(program: &Program, preds: Vec<Pred>) -> BTreeSet<Pred> {
    let mut seen: BTreeSet<Pred> = BTreeSet::new();
    let mut todo = preds;
    while let Some(p) = todo.pop() {
        if !seen.insert(p.clone()) {
            continue;
        }
        for r in program.rules_for(&p) {
            todo.extend(body_preds(r));
        }
    }
    seen
}

/// A program with host rules, run as a sequence of Datalog jobs.
pub struct HostJob {
    caps: Capabilities,
    options: Options,
    program: Program,
    /// Host rules still to resolve (indices into `program.rules`), in dependency order.
    queue: VecDeque<usize>,
    /// Facts derived by resolved host rules, by rule index.
    facts: HashMap<usize, Vec<Rule>>,
    /// The auxiliary run of the host rule being resolved, or the final run.
    current: Option<(Option<usize>, DatalogJob, Vec<String>)>,
    goal_filters: Vec<Expr>,
    rounds: Vec<usize>,
}

impl HostJob {
    pub fn new(program: Program, caps: &Capabilities, options: &Options) -> Result<Self> {
        let functions = &options.functions;
        let host: Vec<usize> = (0..program.rules.len())
            .filter(|&i| is_host_rule(&program.rules[i], functions))
            .collect();
        // Order: a host rule runs once every host rule it reads (transitively) has.
        let deps: HashMap<usize, BTreeSet<Pred>> = host
            .iter()
            .map(|&i| (i, closure(&program, body_preds(&program.rules[i]))))
            .collect();
        for &i in &host {
            let rule = &program.rules[i];
            if rule.head.aggregates() {
                return Err(DatalogError::Unsupported(format!(
                    "the rule for {} aggregates and calls a host function; aggregate in a rule over its result instead",
                    rule.head.pred
                )));
            }
            if deps[&i].contains(&rule.head.pred) {
                return Err(DatalogError::Unsupported(format!(
                    "a rule calling a host function cannot be recursive: the rule for {} depends on itself",
                    rule.head.pred
                )));
            }
        }
        let mut queue = VecDeque::new();
        let mut left: Vec<usize> = host.clone();
        while !left.is_empty() {
            let ready = left.iter().position(|&i| {
                !left
                    .iter()
                    .any(|&j| j != i && deps[&i].contains(&program.rules[j].head.pred))
            });
            let Some(pos) = ready else {
                return Err(DatalogError::Unsupported(
                    "rules calling host functions depend on each other in a cycle".into(),
                ));
            };
            queue.push_back(left.remove(pos));
        }
        let mut goal_filters = Vec::new();
        let mut program = program;
        if let Some(g) = &mut program.goal {
            let (host_c, sql_c): (Vec<Expr>, Vec<Expr>) = std::mem::take(&mut g.constraints)
                .into_iter()
                .partition(calls_host);
            g.constraints = sql_c;
            goal_filters = host_c;
        }
        Ok(Self {
            caps: caps.clone(),
            options: options.clone(),
            program,
            queue,
            facts: HashMap::new(),
            current: None,
            goal_filters,
            rounds: Vec::new(),
        })
    }

    /// The program with resolved host rules replaced by their facts, and unresolved ones left
    /// out.
    fn resolved_program(&self) -> Program {
        let mut rules = Vec::new();
        for (i, r) in self.program.rules.iter().enumerate() {
            match self.facts.get(&i) {
                Some(facts) => rules.extend(facts.iter().cloned()),
                None if self.queue.contains(&i) => {}
                None => rules.push(r.clone()),
            }
        }
        Program {
            rules,
            goal: self.program.goal.clone(),
            version: self.program.version.clone(),
        }
    }

    fn compile(&self, program: &Program) -> Result<DatalogJob> {
        let analysis = crate::program::analyse(program)?;
        let compiled = crate::sql::compile(program, &analysis, &self.caps, &self.options)?;
        Ok(DatalogJob::new(compiled, self.caps.clone()))
    }

    /// Starts the next run: the auxiliary rule of the next host rule, or the whole program.
    fn next(&mut self) -> Result<Option<Step<DatalogResult>>> {
        if let Some(i) = self.queue.front().copied() {
            let rule = self.program.rules[i].clone();
            let (sql_body, vars) = split(&rule, &self.options.functions);
            if vars.is_empty() {
                // Nothing to read: the calls run once, over no bindings.
                self.queue.pop_front();
                let facts = self.apply(&rule, i, &[], &[Vec::new()])?;
                self.facts.insert(i, facts);
                return Ok(None);
            }
            let aux = Pred::Idb("__host_rule".to_owned());
            let span = rule.head.span;
            let aux_rule = Rule {
                head: Head {
                    pred: aux.clone(),
                    args: vars
                        .iter()
                        .map(|v| HeadArg::Plain(Arg::Var(v.clone())))
                        .collect(),
                    span,
                },
                body: sql_body,
            };
            let base = self.resolved_program();
            let needed = closure(&base, body_preds(&aux_rule));
            let mut rules: Vec<Rule> = base
                .rules
                .into_iter()
                .filter(|r| needed.contains(&r.head.pred))
                .collect();
            rules.push(aux_rule);
            let program = Program {
                rules,
                goal: Some(Goal {
                    atom: Atom {
                        pred: aux,
                        args: vars.iter().map(|v| Arg::Var(v.clone())).collect(),
                        span,
                        at: None,
                    },
                    constraints: Vec::new(),
                }),
                version: self.program.version.clone(),
            };
            let job = self.compile(&program)?;
            self.current = Some((Some(i), job, vars));
        } else {
            let job = self.compile(&self.resolved_program())?;
            self.current = Some((None, job, Vec::new()));
        }
        Ok(None)
    }

    /// The facts a host rule derives from the rows of its auxiliary rule.
    fn apply(
        &self,
        rule: &Rule,
        index: usize,
        vars: &[String],
        rows: &[Vec<Option<Term>>],
    ) -> Result<Vec<Rule>> {
        let functions = &self.options.functions;
        let rust: Vec<&BodyItem> = rule
            .body
            .iter()
            .filter(|i| !is_sql_item(i, rule, functions))
            .collect();
        let mut seen = BTreeSet::new();
        let mut facts = Vec::new();
        for row in rows {
            let mut env: HashMap<String, Term> = HashMap::new();
            for (v, t) in vars.iter().zip(row) {
                if let Some(t) = t {
                    env.insert(v.clone(), t.clone());
                }
            }
            if !run_items(&rust, &mut env, functions)? {
                continue;
            }
            let mut tuple = Vec::with_capacity(rule.head.args.len());
            for a in &rule.head.args {
                tuple.push(match a {
                    HeadArg::Plain(Arg::Var(v)) => {
                        env.get(v).cloned().ok_or_else(|| DatalogError::Unsafe {
                            predicate: rule.head.pred.to_string(),
                            variable: format!("?{v}"),
                        })?
                    }
                    HeadArg::Plain(Arg::Const(t)) => t.clone(),
                    _ => {
                        return Err(DatalogError::Unsupported(format!(
                            "the head of {} must be variables and constants",
                            rule.head.pred
                        )))
                    }
                });
            }
            if seen.insert(tuple.iter().map(Term::to_string).collect::<Vec<_>>()) {
                facts.push(fact(&rule.head, tuple));
            }
        }
        if facts.is_empty() {
            facts.push(empty_relation(&rule.head));
        }
        let _ = index;
        Ok(facts)
    }
}

fn fact(head: &Head, tuple: Vec<Term>) -> Rule {
    Rule {
        head: Head {
            pred: head.pred.clone(),
            args: tuple
                .into_iter()
                .map(|t| HeadArg::Plain(Arg::Const(t)))
                .collect(),
            span: head.span,
        },
        body: Vec::new(),
    }
}

/// A rule that keeps the relation defined (so later rules may read it) and derives nothing.
fn empty_relation(head: &Head) -> Rule {
    let mut rule = fact(
        head,
        vec![Literal::new_simple_literal("").into(); head.args.len()],
    );
    rule.body.push(BodyItem::Constraint(Expr::Const(
        Literal::from(false).into(),
    )));
    rule
}

/// Is this body item evaluated by SQL (rather than in Rust)?
fn is_sql_item(item: &BodyItem, rule: &Rule, functions: &Functions) -> bool {
    match item {
        BodyItem::Atom(a) => !is_host_atom(a, functions),
        BodyItem::Negated(_) => true,
        BodyItem::Constraint(e) => {
            let bound = sql_bound(rule, functions);
            let mut vars = Vec::new();
            expr_vars(e, &mut vars);
            !calls_host(e) && vars.iter().all(|v| bound.contains(v))
        }
    }
}

/// Variables the store binds: those of the positive atoms that are not host calls.
fn sql_bound(rule: &Rule, functions: &Functions) -> Vec<String> {
    let mut vars = Vec::new();
    for item in &rule.body {
        if let BodyItem::Atom(a) = item {
            if !is_host_atom(a, functions) {
                atom_vars(a, &mut vars);
            }
        }
    }
    vars
}

/// The SQL part of a host rule, and the variables it binds (the auxiliary rule's columns).
fn split(rule: &Rule, functions: &Functions) -> (Vec<BodyItem>, Vec<String>) {
    let vars = sql_bound(rule, functions);
    let body = rule
        .body
        .iter()
        .filter(|i| is_sql_item(i, rule, functions))
        .cloned()
        .collect();
    (body, vars)
}

/// Applies the Rust-evaluated items to one row; `false` drops the row.
fn run_items(
    items: &[&BodyItem],
    env: &mut HashMap<String, Term>,
    functions: &Functions,
) -> Result<bool> {
    for item in items {
        match item {
            BodyItem::Atom(a) => {
                let Pred::Edb(iri) = &a.pred else {
                    unreachable!("only host atoms run in Rust")
                };
                let f = functions.get(iri.as_str()).expect("a host atom");
                let (inputs, output) = if a.args.len() == 1 {
                    (&a.args[..], None)
                } else {
                    (&a.args[..a.args.len() - 1], a.args.last())
                };
                let mut terms = Vec::with_capacity(inputs.len());
                for arg in inputs {
                    terms.push(match arg {
                        Arg::Const(t) => t.clone(),
                        Arg::Var(v) => env
                            .get(v)
                            .cloned()
                            .ok_or_else(|| unbound(v, iri.as_str()))?,
                        Arg::Wildcard => {
                            return Err(DatalogError::Unsupported(format!(
                                "`_` cannot be an input of host function <{iri}>"
                            )))
                        }
                    });
                }
                let Some(result) = f.call(&terms) else {
                    return Ok(false);
                };
                match output {
                    None => {
                        if !ebv(&result).unwrap_or(false) {
                            return Ok(false);
                        }
                    }
                    Some(Arg::Wildcard) => {}
                    Some(Arg::Const(t)) => {
                        if !same(&result, t) {
                            return Ok(false);
                        }
                    }
                    Some(Arg::Var(v)) => match env.get(v) {
                        Some(t) if !same(&result, t) => return Ok(false),
                        Some(_) => {}
                        None => {
                            env.insert(v.clone(), result);
                        }
                    },
                }
            }
            BodyItem::Constraint(e) => {
                // `?v = expr` with `?v` not yet bound is an assignment.
                if let Expr::Binary {
                    op: BinOp::Eq,
                    left,
                    right,
                } = e
                {
                    let target = match (&**left, &**right) {
                        (Expr::Var(v), other) | (other, Expr::Var(v)) if !env.contains_key(v) => {
                            Some((v, other))
                        }
                        _ => None,
                    };
                    if let Some((v, other)) = target {
                        match eval(other, env, functions)? {
                            Some(t) => {
                                env.insert(v.clone(), t);
                                continue;
                            }
                            None => return Ok(false),
                        }
                    }
                }
                match eval(e, env, functions)? {
                    Some(t) if ebv(&t) == Some(true) => {}
                    _ => return Ok(false),
                }
            }
            BodyItem::Negated(_) => unreachable!("negation runs in SQL"),
        }
    }
    Ok(true)
}

fn unbound(v: &str, f: &str) -> DatalogError {
    DatalogError::Unsupported(format!(
        "?{v} is not bound when host function <{f}> is called: bind it with an atom or an earlier call"
    ))
}

/// Evaluates an expression over bound terms; `None` is an evaluation error (the row is dropped).
fn eval(e: &Expr, env: &HashMap<String, Term>, functions: &Functions) -> Result<Option<Term>> {
    Ok(match e {
        Expr::Var(v) => Some(
            env.get(v)
                .cloned()
                .ok_or_else(|| DatalogError::Unsupported(format!("?{v} is not bound")))?,
        ),
        Expr::Const(t) => Some(t.clone()),
        Expr::Call { name, args } => {
            let Some(f) = functions.get(name) else {
                return Err(DatalogError::Unsupported(format!(
                    "{name} cannot be evaluated with a host function in one constraint; give it a constraint of its own"
                )));
            };
            let mut terms = Vec::with_capacity(args.len());
            for a in args {
                match eval(a, env, functions)? {
                    Some(t) => terms.push(t),
                    None => return Ok(None),
                }
            }
            f.call(&terms)
        }
        Expr::Not(x) => eval(x, env, functions)?
            .and_then(|t| ebv(&t))
            .map(|b| Literal::from(!b).into()),
        Expr::Neg(x) => eval(x, env, functions)?
            .and_then(|t| number(&t))
            .map(|n| num_term(-n.0, n.1)),
        Expr::Binary { op, left, right } => {
            let (Some(a), Some(b)) = (eval(left, env, functions)?, eval(right, env, functions)?)
            else {
                return Ok(None);
            };
            binary(*op, &a, &b)
        }
    })
}

/// A number and whether it is an integer.
fn number(t: &Term) -> Option<(f64, bool)> {
    let Term::Literal(l) = t else { return None };
    let rank = oxilite_core::encoding::numeric_rank(l.datatype().as_str())?;
    Some((l.value().parse().ok()?, rank == 1))
}

fn num_term(v: f64, integer: bool) -> Term {
    if integer && v.fract() == 0.0 && v.abs() < 9e15 {
        Literal::from(v as i64).into()
    } else {
        Literal::from(v).into()
    }
}

fn ebv(t: &Term) -> Option<bool> {
    let Term::Literal(l) = t else { return None };
    if l.datatype() == xsd::BOOLEAN {
        return Some(matches!(l.value(), "true" | "1"));
    }
    if let Some((n, _)) = number(t) {
        return Some(n != 0.0 && !n.is_nan());
    }
    if l.datatype() == xsd::STRING || l.language().is_some() {
        return Some(!l.value().is_empty());
    }
    None
}

/// RDF term equality, with numbers compared by value.
fn same(a: &Term, b: &Term) -> bool {
    match (number(a), number(b)) {
        (Some((x, _)), Some((y, _))) => x == y,
        _ => a == b,
    }
}

fn binary(op: BinOp, a: &Term, b: &Term) -> Option<Term> {
    use std::cmp::Ordering;
    let bool_term = |v: bool| -> Option<Term> { Some(Literal::from(v).into()) };
    match op {
        BinOp::And => bool_term(ebv(a)? && ebv(b)?),
        BinOp::Or => bool_term(ebv(a)? || ebv(b)?),
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
            let ((x, xi), (y, yi)) = (number(a)?, number(b)?);
            let v = match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                _ if y == 0.0 => return None,
                _ => x / y,
            };
            Some(num_term(v, xi && yi && op != BinOp::Div))
        }
        BinOp::Eq => bool_term(same(a, b)),
        BinOp::Ne => bool_term(!same(a, b)),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ord = match (number(a), number(b), a, b) {
                (Some((x, _)), Some((y, _)), _, _) => x.partial_cmp(&y)?,
                (_, _, Term::Literal(x), Term::Literal(y)) if x.datatype() == y.datatype() => {
                    x.value().cmp(y.value())
                }
                _ => return None,
            };
            bool_term(match op {
                BinOp::Lt => ord == Ordering::Less,
                BinOp::Le => ord != Ordering::Greater,
                BinOp::Gt => ord == Ordering::Greater,
                _ => ord != Ordering::Less,
            })
        }
    }
}

impl Job for HostJob {
    type Output = DatalogResult;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<DatalogResult>> {
        self.step_inner(response).map_err(|e| match e {
            DatalogError::Store(e) => e,
            other => oxilite_core::Error::Other(other.to_string()),
        })
    }
}

impl HostJob {
    fn step_inner(&mut self, mut response: Option<Response>) -> Result<Step<DatalogResult>> {
        loop {
            if self.current.is_none() {
                self.next()?;
                if self.current.is_none() {
                    // A host rule that read nothing was resolved in place.
                    continue;
                }
                response = None;
            }
            let (which, job, vars) = self.current.as_mut().expect("a run is current");
            match job.step(response.take())? {
                Step::Execute(r) => return Ok(Step::Execute(r)),
                Step::Done(result) => {
                    let which = *which;
                    let vars = vars.clone();
                    self.current = None;
                    self.rounds.extend(result.rounds.iter().copied());
                    match which {
                        Some(i) => {
                            self.queue.pop_front();
                            let rule = self.program.rules[i].clone();
                            let facts = self.apply(&rule, i, &vars, &result.rows)?;
                            self.facts.insert(i, facts);
                        }
                        None => return Ok(Step::Done(self.finish(result)?)),
                    }
                }
            }
        }
    }

    fn finish(&self, mut result: DatalogResult) -> Result<DatalogResult> {
        if !self.goal_filters.is_empty() {
            let items: Vec<BodyItem> = self
                .goal_filters
                .iter()
                .cloned()
                .map(BodyItem::Constraint)
                .collect();
            let refs: Vec<&BodyItem> = items.iter().collect();
            let mut kept = Vec::new();
            for row in result.rows {
                let mut env: HashMap<String, Term> = HashMap::new();
                for (v, t) in result.variables.iter().zip(&row) {
                    if let Some(t) = t {
                        env.insert(v.clone(), t.clone());
                    }
                }
                if run_items(&refs, &mut env, &self.options.functions)? {
                    kept.push(row);
                }
            }
            result.rows = kept;
        }
        result.rounds = self.rounds.clone();
        Ok(result)
    }
}
