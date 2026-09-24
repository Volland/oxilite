//! Program analysis: dependency graph, strongly connected components, stratification,
//! safety, and the recursion shape of each component.
//!
//! The checks here run before any SQL exists, so an unstratified or unsafe program is
//! rejected with the offending cycle or variable named rather than with a SQLite error.
//!
// @lat: [[architecture#Datalog frontend#Stratification]]

use crate::ast::*;
use crate::error::{DatalogError, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// How one predicate depends on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dep {
    Positive,
    Negative,
    Aggregating,
}

/// The recursion shape of a component, which decides how it is evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// No rule of the component refers to the component.
    NonRecursive,
    /// Every recursive rule has exactly one recursive body atom, and the component defines
    /// one predicate: a single `WITH RECURSIVE` common table expression.
    Linear,
    /// Linear, but the component defines several predicates: one tagged CTE.
    LinearMutual,
    /// Some rule has two or more recursive body atoms.
    NonLinear,
}

/// One stratum: a component of the dependency graph, with its evaluation shape.
#[derive(Debug, Clone)]
pub struct Stratum {
    /// The predicates of this component, in a stable order.
    pub preds: Vec<Pred>,
    pub shape: Shape,
}

impl Stratum {
    /// Is this component evaluated recursively?
    pub fn is_recursive(&self) -> bool {
        self.shape != Shape::NonRecursive
    }
}

/// A program that has passed every check, with its strata in evaluation order.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub strata: Vec<Stratum>,
    /// The arity of every derived predicate.
    pub arity: HashMap<Pred, usize>,
}

impl Analysis {
    /// The stratum index a predicate is computed in.
    pub fn stratum_of(&self, pred: &Pred) -> Option<usize> {
        self.strata.iter().position(|s| s.preds.contains(pred))
    }
}

/// Checks a program and orders its strata.
pub fn analyse(program: &Program) -> Result<Analysis> {
    let arity = arities(program)?;
    check_safety(program)?;
    let (nodes, edges) = dependency_graph(program);
    let sccs = tarjan(nodes.len(), &edges);
    let owner = component_of(nodes.len(), &sccs);
    check_stratification(&nodes, &sccs, &owner, &edges)?;

    let mut strata = Vec::with_capacity(sccs.len());
    for ci in topological(&sccs, &owner, &edges) {
        let preds: Vec<Pred> = sccs[ci].iter().map(|&v| nodes[v].clone()).collect();
        let shape = shape_of(program, &preds);
        strata.push(Stratum { preds, shape });
    }
    Ok(Analysis { strata, arity })
}

/// Records the arity of every predicate a rule defines, and checks that every use agrees.
///
/// A predicate is derived when a rule gives it a head — including when it is written as an
/// IRI, so `ex:ancestor(?x, ?y) :- …` defines a relation rather than reading one. Stored
/// relations are addressed, not defined, so the same IRI may be a class in one atom and a
/// predicate in another.
fn arities(program: &Program) -> Result<HashMap<Pred, usize>> {
    let mut arity: HashMap<Pred, usize> = HashMap::new();
    for rule in &program.rules {
        let n = rule.head.args.len();
        if let Some(&known) = arity.get(&rule.head.pred) {
            if known != n {
                return Err(DatalogError::Arity {
                    predicate: rule.head.pred.to_string(),
                    expected: known,
                    found: n,
                });
            }
        } else {
            arity.insert(rule.head.pred.clone(), n);
        }
    }
    let check = |pred: &Pred, n: usize| -> Result<()> {
        match arity.get(pred) {
            Some(&known) if known != n => Err(DatalogError::Arity {
                predicate: pred.to_string(),
                expected: known,
                found: n,
            }),
            _ => Ok(()),
        }
    };
    for rule in &program.rules {
        for item in &rule.body {
            match item {
                BodyItem::Atom(a) | BodyItem::Negated(a) => check(&a.pred, a.args.len())?,
                BodyItem::Constraint(_) => {}
            }
        }
    }
    if let Some(goal) = &program.goal {
        check(&goal.atom.pred, goal.atom.args.len())?;
    }
    Ok(arity)
}

/// Every head, negated and constraint variable must be bound by a positive body atom.
fn check_safety(program: &Program) -> Result<()> {
    for rule in &program.rules {
        let mut bound = Vec::new();
        for atom in rule.positive() {
            atom_vars(atom, &mut bound);
        }
        let name = rule.head.pred.to_string();
        let require = |v: &String| -> Result<()> {
            if bound.contains(v) {
                Ok(())
            } else {
                Err(DatalogError::Unsafe {
                    predicate: name.clone(),
                    variable: format!("?{v}"),
                })
            }
        };
        for arg in &rule.head.args {
            match arg {
                HeadArg::Plain(Arg::Var(v)) => require(v)?,
                HeadArg::Agg { var, .. } if var != "*" => require(var)?,
                _ => {}
            }
        }
        for atom in rule.negated() {
            let mut vars = Vec::new();
            atom_vars(atom, &mut vars);
            for v in &vars {
                require(v)?;
            }
        }
        for c in rule.constraints() {
            let mut vars = Vec::new();
            expr_vars(c, &mut vars);
            for v in &vars {
                require(v)?;
            }
        }
    }
    Ok(())
}

/// Builds the predicate dependency graph. Only derived predicates are nodes: a stored
/// relation cannot depend on anything.
fn dependency_graph(program: &Program) -> (Vec<Pred>, BTreeMap<(usize, usize), Dep>) {
    let mut nodes: Vec<Pred> = Vec::new();
    let mut index = HashMap::new();
    for rule in &program.rules {
        if !index.contains_key(&rule.head.pred) {
            index.insert(rule.head.pred.clone(), nodes.len());
            nodes.push(rule.head.pred.clone());
        }
    }
    let mut edges: BTreeMap<(usize, usize), Dep> = BTreeMap::new();
    for rule in &program.rules {
        let head = index[&rule.head.pred];
        // An aggregating head must see a complete relation, so every body predicate of an
        // aggregating rule is an aggregating dependency.
        let aggregating = rule.head.aggregates();
        for item in &rule.body {
            let (atom, dep) = match item {
                BodyItem::Atom(a) => (
                    a,
                    if aggregating {
                        Dep::Aggregating
                    } else {
                        Dep::Positive
                    },
                ),
                BodyItem::Negated(a) => (a, Dep::Negative),
                BodyItem::Constraint(_) => continue,
            };
            if let Some(&to) = index.get(&atom.pred) {
                let key = (head, to);
                // A stronger dependency wins: Negative and Aggregating both forbid a cycle.
                let entry = edges.entry(key).or_insert(dep);
                if dep > *entry {
                    *entry = dep;
                }
            }
        }
    }
    (nodes, edges)
}

/// Tarjan's strongly connected components, as lists of node indices.
fn tarjan(n: usize, edges: &BTreeMap<(usize, usize), Dep>) -> Vec<Vec<usize>> {
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(from, to) in edges.keys() {
        succ[from].push(to);
    }

    struct State {
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        out: Vec<Vec<usize>>,
    }

    fn strongconnect(v: usize, succ: &[Vec<usize>], st: &mut State) {
        st.index[v] = Some(st.next);
        st.low[v] = st.next;
        st.next += 1;
        st.stack.push(v);
        st.on_stack[v] = true;
        for i in 0..succ[v].len() {
            let w = succ[v][i];
            match st.index[w] {
                None => {
                    strongconnect(w, succ, st);
                    st.low[v] = st.low[v].min(st.low[w]);
                }
                Some(iw) if st.on_stack[w] => st.low[v] = st.low[v].min(iw),
                Some(_) => {}
            }
        }
        if st.index[v] == Some(st.low[v]) {
            let mut component = Vec::new();
            while let Some(w) = st.stack.pop() {
                st.on_stack[w] = false;
                component.push(w);
                if w == v {
                    break;
                }
            }
            component.sort_unstable();
            st.out.push(component);
        }
    }

    let mut st = State {
        index: vec![None; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        next: 0,
        out: Vec::new(),
    };
    for v in 0..n {
        if st.index[v].is_none() {
            strongconnect(v, &succ, &mut st);
        }
    }
    st.out
}

/// The component each node belongs to.
fn component_of(n: usize, sccs: &[Vec<usize>]) -> Vec<usize> {
    let mut out = vec![0usize; n];
    for (ci, scc) in sccs.iter().enumerate() {
        for &v in scc {
            out[v] = ci;
        }
    }
    out
}

/// Rejects a negative or aggregating edge inside a component: that is exactly the condition
/// SQLite's recursive CTE cannot express, and exactly what stratification forbids.
fn check_stratification(
    nodes: &[Pred],
    sccs: &[Vec<usize>],
    owner: &[usize],
    edges: &BTreeMap<(usize, usize), Dep>,
) -> Result<()> {
    for (&(from, to), &dep) in edges {
        if dep == Dep::Positive || owner[from] != owner[to] {
            continue;
        }
        return Err(DatalogError::Unstratified {
            kind: if dep == Dep::Negative {
                "negation"
            } else {
                "aggregation"
            },
            component: sccs[owner[from]]
                .iter()
                .map(|&v| nodes[v].to_string())
                .collect(),
        });
    }
    Ok(())
}

/// Orders components so that every dependency is evaluated before its dependent.
fn topological(
    sccs: &[Vec<usize>],
    owner: &[usize],
    edges: &BTreeMap<(usize, usize), Dep>,
) -> Vec<usize> {
    let n = sccs.len();
    let mut succ: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    let mut indegree = vec![0usize; n];
    for &(from, to) in edges.keys() {
        let (a, b) = (owner[from], owner[to]);
        // `a` depends on `b`, so `b` must be evaluated first.
        if a != b && succ[b].insert(a) {
            indegree[a] += 1;
        }
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut out = Vec::with_capacity(n);
    while let Some(&i) = ready.iter().next() {
        ready.remove(&i);
        out.push(i);
        for &j in &succ[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                ready.insert(j);
            }
        }
    }
    // Components form a DAG by construction, but never drop a stratum if that ever changes.
    for i in 0..n {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

/// Classifies how a component must be evaluated.
fn shape_of(program: &Program, preds: &[Pred]) -> Shape {
    let inside = |p: &Pred| preds.contains(p);
    let mut recursive = false;
    let mut non_linear = false;
    for rule in &program.rules {
        if !inside(&rule.head.pred) {
            continue;
        }
        let n = rule.positive().filter(|a| inside(&a.pred)).count();
        if n > 0 {
            recursive = true;
        }
        if n > 1 {
            non_linear = true;
        }
    }
    if !recursive {
        Shape::NonRecursive
    } else if non_linear {
        Shape::NonLinear
    } else if preds.len() > 1 {
        Shape::LinearMutual
    } else {
        Shape::Linear
    }
}
