//! `datalog()` on the blocking and async stores.
//!
// @lat: [[architecture#Datalog frontend]]

use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::{AsyncBackend, SyncBackend};
use oxilite_datalog::{
    compile, explain as explain_program, prepare, DatalogError, DatalogJob, DatalogResult,
    MaterializeJob, MaterializeStats, Options,
};

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// Runs a Datalog program and returns the solutions of its goal.
    ///
    /// ```
    /// # use oxilite::store::Store;
    /// let store = Store::new()?;
    /// store.update(
    ///     "PREFIX ex: <http://example.org/>
    ///      INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
    /// )?;
    /// let r = store.datalog(
    ///     "@prefix ex: <http://example.org/> .
    ///      anc(?x, ?y) :- ex:parent(?x, ?y).
    ///      anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).
    ///      ?- anc(?x, ?y).",
    /// )?;
    /// assert_eq!(r.rows.len(), 3);
    /// # Result::<_, Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn datalog(&self, program: &str) -> Result<DatalogResult, DatalogError> {
        self.datalog_with(program, &Options::default())
    }

    /// Runs a Datalog program with options (graph scope, inferences, iteration bound).
    pub fn datalog_with(
        &self,
        program: &str,
        options: &Options,
    ) -> Result<DatalogResult, DatalogError> {
        let job: DatalogJob = prepare(program, self.caps(), options)?;
        Ok(self.run(job)?)
    }

    /// Describes how a program runs: its strata, the strategy chosen for each recursive
    /// component, and the SQL it compiles to.
    pub fn explain_datalog(&self, program: &str) -> Result<String, DatalogError> {
        explain_program(program, self.caps(), &Options::default())
    }

    /// The SQL a program compiles to, without running it.
    pub fn datalog_sql(&self, program: &str) -> Result<String, DatalogError> {
        Ok(compile(program, self.caps(), &Options::default())?.sql)
    }

    /// Evaluates a program and stores everything it derives as inferences.
    ///
    /// Conclusions go to the same place OWL 2 RL materialization writes, so they are visible
    /// to SPARQL and Cypher under `include_inferred`. The two share one inference set:
    /// running either replaces it.
    pub fn datalog_materialize(
        &self,
        program: &str,
    ) -> Result<MaterializeStats, DatalogError> {
        self.datalog_materialize_with(program, &Options::default())
    }

    /// Materializes a program with options.
    pub fn datalog_materialize_with(
        &self,
        program: &str,
        options: &Options,
    ) -> Result<MaterializeStats, DatalogError> {
        let job = MaterializeJob::new(program, self.caps(), options)?;
        Ok(self.run(job)?)
    }
}

impl<B: AsyncBackend> AsyncStore<B> {
    /// Runs a Datalog program (see [`Store::datalog`]).
    pub async fn datalog(&self, program: &str) -> Result<DatalogResult, DatalogError> {
        self.datalog_with(program, &Options::default()).await
    }

    /// Runs a Datalog program with options.
    pub async fn datalog_with(
        &self,
        program: &str,
        options: &Options,
    ) -> Result<DatalogResult, DatalogError> {
        let job: DatalogJob = prepare(program, self.caps(), options)?;
        Ok(oxilite_core::run_async(&self.backend, job).await?)
    }

    /// Describes how a program runs.
    pub async fn explain_datalog(&self, program: &str) -> Result<String, DatalogError> {
        explain_program(program, self.caps(), &Options::default())
    }

    /// Evaluates a program and stores what it derives, in one atomic request.
    pub async fn datalog_materialize(
        &self,
        program: &str,
    ) -> Result<MaterializeStats, DatalogError> {
        let job = MaterializeJob::new(program, self.caps(), &Options::default())?;
        Ok(oxilite_core::run_async(&self.backend, job).await?)
    }
}
