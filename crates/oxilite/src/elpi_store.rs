//! λProlog (ELPI) programs on the blocking store (`elpi_query`, `elpi_check`,
//! `elpi_materialize`). Host calls are synchronous, so there is no `AsyncStore` variant.
//!
// @lat: [[architecture#ELPI reasoning]]

use crate::store::Store;
use oxilite_core::SyncBackend;
use oxilite_elpi::{ElpiError, MaterializeStats, Options, QueryResult, Violation};

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// Solves `goal` against an ELPI `program` that reads the store through `ox.triple S P O`
    /// and `ox.quad S P O G`; bindings come back as RDF terms where they are ones.
    pub fn elpi_query(&self, program: &str, goal: &str) -> Result<QueryResult, ElpiError> {
        self.elpi_query_with(program, goal, &Options::default())
    }

    /// Solves a goal with options (scope, inferences, solution limit, fuel, row budget).
    pub fn elpi_query_with(
        &self,
        program: &str,
        goal: &str,
        options: &Options,
    ) -> Result<QueryResult, ElpiError> {
        oxilite_elpi::query(self.versioned(), program, goal, options)
    }

    /// Every violation the program reports with `ox.violation Severity Name Message Focus`.
    pub fn elpi_check(&self, program: &str) -> Result<Vec<Violation>, ElpiError> {
        self.elpi_check_with(program, &Options::default())
    }

    /// Checks with options.
    pub fn elpi_check_with(
        &self,
        program: &str,
        options: &Options,
    ) -> Result<Vec<Violation>, ElpiError> {
        oxilite_elpi::check(self.versioned(), program, options)
    }

    /// Stores what the program concludes with `ox.conclude (triple S P O)` as inferences of the
    /// producer `elpi`, replacing its earlier ones in one atomic request.
    pub fn elpi_materialize(&self, program: &str) -> Result<MaterializeStats, ElpiError> {
        self.elpi_materialize_with(program, &Options::default())
    }

    /// Materializes with options; `options.producer` names the program's inferences.
    pub fn elpi_materialize_with(
        &self,
        program: &str,
        options: &Options,
    ) -> Result<MaterializeStats, ElpiError> {
        oxilite_elpi::materialize(self.versioned(), program, options)
    }
}
