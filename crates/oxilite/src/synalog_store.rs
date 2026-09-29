//! `synalog()` on the blocking and async stores.
//!
// @lat: [[architecture#Synalog frontend]]

use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::{AsyncBackend, SyncBackend};
use oxilite_synalog::{compile, prepare, Options, SynalogError, SynalogResult};

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// Runs a Synalog program over the store and returns the rows of one predicate.
    ///
    /// The store reads as relational tables: `triples(subject, predicate, object, kind,
    /// datatype, lang, graph)`, plus the predicate and class tables the program declares with
    /// `# @table NAME <IRI>` and `# @class NAME <IRI>`.
    ///
    /// ```
    /// # use oxilite::store::Store;
    /// let store = Store::new()?;
    /// store.update(
    ///     "PREFIX ex: <http://example.org/>
    ///      INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
    /// )?;
    /// let r = store.synalog(
    ///     "# @table parent <http://example.org/parent>
    ///      @Recursive(Ancestor, 10);
    ///      Ancestor(x:, y:) distinct :- parent(subject: x, object: y);
    ///      Ancestor(x:, y:) distinct :- Ancestor(x:, y: m), parent(subject: m, object: y);",
    ///     "Ancestor",
    /// )?;
    /// assert_eq!(r.rows.len(), 3);
    /// # Result::<_, Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn synalog(&self, program: &str, predicate: &str) -> Result<SynalogResult, SynalogError> {
        self.synalog_with(program, predicate, &Options::default())
    }

    /// Runs a Synalog program with options (graph scope, inferences, version, pagination).
    pub fn synalog_with(
        &self,
        program: &str,
        predicate: &str,
        options: &Options,
    ) -> Result<SynalogResult, SynalogError> {
        let mut options = options.clone();
        if let Some(v) = &options.as_of {
            options.as_of_tick = Some(self.resolve_version(v)?);
        }
        let job = prepare(program, predicate, self.caps(), &options)?;
        Ok(self.run(job)?)
    }

    /// The SQL a predicate compiles to on this store, without running it.
    pub fn synalog_sql(&self, program: &str, predicate: &str) -> Result<String, SynalogError> {
        self.synalog_sql_with(program, predicate, &Options::default())
    }

    /// The SQL a predicate compiles to with options (`as_of` is resolved against this store).
    pub fn synalog_sql_with(
        &self,
        program: &str,
        predicate: &str,
        options: &Options,
    ) -> Result<String, SynalogError> {
        let mut options = options.clone();
        if let Some(v) = &options.as_of {
            options.as_of_tick = Some(self.resolve_version(v)?);
        }
        Ok(compile(program, predicate, self.caps(), &options)?.sql)
    }
}

impl<B: AsyncBackend> AsyncStore<B> {
    /// Runs a Synalog program (see [`Store::synalog`]).
    pub async fn synalog(
        &self,
        program: &str,
        predicate: &str,
    ) -> Result<SynalogResult, SynalogError> {
        self.synalog_with(program, predicate, &Options::default())
            .await
    }

    /// Runs a Synalog program with options.
    pub async fn synalog_with(
        &self,
        program: &str,
        predicate: &str,
        options: &Options,
    ) -> Result<SynalogResult, SynalogError> {
        let mut options = options.clone();
        if let Some(v) = &options.as_of {
            options.as_of_tick = Some(self.resolve_version(v).await?);
        }
        let job = prepare(program, predicate, self.caps(), &options)?;
        Ok(oxilite_core::run_async(&self.backend, job).await?)
    }

    /// The SQL a predicate compiles to on this store, without running it.
    pub fn synalog_sql(&self, program: &str, predicate: &str) -> Result<String, SynalogError> {
        Ok(compile(program, predicate, self.caps(), &Options::default())?.sql)
    }
}
