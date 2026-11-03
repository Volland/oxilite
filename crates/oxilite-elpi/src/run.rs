//! Running a program over the store: goal queries, constraint checks and materialization.

#[cfg(not(feature = "native"))]
use crate::error::ElpiError;
use crate::error::Result;
use crate::host::{Host, Lookup};
use crate::term::{from_elpi, iri_from_elpi, subject_from_elpi, ETerm};
use oxrdf::{GraphName, Term, Triple};
use std::collections::BTreeMap;

/// The producer name materialized ELPI conclusions are attributed to by default.
pub const ELPI_PRODUCER: &str = "elpi";

/// The types and host predicates every program sees, loaded in front of its text.
pub const PRELUDE: &str = r#"
kind term type.
type iri string -> term.
type str string -> term.
type num int -> term.
type dbl float -> term.
type lang string -> string -> term.
type lit string -> string -> term.
type bnode string -> term.
type default-graph term.
kind fact type.
type triple term -> term -> term -> fact.
kind severity type.
type error severity.
type warning severity.
pred ox.triple o:term, o:term, o:term.
pred ox.quad o:term, o:term, o:term, o:term.
pred ox.conclude o:fact.
pred ox.violation o:severity, o:string, o:string, o:list term.
"#;

/// What a run reads and how long it may take.
#[derive(Debug, Clone)]
// @tg: implements:: [[openspec:elpi-programs#Bounded runs]] {since: "2026-11-03"}
pub struct Options {
    /// The graphs `ox.triple` and `ox.quad` read; `None` reads every graph but the system graphs.
    pub graphs: Option<Vec<GraphName>>,
    /// Also read materialized inferences of other producers (never the run's own).
    pub include_inferred: bool,
    /// The producer materialized conclusions are stored under.
    pub producer: String,
    /// Solutions to collect; 0 collects all.
    pub limit: usize,
    /// Resolution steps the run may take; 0 uses lean-elpi's default budget.
    pub fuel: u64,
    /// Rows the host builtins may return in total; 0 is unbounded.
    pub max_rows: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            graphs: None,
            include_inferred: false,
            producer: ELPI_PRODUCER.to_owned(),
            limit: 0,
            fuel: 0,
            max_rows: 1_000_000,
        }
    }
}

/// The value of a query variable.
#[derive(Debug, Clone, PartialEq)]
pub enum Binding {
    /// An RDF term (`(iri …)`, `(str …)`, `(num …)`, …).
    Term(Term),
    /// Any other ELPI value, as ELPI prints it (an `int`, a list, a λ-term…).
    Value(String),
}

/// One answer to a goal.
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    pub bindings: BTreeMap<String, Binding>,
    /// Suspended constraints, as ELPI prints them.
    pub constraints: Vec<String>,
}

/// The answers to a goal.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub solutions: Vec<Solution>,
    /// False when `Options::limit` cut the search short.
    pub exhausted: bool,
    /// Resolution steps the run took.
    pub fuel_used: u64,
    /// Rows the host builtins returned.
    pub rows_read: u64,
}

/// A violation a program reports through `ox.violation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// `error` or `warning`.
    pub severity: String,
    pub name: String,
    pub message: String,
    /// The terms the violation is about.
    pub focus: Vec<Term>,
}

/// What a materialization stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeStats {
    /// Distinct inferred quads now attributed to the producer.
    pub inferred: u64,
    /// Conclusions skipped because they are no storable triple (a literal subject, a variable).
    pub unstorable: usize,
    pub rows_read: u64,
    pub producer: String,
}

/// A solved goal: the raw ELPI answers.
/// One raw answer: the bindings as terms, as ELPI prints them, and the suspended constraints.
pub(crate) type Answer = (
    BTreeMap<String, ETerm>,
    BTreeMap<String, String>,
    Vec<String>,
);

pub(crate) struct Solved {
    pub solutions: Vec<Answer>,
    pub exhausted: bool,
    pub fuel_used: u64,
}

fn binding(term: Option<&ETerm>, printed: Option<&String>) -> Binding {
    match term.and_then(from_elpi) {
        Some(t) => Binding::Term(t),
        None => Binding::Value(printed.cloned().unwrap_or_default()),
    }
}

/// Solves `goal`, the host builtins reading the store through `lookup`; also returns the rows
/// they read.
fn solve_raw(
    program: &str,
    goal: &str,
    options: &Options,
    lookup: &mut Lookup<'_>,
) -> Result<(Solved, u64)> {
    let mut h = Host::new(
        lookup,
        options.graphs.clone(),
        options.include_inferred,
        Some(options.producer.clone()),
        options.max_rows,
    );
    let solved = crate::engine::solve(program, goal, options, &mut h)?;
    Ok((solved, h.rows))
}

/// Solves `goal` against `program`, reading the store through `lookup`.
pub fn query(
    program: &str,
    goal: &str,
    options: &Options,
    lookup: &mut Lookup<'_>,
) -> Result<QueryResult> {
    let (solved, rows_read) = solve_raw(program, goal, options, lookup)?;
    Ok(QueryResult {
        solutions: solved
            .solutions
            .iter()
            .map(|(terms, printed, constraints)| Solution {
                bindings: printed
                    .keys()
                    .chain(terms.keys())
                    .map(|k| (k.clone(), binding(terms.get(k), printed.get(k))))
                    .collect(),
                constraints: constraints.clone(),
            })
            .collect(),
        exhausted: solved.exhausted,
        fuel_used: solved.fuel_used,
        rows_read,
    })
}

/// The goal `check` solves.
pub const VIOLATION_GOAL: &str = "ox.violation Severity Name Message Focus";

/// Runs `ox.violation Severity Name Message Focus` and returns every violation it reports.
// @tg: implements:: [[openspec:elpi-programs#Constraint checks]] {since: "2026-11-03"}
pub fn check(program: &str, options: &Options, lookup: &mut Lookup<'_>) -> Result<Vec<Violation>> {
    let mut opts = options.clone();
    opts.limit = 0;
    let (solved, _) = solve_raw(program, VIOLATION_GOAL, &opts, lookup)?;
    let mut out: Vec<Violation> = solved
        .solutions
        .iter()
        .filter_map(|(terms, printed, _)| {
            let text = |k: &str| match terms.get(k) {
                Some(ETerm::Str(s)) => Some(s.clone()),
                Some(ETerm::Const(c)) => Some(c.clone()),
                _ => printed.get(k).map(|p| p.trim_matches('"').to_owned()),
            };
            Some(Violation {
                severity: text("Severity")?,
                name: text("Name")?,
                message: text("Message")?,
                focus: terms
                    .get("Focus")
                    .and_then(ETerm::as_list)
                    .map(|items| items.into_iter().filter_map(from_elpi).collect())
                    .unwrap_or_default(),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        (
            &a.name,
            &a.message,
            a.focus.iter().map(ToString::to_string).collect::<Vec<_>>(),
        )
            .cmp(&(
                &b.name,
                &b.message,
                b.focus.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ))
    });
    out.dedup();
    Ok(out)
}

/// The goal `conclusions` solves.
pub const CONCLUDE_GOAL: &str = "ox.conclude F";

/// Runs `ox.conclude F` and returns the triples `(triple S P O)` it concludes, how many
/// conclusions were no storable triple, and the rows read.
pub fn conclusions(
    program: &str,
    options: &Options,
    lookup: &mut Lookup<'_>,
) -> Result<(Vec<Triple>, usize, u64)> {
    let mut opts = options.clone();
    opts.limit = 0;
    let (solved, rows) = solve_raw(program, CONCLUDE_GOAL, &opts, lookup)?;
    let mut out = Vec::new();
    let mut unstorable = 0;
    for (terms, _, _) in &solved.solutions {
        let triple = match terms.get("F") {
            Some(ETerm::App(c, a)) if c == "triple" && a.len() == 3 => {
                match (
                    subject_from_elpi(&a[0]),
                    iri_from_elpi(&a[1]),
                    from_elpi(&a[2]),
                ) {
                    (Some(s), Some(p), Some(o)) => Some(Triple::new(s, p, o)),
                    _ => None,
                }
            }
            _ => None,
        };
        match triple {
            Some(t) => out.push(t),
            None => unstorable += 1,
        }
    }
    out.sort_by_cached_key(ToString::to_string);
    out.dedup();
    Ok((out, unstorable, rows))
}

/// The error for a run on a build without the engine.
#[cfg(not(feature = "native"))]
pub(crate) fn unsupported() -> ElpiError {
    ElpiError::Unsupported(
        "oxilite was built without the native lean-elpi engine (enable `elpi-native` and set LEANELPI_DIST)"
            .into(),
    )
}
