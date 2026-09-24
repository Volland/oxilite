//! Running a compiled program: the sans-IO step machine and result decoding.
//!
//! A program whose recursion is linear is two requests on every backend, D1 included: the
//! statement, then the term lookup. A component that has to be iterated adds one request per
//! round, which is why the compiler prefers any strategy that avoids it.
//!
// @lat: [[architecture#Datalog frontend#Execution]]

use crate::error::{DatalogError, Result};
use crate::sql::Compiled;
use oxilite_core::job::{Job, Step};
use oxilite_core::resolve::TermResolver;
use oxilite_core::sql::{Capabilities, Request, Response, SqlValue, Statement};
use oxrdf::Term;

/// The solutions of a program.
#[derive(Debug, Clone, PartialEq)]
pub struct DatalogResult {
    /// The goal's variables, in column order.
    pub variables: Vec<String>,
    /// One row per solution; `None` where a column is unbound.
    pub rows: Vec<Vec<Option<Term>>>,
    /// Rounds each iterated component took, in order, for `explain()` and for tests.
    pub rounds: Vec<usize>,
}

impl DatalogResult {
    /// The column index of a variable.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        let name = name.strip_prefix('?').unwrap_or(name);
        self.variables.iter().position(|v| v == name)
    }

    /// The value bound to a variable in one row.
    pub fn get(&self, row: usize, name: &str) -> Option<&Term> {
        let i = self.index_of(name)?;
        self.rows.get(row)?.get(i)?.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Start,
    /// Waiting for the seed of a phase; the count comes back with it.
    Seeded(usize),
    /// Waiting for a step of a phase, with the count before it and the round number.
    Stepped {
        phase: usize,
        before: i64,
        round: usize,
    },
    Rows,
    CleaningUp,
    Resolving,
    Done,
}

/// Runs a compiled program to its solutions.
#[derive(Debug)]
pub struct DatalogJob {
    compiled: Compiled,
    caps: Capabilities,
    state: State,
    resolver: TermResolver,
    ids: Vec<Vec<Option<i64>>>,
    rounds: Vec<usize>,
}

impl DatalogJob {
    pub fn new(compiled: Compiled, caps: Capabilities) -> Self {
        let resolver = TermResolver::with_constants(
            compiled
                .constants
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect(),
        );
        Self {
            compiled,
            caps,
            state: State::Start,
            resolver,
            ids: Vec::new(),
            rounds: Vec::new(),
        }
    }

    /// The SQL this job runs for its goal.
    pub fn sql(&self) -> &str {
        &self.compiled.sql
    }

    /// Applies a phase's non-recursive rules, then counts.
    fn seed(&self, phase: usize) -> Request {
        let f = &self.compiled.fixpoint;
        let mut statements: Vec<Statement> = Vec::new();
        if phase == 0 {
            // A previous run that was interrupted may have left rows under this id.
            statements.push(Statement::new(f.cleanup()));
        }
        statements.extend(f.phases[phase].seed.iter().map(Statement::new));
        statements.push(Statement::new(f.count()));
        Request::atomic(statements)
    }

    /// Applies a phase's recursive rules once, then counts.
    fn step_once(&self, phase: usize) -> Request {
        let f = &self.compiled.fixpoint;
        let mut statements: Vec<Statement> =
            f.phases[phase].step.iter().map(Statement::new).collect();
        statements.push(Statement::new(f.count()));
        Request::atomic(statements)
    }

    fn main_query(&self) -> Request {
        Request::read(vec![Statement::new(self.compiled.sql.clone())])
    }

    /// The count is always the last statement of a fixpoint request.
    fn count_of(response: &Response) -> i64 {
        response
            .last()
            .and_then(|rs| rs.rows.first())
            .and_then(|row| row.first())
            .and_then(SqlValue::as_i64)
            .unwrap_or(0)
    }

    /// Moves on to the next phase, or to the goal when every phase has converged.
    fn after_phase(&mut self, phase: usize) -> Request {
        if phase + 1 < self.compiled.fixpoint.phases.len() {
            self.state = State::Seeded(phase + 1);
            self.seed(phase + 1)
        } else {
            self.state = State::Rows;
            self.main_query()
        }
    }

    fn absorb(&mut self, response: Response) -> Result<()> {
        let rs = response
            .into_iter()
            .next()
            .ok_or_else(|| oxilite_core::Error::backend("empty response"))?;
        let width = self.compiled.variables.len();
        for row in rs.rows {
            let mut out = Vec::with_capacity(width);
            for i in 0..width {
                // D1 returns 64-bit integers as text to keep JavaScript from rounding them.
                let id = match row.get(i) {
                    None | Some(SqlValue::Null) => None,
                    Some(v) => match v.as_i64() {
                        Some(id) => Some(id),
                        None => v.as_str().and_then(|s| s.parse::<i64>().ok()),
                    },
                };
                if let Some(id) = id {
                    self.resolver.want(id);
                }
                out.push(id);
            }
            self.ids.push(out);
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<DatalogResult> {
        let mut rows = Vec::with_capacity(self.ids.len());
        for row in &self.ids {
            let mut out = Vec::with_capacity(row.len());
            for cell in row {
                out.push(match cell {
                    None => None,
                    Some(id) => Some(self.resolver.get(*id)?),
                });
            }
            rows.push(out);
        }
        Ok(DatalogResult {
            variables: self.compiled.variables.clone(),
            rows,
            rounds: self.rounds.clone(),
        })
    }

    /// Resolution, or the finished result when nothing is left to look up.
    fn resolve_or_done(&mut self) -> Result<Step<DatalogResult>> {
        match self.resolver.request(&self.caps) {
            Some(r) => {
                self.state = State::Resolving;
                Ok(Step::Execute(r))
            }
            None => {
                self.state = State::Done;
                Ok(Step::Done(self.finish()?))
            }
        }
    }
}

impl Job for DatalogJob {
    type Output = DatalogResult;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<Self::Output>> {
        match self.state {
            State::Start => {
                if self.compiled.fixpoint.is_empty() {
                    self.state = State::Rows;
                    Ok(Step::Execute(self.main_query()))
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
                    // Nothing new was derived: this component has reached its least fixpoint.
                    let request = self.after_phase(phase);
                    return Ok(Step::Execute(request));
                }
                if round >= self.compiled.fixpoint.max_iterations {
                    return Err(into_core(DatalogError::IterationLimit {
                        component: self.compiled.fixpoint.phases[phase].preds.join(", "),
                        rounds: round,
                    }));
                }
                self.state = State::Stepped {
                    phase,
                    before: after,
                    round: round + 1,
                };
                Ok(Step::Execute(self.step_once(phase)))
            }
            State::Rows => {
                let response = response.ok_or_else(|| {
                    oxilite_core::Error::backend("missing response for the program")
                })?;
                self.absorb(response).map_err(into_core)?;
                if !self.compiled.fixpoint.is_empty() {
                    self.state = State::CleaningUp;
                    return Ok(Step::Execute(Request::atomic(vec![Statement::new(
                        self.compiled.fixpoint.cleanup(),
                    )])));
                }
                self.resolve_or_done().map_err(into_core)
            }
            State::CleaningUp => self.resolve_or_done().map_err(into_core),
            State::Resolving => {
                if let Some(response) = response {
                    self.resolver.absorb(response)?;
                }
                self.resolve_or_done().map_err(into_core)
            }
            State::Done => Err(oxilite_core::Error::backend(
                "the program is already finished",
            )),
        }
    }
}

/// The job protocol speaks the core error type, so a Datalog error is carried through it.
fn into_core(e: DatalogError) -> oxilite_core::Error {
    match e {
        DatalogError::Store(e) => e,
        other => oxilite_core::Error::backend(other.to_string()),
    }
}
