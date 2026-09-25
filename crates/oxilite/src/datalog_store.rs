//! `datalog()` on the blocking and async stores.
//!
// @lat: [[architecture#Datalog frontend]]

use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::{AsyncBackend, SyncBackend};
use oxilite_datalog::{
    compile, explain as explain_program, prepare, version_of, version_refs, DatalogError,
    DatalogJob, DatalogResult, MaterializeJob, MaterializeStats, Options,
};
use std::borrow::Cow;

/// A program reading a version: inferences exist only for the current state.
fn check_version(options: &Options) -> Result<(), DatalogError> {
    if options.include_inferred {
        return Err(DatalogError::Unsupported(
            "inferences describe the current state only; they cannot be combined with @version"
                .into(),
        ));
    }
    Ok(())
}

/// Materialization writes the current state: it cannot read another version.
fn check_materialize(program: &str, options: &Options) -> Result<(), DatalogError> {
    if version_of(program, options)?.is_some() {
        return Err(DatalogError::Unsupported(
            "materialization derives from the current state; drop @version / as_of".into(),
        ));
    }
    Ok(())
}

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
        let options = self.datalog_version(program, options)?;
        let job: DatalogJob = prepare(program, self.caps(), &options)?;
        Ok(self.run(job)?)
    }

    /// The options with the version the program reads resolved to a tick.
    fn datalog_version<'o>(
        &self,
        program: &str,
        options: &'o Options,
    ) -> Result<Cow<'o, Options>, DatalogError> {
        let (whole, atoms) = version_refs(program, options)?;
        let mut o = options.clone();
        o.history = Some(self.stats().version);
        if let Some(v) = whole {
            check_version(options)?;
            o.as_of_tick = Some(self.resolve_version(&v)?);
        }
        for r in atoms {
            let t = self.resolve_version(&r)?;
            o.versions.insert(r, t);
        }
        Ok(Cow::Owned(o))
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
    pub fn datalog_materialize(&self, program: &str) -> Result<MaterializeStats, DatalogError> {
        self.datalog_materialize_with(program, &Options::default())
    }

    /// Materializes a program with options.
    pub fn datalog_materialize_with(
        &self,
        program: &str,
        options: &Options,
    ) -> Result<MaterializeStats, DatalogError> {
        check_materialize(program, options)?;
        let mut options = options.clone();
        options.history = Some(self.stats().version);
        let job = MaterializeJob::new(program, self.caps(), &options)?;
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
        let (whole, atoms) = version_refs(program, options)?;
        let mut o = options.clone();
        o.history = Some(self.stats.borrow().version);
        if let Some(v) = whole {
            check_version(options)?;
            o.as_of_tick = Some(self.resolve_version(&v).await?);
        }
        for r in atoms {
            let t = self.resolve_version(&r).await?;
            o.versions.insert(r, t);
        }
        let options: Cow<'_, Options> = Cow::Owned(o);
        let job: DatalogJob = prepare(program, self.caps(), &options)?;
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
        check_materialize(program, &Options::default())?;
        let job = MaterializeJob::new(program, self.caps(), &Options::default())?;
        Ok(oxilite_core::run_async(&self.backend, job).await?)
    }
}
