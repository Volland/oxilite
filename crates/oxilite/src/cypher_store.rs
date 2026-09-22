//! `cypher()` on the blocking and async stores.
//!
// @lat: [[architecture#Property graph frontend]]

use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::job::Job;
use oxilite_core::{AsyncBackend, Step, SyncBackend};
use oxilite_cypher::{
    prepare_for, schema_query, CypherError, CypherOptions, CypherResult, CypherStep, Params,
    Schema, SqlCypherJob, StepInput,
};

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// Runs a Cypher statement over the property-graph view of the dataset.
    ///
    /// ```
    /// let store = oxilite::store::Store::new()?;
    /// store.cypher("CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {name: 'Alan'})")?;
    /// let r = store.cypher("MATCH (a:Person)-[:KNOWS]->(b) RETURN b.name")?;
    /// assert_eq!(r.rows[0][0], oxilite::cypher::Value::String("Alan".into()));
    /// # Result::<_, Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn cypher(&self, query: &str) -> Result<CypherResult, CypherError> {
        self.cypher_with(query, &Params::new(), &CypherOptions::default())
    }

    /// Runs a Cypher statement with parameters and options (vocabulary, reasoning…).
    ///
    /// Reads compile to SQL (with the spareval fallback for the rest); a writing statement
    /// runs inside one transaction: its read, then one atomic write request.
    pub fn cypher_with(
        &self,
        query: &str,
        params: &Params,
        options: &CypherOptions,
    ) -> Result<CypherResult, CypherError> {
        let mut job = prepare_for(query, params, options, self.caps())?;
        let backend = self.backend();
        let tx = job.writes() && self.caps().interactive_transactions;
        if tx {
            backend.begin()?;
        }
        let mut run = || -> Result<CypherResult, CypherError> {
            let mut input = None;
            loop {
                input = Some(match job.step(input)? {
                    CypherStep::Query(q, o) => StepInput::Output(self.evaluate(&q, &o)?),
                    CypherStep::Sql(r) => StepInput::Response(backend.execute(&r)?),
                    CypherStep::Write(r) => StepInput::Response(backend.execute(&r)?),
                    CypherStep::Done(r) => return Ok(r),
                });
            }
        };
        match run() {
            Ok(r) => {
                if tx {
                    backend.commit()?;
                }
                if r.schema_changed {
                    self.reload_stats()?;
                }
                Ok(r)
            }
            Err(e) => {
                if tx {
                    let _ = backend.rollback();
                }
                Err(e)
            }
        }
    }

    /// Reads the SHACL shapes of the dataset once, for [`CypherOptions::schema`].
    pub fn cypher_schema(&self) -> Result<std::sync::Arc<Schema>, CypherError> {
        let out = self.evaluate(&schema_query(), &Default::default())?;
        Ok(std::sync::Arc::new(Schema::from_output(out)?))
    }

    /// Describes how a Cypher statement runs: its SPARQL, the SQL it compiles to, and the
    /// clauses evaluated in Rust.
    pub fn explain_cypher(
        &self,
        query: &str,
        params: &Params,
        options: &CypherOptions,
    ) -> Result<String, CypherError> {
        let job = prepare_for(query, params, options, self.caps())?;
        let mut out = job.explain();
        for (q, o) in job.queries() {
            out.push_str(&self.explain_opt(q, &o)?);
            out.push('\n');
        }
        Ok(out)
    }
}

impl<B: AsyncBackend> AsyncStore<B> {
    /// Runs a Cypher statement (see [`Store::cypher`]). Parts that need the spareval
    /// fallback return [`CypherError::Store`] with an unsupported error.
    pub async fn cypher(&self, query: &str) -> Result<CypherResult, CypherError> {
        self.cypher_with(query, &Params::new(), &CypherOptions::default())
            .await
    }

    /// Reads the SHACL shapes of the dataset once, for [`CypherOptions::schema`].
    pub async fn cypher_schema(&self) -> Result<std::sync::Arc<Schema>, CypherError> {
        let out = self
            .query_output(schema_query(), &Default::default())
            .await?;
        Ok(std::sync::Arc::new(Schema::from_output(out)?))
    }

    /// Runs a Cypher statement with parameters and options. A writing statement reads, then
    /// applies its changes as one atomic request (one D1 batch).
    pub async fn cypher_with(
        &self,
        query: &str,
        params: &Params,
        options: &CypherOptions,
    ) -> Result<CypherResult, CypherError> {
        let job = prepare_for(query, params, options, self.caps())?;
        let stats = self.stats.borrow().clone();
        let mut sql = SqlCypherJob::new(job, stats, self.caps().clone(), options.query.clone());
        let mut response = None;
        let r = loop {
            let step = match sql.step(response.take()) {
                Ok(s) => s,
                Err(e) => return Err(sql.take_error().unwrap_or(CypherError::Store(e))),
            };
            match step {
                Step::Execute(request) => response = Some(self.backend.execute(&request).await?),
                Step::Done(out) => break out,
            }
        };
        if r.schema_changed {
            self.reload_stats().await?;
        }
        Ok(r)
    }
}
