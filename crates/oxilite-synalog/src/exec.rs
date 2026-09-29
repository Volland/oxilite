//! Running a compiled program: one read request, whose rows are already SQL values.
//!
// @lat: [[architecture#Synalog frontend#Execution]]

use crate::Compiled;
use oxilite_core::job::{Job, Step};
use oxilite_core::sql::{Request, Response, SqlValue, Statement};

/// The rows of a predicate.
#[derive(Debug, Clone, PartialEq)]
pub struct SynalogResult {
    /// The predicate's head columns, in order.
    pub columns: Vec<String>,
    /// One row per result, as SQL values.
    pub rows: Vec<Vec<SqlValue>>,
}

impl SynalogResult {
    /// The index of a column.
    pub fn index_of(&self, column: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == column)
    }

    /// The value of a column in one row.
    pub fn get(&self, row: usize, column: &str) -> Option<&SqlValue> {
        self.rows.get(row)?.get(self.index_of(column)?)
    }
}

/// Runs a compiled program to its rows.
#[derive(Debug)]
pub struct SynalogJob {
    compiled: Compiled,
    sent: bool,
}

impl SynalogJob {
    pub fn new(compiled: Compiled) -> Self {
        Self {
            compiled,
            sent: false,
        }
    }

    /// The SQL this job runs.
    pub fn sql(&self) -> &str {
        &self.compiled.sql
    }
}

impl Job for SynalogJob {
    type Output = SynalogResult;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<Self::Output>> {
        if !self.sent {
            self.sent = true;
            return Ok(Step::Execute(Request::read(vec![Statement::new(
                self.compiled.sql.clone(),
            )])));
        }
        let rs = response
            .and_then(|r| r.into_iter().next())
            .ok_or_else(|| oxilite_core::Error::backend("missing response for the program"))?;
        Ok(Step::Done(SynalogResult {
            columns: self.compiled.columns.clone(),
            rows: rs.rows,
        }))
    }
}
