//! `cypher()` on the blocking and async stores.
//!
// @lat: [[architecture#Property graph frontend]]

use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::job::Job;
use oxilite_core::{AsyncBackend, Step, SyncBackend};
use oxilite_cypher::{
    prepare_for, schema_command, CypherError, CypherOptions, CypherResult, CypherStep, Params,
    Schema, SchemaCommand, SqlCypherJob, StepInput, Value,
};
use std::borrow::Cow;

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
        if let Some(command) = schema_command(query)? {
            return self.cypher_schema_command(command, options);
        }
        let options = &*self.cypher_functions(options);
        // A version option is resolved once: every read of the statement sees that tick.
        let resolved;
        let options = match &options.query.as_of {
            Some(v) if options.query.as_of_tick.is_none() => {
                let mut o = options.clone();
                o.query.as_of_tick = Some(self.resolve_version(v)?);
                resolved = o;
                &resolved
            }
            _ => options,
        };
        let mut job = prepare_for(query, params, options, self.caps())?;
        if job.writes() && options.query.as_of.is_some() {
            return Err(versioned_write());
        }
        let backend = self.versioned();
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
        let index = self.shape_index().map_err(CypherError::Store)?;
        Ok(std::sync::Arc::new(Schema::from_index(index)))
    }

    /// The options with this store's host functions, unless the caller set its own.
    fn cypher_functions<'o>(&self, options: &'o CypherOptions) -> Cow<'o, CypherOptions> {
        let functions = self.host_functions();
        if options.functions.registry().is_some() || functions.is_empty() {
            Cow::Borrowed(options)
        } else {
            let mut o = options.clone();
            o.functions = functions;
            Cow::Owned(o)
        }
    }

    /// `CREATE VECTOR INDEX`, `DROP INDEX` and `SHOW VECTOR INDEXES`, with labels and property
    /// keys mapped to IRIs by the options' vocabulary.
    fn cypher_schema_command(
        &self,
        command: SchemaCommand,
        options: &CypherOptions,
    ) -> Result<CypherResult, CypherError> {
        use oxilite_core::vector::{ElementType, Metric, VectorIndex};
        let vocab = &options.vocabulary;
        let empty = |columns: Vec<String>, rows: Vec<Vec<Value>>| CypherResult {
            columns,
            rows,
            stats: Default::default(),
            schema_changed: false,
        };
        match command {
            SchemaCommand::CreateVectorIndex {
                name,
                if_not_exists,
                label,
                property,
                dimensions,
                similarity,
                element_type,
            } => {
                let exists = self
                    .stats()
                    .vector_indexes
                    .iter()
                    .any(|d| d.name.eq_ignore_ascii_case(&name));
                if exists && if_not_exists {
                    return Ok(empty(Vec::new(), Vec::new()));
                }
                let dimensions = dimensions.ok_or_else(|| {
                    CypherError::Semantic(format!(
                        "vector index {name} needs OPTIONS {{indexConfig: {{`vector.dimensions`: N}}}}"
                    ))
                })?;
                let mut index = VectorIndex::new(name, vocab.iri(&property), dimensions);
                if let Some(s) = similarity {
                    index.metric = Metric::parse(&s).ok_or_else(|| {
                        CypherError::Semantic(format!(
                            "unknown vector.similarity_function {s:?} (cosine, euclidean, dot, jaccard)"
                        ))
                    })?;
                    if index.metric == Metric::Jaccard {
                        index.element_type = ElementType::SparseFloat32;
                    }
                }
                if let Some(t) = element_type {
                    index.element_type = ElementType::parse(&t).ok_or_else(|| {
                        CypherError::Semantic(format!(
                            "unknown vector.element_type {t:?} (float32, float64, int8, bit1, sparse)"
                        ))
                    })?;
                }
                if let Some(l) = label {
                    index.class = Some(vocab.iri(&l));
                }
                self.create_vector_index(&index)?;
                Ok(empty(Vec::new(), Vec::new()))
            }
            SchemaCommand::DropIndex { name, if_exists } => {
                if !self.drop_vector_index(&name)? && !if_exists {
                    return Err(CypherError::Semantic(format!(
                        "there is no index named {name}"
                    )));
                }
                Ok(empty(Vec::new(), Vec::new()))
            }
            SchemaCommand::ShowVectorIndexes => {
                let columns = [
                    "name",
                    "label",
                    "property",
                    "dimensions",
                    "similarityFunction",
                    "elementType",
                    "rows",
                    "state",
                ]
                .map(String::from)
                .to_vec();
                let rows = self
                    .vector_indexes()?
                    .into_iter()
                    .map(|i| {
                        vec![
                            Value::String(i.index.name.clone()),
                            i.index
                                .class
                                .as_ref()
                                .map_or(Value::Null, |c| Value::String(vocab.name(c.as_str()))),
                            Value::String(vocab.name(i.index.property.as_str())),
                            Value::Int(i64::from(i.index.dimensions)),
                            Value::String(i.index.metric.name().into()),
                            Value::String(i.index.element_type.name().into()),
                            Value::Int(i.rows as i64),
                            Value::String(if i.built { "ONLINE" } else { "NOT BUILT" }.into()),
                        ]
                    })
                    .collect();
                Ok(empty(columns, rows))
            }
        }
    }

    /// Describes how a Cypher statement runs: its SPARQL, the SQL it compiles to, and the
    /// clauses evaluated in Rust.
    pub fn explain_cypher(
        &self,
        query: &str,
        params: &Params,
        options: &CypherOptions,
    ) -> Result<String, CypherError> {
        let options = &*self.cypher_functions(options);
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
        let index = self.shape_index().await.map_err(CypherError::Store)?;
        Ok(std::sync::Arc::new(Schema::from_index(index)))
    }

    /// Runs a Cypher statement with parameters and options. A writing statement reads, then
    /// applies its changes as one atomic request (one D1 batch).
    pub async fn cypher_with(
        &self,
        query: &str,
        params: &Params,
        options: &CypherOptions,
    ) -> Result<CypherResult, CypherError> {
        if schema_command(query)?.is_some() {
            return Err(CypherError::Unsupported(
                "vector index commands run on the blocking store (Store::cypher)".into(),
            ));
        }
        let resolved;
        let options = match &options.query.as_of {
            Some(v) if options.query.as_of_tick.is_none() => {
                let mut o = options.clone();
                o.query.as_of_tick = Some(self.resolve_version(v).await?);
                resolved = o;
                &resolved
            }
            _ => options,
        };
        let job = prepare_for(query, params, options, self.caps())?;
        if job.writes() && options.query.as_of.is_some() {
            return Err(versioned_write());
        }
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

/// A writing statement with a version option: writes apply to the current state.
fn versioned_write() -> CypherError {
    CypherError::Unsupported(
        "a writing statement cannot run at a past version: writes apply to the current state"
            .into(),
    )
}
