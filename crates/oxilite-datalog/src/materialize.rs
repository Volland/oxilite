//! Storing what a program derives, as inferences.
//!
//! Derived facts go into `quads_inf`, the table OWL 2 RL materialization already uses, so a
//! user rule is visible to SPARQL and Cypher through the `inferred` option that already
//! exists. Conclusions are attributed to a producer (`Options::producer`), so materializing a
//! program replaces its own earlier conclusions and leaves other producers' in place.
//!
// @lat: [[architecture#Datalog frontend#Materialization]]

use crate::ast::{Arg, Atom, Goal, Head, Pred, Program};
use crate::error::{DatalogError, Result};
use crate::program::{self, Analysis};
use crate::sql::{self, Fixpoint, Options};
use oxilite_core::encoding::EncodedRows;
use oxilite_core::job::{Job, Step};
use oxilite_core::sql::{Capabilities, Request, Response, SqlValue, Statement};
use oxrdf::TermRef;

/// What a materialization did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MaterializeStats {
    /// Triples in the inference table afterwards.
    pub inferred: u64,
    /// Relations whose conclusions were stored.
    pub relations: usize,
}

/// Every rule head must have an RDF form, or nothing is written.
fn triple_shaped(head: &Head) -> Result<()> {
    let ok = match &head.pred {
        // Binary is a predicate, unary is a class: `ex:Adult(?p)` stores `?p rdf:type ex:Adult`.
        Pred::Edb(_) => matches!(head.args.len(), 1 | 2),
        Pred::Triple { graph } => head.args.len() == if *graph { 4 } else { 3 },
        Pred::Idb(_) => false,
    };
    if ok {
        Ok(())
    } else {
        Err(DatalogError::NotTripleShaped {
            predicate: head.pred.to_string(),
        })
    }
}

/// The statements that replace the inference set with what this program derives, and the
/// iteration those statements depend on.
///
/// The writes go into one atomic request: the reset, the term rows for every constant the
/// program mentions, one `INSERT … SELECT` per derived relation, and a count. A component that
/// has to be iterated runs first, in its own requests, and every head shares that one
/// evaluation.
pub fn plan(
    src: &str,
    caps: &Capabilities,
    options: &Options,
) -> Result<(Fixpoint, Vec<Statement>, usize)> {
    let program = crate::parse(src)?;
    let analysis = program::analyse(&program)?;
    let heads = materializable(&program)?;
    let run = sql::new_run();

    let mut statements = oxilite_core::reason::materialize_reset_for(caps, &options.producer);
    let mut rows = EncodedRows::default();
    let mut inserts = Vec::new();
    let mut fixpoint = Fixpoint {
        run,
        ..Default::default()
    };
    for head in &heads {
        let compiled = relation_for(&program, &analysis, head, caps, options, run)?;
        // Every head compiles the same strata, so one plan covers them all.
        if fixpoint.is_empty() {
            fixpoint = compiled.fixpoint.clone();
        }
        // A conclusion may mention an IRI the store has never seen — the head predicate, most
        // of all — so every constant gets its `terms` row before the insert runs.
        for term in compiled.constants.values() {
            rows.term(TermRef::from(term));
        }
        if let Pred::Edb(iri) = &head.pred {
            rows.iri(iri.as_str());
        }
        inserts.push(Statement::new(insert_for(head, &compiled)?));
    }
    rows.dedup();
    statements.extend(oxilite_core::writer::term_statements(&rows, caps));
    statements.extend(inserts);
    statements.push(oxilite_core::reason::inference_attribute(&options.producer));
    statements.push(Statement::new("SELECT COUNT(*) FROM quads_inf"));
    Ok((fixpoint, statements, heads.len()))
}

/// The distinct rule heads a program asks to store.
fn materializable(program: &Program) -> Result<Vec<Head>> {
    let mut out: Vec<Head> = Vec::new();
    for rule in &program.rules {
        triple_shaped(&rule.head)?;
        if !out.iter().any(|h| h.pred == rule.head.pred) {
            out.push(rule.head.clone());
        }
    }
    Ok(out)
}

/// Compiles the relation behind one rule head, by giving the program a goal that names it.
fn relation_for(
    program: &Program,
    analysis: &Analysis,
    head: &Head,
    caps: &Capabilities,
    options: &Options,
    run: i64,
) -> Result<sql::Compiled> {
    let arity = head.args.len();
    let goal = Goal {
        atom: Atom {
            pred: head.pred.clone(),
            args: (0..arity).map(|i| Arg::Var(format!("__m{i}"))).collect(),
            span: head.span,
        },
        constraints: Vec::new(),
    };
    sql::compile_for(program, analysis, caps, options, &goal, run)
}

/// `INSERT OR IGNORE INTO quads_inf (s, p, o, g) SELECT … FROM (<the head's relation>)`.
fn insert_for(head: &Head, compiled: &sql::Compiled) -> Result<String> {
    let arity = head.args.len();
    let (s, p, o, g) = match &head.pred {
        // A one-argument head is a class: `ex:Adult(?p)` stores `?p rdf:type ex:Adult`.
        Pred::Edb(iri) if arity == 1 => (
            "v0".to_owned(),
            oxilite_core::encoding::rdf_type_id().to_string(),
            oxilite_core::encoding::named_node_id(iri.as_str()).to_string(),
            "0".to_owned(),
        ),
        Pred::Edb(iri) => (
            "v0".to_owned(),
            oxilite_core::encoding::named_node_id(iri.as_str()).to_string(),
            "v1".to_owned(),
            "0".to_owned(),
        ),
        Pred::Triple { graph } => (
            "v0".to_owned(),
            "v1".to_owned(),
            "v2".to_owned(),
            if *graph { "v3".to_owned() } else { "0".to_owned() },
        ),
        Pred::Idb(_) => {
            return Err(DatalogError::NotTripleShaped {
                predicate: head.pred.to_string(),
            })
        }
    };
    Ok(format!(
        "INSERT OR IGNORE INTO quads_inf (s, p, o, g) \
         SELECT {s}, {p}, {o}, {g} FROM ({}) WHERE {}",
        compiled.sql,
        (0..arity)
            .map(|i| format!("v{i} IS NOT NULL"))
            .collect::<Vec<_>>()
            .join(" AND ")
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Start,
    Seeded(usize),
    Stepped { phase: usize, before: i64, round: usize },
    Wrote,
    CleaningUp(u64),
    Done,
}

/// Evaluates a program and applies its conclusions in one atomic request.
///
/// A component that needs iteration runs first, in its own requests; the writes still land in
/// a single atomic request, so a backend without interactive transactions is never left
/// holding a half-replaced inference set.
#[derive(Debug)]
pub struct MaterializeJob {
    fixpoint: Fixpoint,
    statements: Vec<Statement>,
    relations: usize,
    state: State,
    rounds: Vec<usize>,
}

impl MaterializeJob {
    pub fn new(src: &str, caps: &Capabilities, options: &Options) -> Result<Self> {
        let (fixpoint, statements, relations) = plan(src, caps, options)?;
        Ok(Self {
            fixpoint,
            statements,
            relations,
            state: State::Start,
            rounds: Vec::new(),
        })
    }

    fn seed(&self, phase: usize) -> Request {
        let mut statements: Vec<Statement> = Vec::new();
        if phase == 0 {
            statements.push(Statement::new(self.fixpoint.cleanup()));
        }
        statements.extend(self.fixpoint.phases[phase].seed.iter().map(Statement::new));
        statements.push(Statement::new(self.fixpoint.count()));
        Request::atomic(statements)
    }

    fn step_once(&self, phase: usize) -> Request {
        let mut statements: Vec<Statement> = self.fixpoint.phases[phase]
            .step
            .iter()
            .map(Statement::new)
            .collect();
        statements.push(Statement::new(self.fixpoint.count()));
        Request::atomic(statements)
    }

    fn writes(&mut self) -> Request {
        Request::atomic(std::mem::take(&mut self.statements))
    }

    fn count_of(response: &Response) -> i64 {
        response
            .last()
            .and_then(|rs| rs.rows.first())
            .and_then(|row| row.first())
            .and_then(SqlValue::as_i64)
            .unwrap_or(0)
    }

    fn after_phase(&mut self, phase: usize) -> Request {
        if phase + 1 < self.fixpoint.phases.len() {
            self.state = State::Seeded(phase + 1);
            self.seed(phase + 1)
        } else {
            self.state = State::Wrote;
            self.writes()
        }
    }

    fn done(&self, inferred: u64) -> MaterializeStats {
        MaterializeStats {
            inferred,
            relations: self.relations,
        }
    }
}

impl Job for MaterializeJob {
    type Output = MaterializeStats;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<Self::Output>> {
        match self.state {
            State::Start => {
                if self.fixpoint.is_empty() {
                    self.state = State::Wrote;
                    let r = self.writes();
                    Ok(Step::Execute(r))
                } else {
                    self.state = State::Seeded(0);
                    Ok(Step::Execute(self.seed(0)))
                }
            }
            State::Seeded(phase) => {
                let before = Self::count_of(&response.unwrap_or_default());
                self.rounds.push(0);
                self.state = State::Stepped {
                    phase,
                    before,
                    round: 1,
                };
                Ok(Step::Execute(self.step_once(phase)))
            }
            State::Stepped {
                phase,
                before,
                round,
            } => {
                let after = Self::count_of(&response.unwrap_or_default());
                *self.rounds.last_mut().expect("a phase was seeded") = round;
                if after == before {
                    let request = self.after_phase(phase);
                    return Ok(Step::Execute(request));
                }
                if round >= self.fixpoint.max_iterations {
                    return Err(oxilite_core::Error::backend(
                        DatalogError::IterationLimit {
                            component: self.fixpoint.phases[phase].preds.join(", "),
                            rounds: round,
                        }
                        .to_string(),
                    ));
                }
                self.state = State::Stepped {
                    phase,
                    before: after,
                    round: round + 1,
                };
                Ok(Step::Execute(self.step_once(phase)))
            }
            State::Wrote => {
                // The count of the inference table is the last statement of the write request.
                let inferred = response
                    .as_ref()
                    .and_then(|r| r.last())
                    .and_then(|rs| rs.rows.first())
                    .and_then(|row| row.first())
                    .and_then(SqlValue::as_i64)
                    .unwrap_or(0) as u64;
                if self.fixpoint.is_empty() {
                    self.state = State::Done;
                    return Ok(Step::Done(self.done(inferred)));
                }
                self.state = State::CleaningUp(inferred);
                Ok(Step::Execute(Request::atomic(vec![Statement::new(
                    self.fixpoint.cleanup(),
                )])))
            }
            State::CleaningUp(inferred) => {
                self.state = State::Done;
                Ok(Step::Done(self.done(inferred)))
            }
            State::Done => Err(oxilite_core::Error::backend(
                "the materialization is already finished",
            )),
        }
    }
}
