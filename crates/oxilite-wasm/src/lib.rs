//! The oxilite core compiled to WebAssembly, driven from JavaScript.
//!
//! JavaScript owns the database connection (a Cloudflare D1 binding, …). Every operation is a
//! [`Job`]: call `step(null)`, and while the result is `{"execute": request}` run the request
//! on the database and call `step(response)`; `{"done": value}` ends the job. Requests and
//! responses are JSON (see `oxilite_core::sql`), results use `oxilite_core::json`.
//!
// @lat: [[architecture#Backends#JavaScript drivers]]

use oxilite_core::job::{Job as CoreJob, Sequence, Step};
use oxilite_core::json::{
    json_to_graph, json_to_quad, json_to_term, output_to_format, output_to_json,
    output_to_sparql_json, quad_to_json, JsQueryOptions,
};
use oxilite_core::query::{compile_query, QueryJob};
use oxilite_core::update::{explain_plan, plan_update_with, PlannedOp};
use oxilite_core::version;
use oxilite_core::{
    ops, Capabilities, CommitInfo, Error, QueryOptions, Request, Response, Stats, StoreOptions,
};
use oxrdf::{GraphName, NamedOrBlankNode, Quad, Term};
use oxrdfio::{RdfFormat, RdfParser, RdfSerializer};
use serde_json::{json, Value};
use spargebra::SparqlParser;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

#[cfg(feature = "jsonld")]
mod jsonld;

fn js(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

type Stepper = Box<dyn FnMut(Option<Response>) -> oxilite_core::Result<Step<Value>>>;

/// A resumable operation (see the crate documentation).
#[wasm_bindgen]
pub struct Job {
    pub(crate) step: Stepper,
    /// The engine's versioning state: when set, atomic writes of a versioned store open a tick
    /// (see `oxilite_core::version`).
    pub(crate) ctx: Option<VersionCtx>,
    pending: bool,
}

/// What a job needs to open ticks: the store's level (in its statistics) and the commit info.
#[derive(Clone)]
pub(crate) struct VersionCtx {
    stats: Rc<RefCell<Stats>>,
    info: Rc<RefCell<CommitInfo>>,
}

impl Job {
    pub(crate) fn new(step: Stepper) -> Self {
        Self {
            step,
            ctx: None,
            pending: false,
        }
    }
}

#[wasm_bindgen]
impl Job {
    /// Advances the job: `response` is the JSON response to the previous request (or null).
    /// Returns `{"execute": request}` or `{"done": value}` as JSON.
    pub fn step(&mut self, response: Option<String>) -> Result<String, JsError> {
        let response = response
            .map(|r| serde_json::from_str::<Response>(&r))
            .transpose()
            .map_err(js)?;
        let response = if std::mem::take(&mut self.pending) {
            response.map(version::strip)
        } else {
            response
        };
        Ok(match (self.step)(response).map_err(js)? {
            Step::Execute(r) => {
                let prepared = self.ctx.as_ref().and_then(|c| {
                    let level = c.stats.borrow().version.level;
                    version::prepare(&r, level, &c.info.borrow())
                });
                match prepared {
                    Some(p) => {
                        self.pending = true;
                        json!({ "execute": p }).to_string()
                    }
                    None => json!({ "execute": r }).to_string(),
                }
            }
            Step::Done(v) => json!({ "done": v }).to_string(),
        })
    }
}

fn wrap_raw<J: CoreJob + 'static>(
    mut job: J,
    done: impl Fn(J::Output) -> oxilite_core::Result<Value> + 'static,
) -> Job {
    Job::new(Box::new(move |r| match job.step(r)? {
        Step::Execute(req) => Ok(Step::Execute(req)),
        Step::Done(o) => Ok(Step::Done(done(o)?)),
    }))
}

#[cfg(feature = "datalog")]
fn datalog_options(options: Option<String>) -> Result<oxilite_datalog::Options, JsError> {
    let value = match options {
        Some(o) => serde_json::from_str::<Value>(&o).map_err(js)?,
        None => Value::Null,
    };
    oxilite_datalog::json::options_from_json(&value).map_err(js)
}

fn changes_to_json(changes: &[version::Change]) -> Value {
    Value::Array(
        changes
            .iter()
            .map(|c| json!({"tick": c.tick, "added": c.added, "quad": quad_to_json(&c.quad)}))
            .collect(),
    )
}

fn ok() -> oxilite_core::Result<Value> {
    Ok(json!({"kind": "ok"}))
}

fn format(name: &str) -> Result<RdfFormat, JsError> {
    RdfFormat::from_media_type(name)
        .or_else(|| RdfFormat::from_extension(name))
        .ok_or_else(|| js(format!("unknown RDF format {name}")))
}

/// The oxilite engine for one database.
#[wasm_bindgen]
pub struct Engine {
    /// The backend's capabilities; jobs get [`Engine::caps`], adjusted to the store's level.
    base_caps: Capabilities,
    options: StoreOptions,
    stats: Rc<RefCell<Stats>>,
    info: Rc<RefCell<CommitInfo>>,
}

impl Engine {
    /// The capabilities jobs see: writers stamp quads, and a statement is kept for the tick.
    fn caps(&self) -> Capabilities {
        version::effective_caps(&self.base_caps, self.stats.borrow().version.level)
    }

    fn version_ctx(&self) -> VersionCtx {
        VersionCtx {
            stats: Rc::clone(&self.stats),
            info: Rc::clone(&self.info),
        }
    }

    /// Makes `job` open a tick for its writes when the store is versioned.
    fn bind(&self, mut job: Job) -> Job {
        job.ctx = Some(self.version_ctx());
        job
    }

    fn wrap<J: CoreJob + 'static>(
        &self,
        job: J,
        done: impl Fn(J::Output) -> oxilite_core::Result<Value> + 'static,
    ) -> Job {
        self.bind(wrap_raw(job, done))
    }
}

#[wasm_bindgen]
impl Engine {
    /// `capabilities` (JSON, optional) defaults to Cloudflare D1's; `options` (JSON,
    /// optional) are the store options, e.g. `{"graphIndex": false}`.
    #[wasm_bindgen(constructor)]
    pub fn new(capabilities: Option<String>, options: Option<String>) -> Result<Engine, JsError> {
        let caps = match capabilities {
            Some(c) => serde_json::from_str(&c).map_err(js)?,
            None => Capabilities::d1(),
        };
        let options = match options {
            Some(o) => serde_json::from_str(&o).map_err(js)?,
            None => StoreOptions::default(),
        };
        Ok(Self {
            base_caps: caps,
            options,
            stats: Rc::default(),
            info: Rc::default(),
        })
    }

    /// A JSON-LD document or (with `"credentials": true` in `options`) Verifiable Credentials
    /// operation: `schema`, `put`, `get`, `remove`, `list`, `find`, `graphs`,
    /// `documentForGraph`, `putContext`, `removeContext`, `contexts`, `rebuild`, `check`,
    /// `putCredential`, `putPresentation`. `args` and `options` are JSON
    /// (`oxilite_jsonld::json`); errors read `oxilite-jsonld:{"code", "message"}`.
    #[cfg(feature = "jsonld")]
    pub fn jsonld(&self, op: &str, args: &str, options: Option<String>) -> Result<Job, JsError> {
        let args: Value = serde_json::from_str(args).map_err(js)?;
        let opts: Value = match options {
            Some(o) => serde_json::from_str(&o).map_err(js)?,
            None => Value::Null,
        };
        jsonld::jsonld_job(op, &args, &opts, &self.caps())
            .map(|j| self.bind(j))
            .map_err(js)
    }

    /// The schema with the JSON-LD tables, as a SQL script; `indexes` (JSON) picks the
    /// metadata indexes.
    #[cfg(feature = "jsonld")]
    #[wasm_bindgen(js_name = jsonldSchemaSql)]
    pub fn jsonld_schema_sql(&self, indexes: Option<String>) -> Result<String, JsError> {
        let indexes: Value = match indexes {
            Some(i) => serde_json::from_str(&i).map_err(js)?,
            None => json!({}),
        };
        jsonld::schema_sql(&self.options, &indexes).map_err(js)
    }

    /// The schema as a SQL script (for `wrangler d1 migrations`).
    #[wasm_bindgen(js_name = schemaSql)]
    pub fn schema_sql(&self) -> String {
        oxilite_core::schema::schema_sql(&self.options)
    }

    fn stats_job<J: CoreJob<Output = Stats> + 'static>(&self, job: J) -> Job {
        let stats = Rc::clone(&self.stats);
        self.wrap(job, move |s| {
            *stats.borrow_mut() = s;
            ok()
        })
    }

    /// Runs `job`, then reloads statistics (the in-memory reasoning facts) when `reload`.
    fn reloading(&self, job: Job, reload: bool) -> Job {
        if !reload {
            return job;
        }
        let stats = Rc::clone(&self.stats);
        let caps = self.caps();
        let ctx = job.ctx.clone();
        let mut inner = job.step;
        let mut value: Option<Value> = None;
        let mut reloader: Option<oxilite_core::job::OneShot<Stats>> = None;
        Job {
            ctx,
            pending: false,
            step: Box::new(move |r| {
                let r = match reloader.as_mut() {
                    Some(j) => j.step(r)?,
                    None => match inner(r)? {
                        Step::Execute(q) => return Ok(Step::Execute(q)),
                        Step::Done(v) => {
                            value = Some(v);
                            reloader.insert(ops::stats_job(&caps)).step(None)?
                        }
                    },
                };
                match r {
                    Step::Execute(q) => Ok(Step::Execute(q)),
                    Step::Done(s) => {
                        *stats.borrow_mut() = s;
                        Ok(Step::Done(value.take().unwrap_or(Value::Null)))
                    }
                }
            }),
        }
    }

    /// Computes the OWL 2 RL closure into the inference table (one batch per rule round);
    /// the result is `{"kind": "number"}`, the number of inferred triples.
    pub fn materialize(&self) -> Job {
        self.wrap(ops::materialize_job(1000, &self.caps()), |n| {
            Ok(json!({"kind": "number", "value": n}))
        })
    }

    /// Removes every materialized inference.
    #[wasm_bindgen(js_name = clearInferences)]
    pub fn clear_inferences(&self) -> Job {
        self.wrap(ops::clear_inferences_job(), |_| ok())
    }

    // The schema registry is RDF in `<oxilite:schema>`: these build the portable SPARQL that
    // registers, maps, lists and drops schema graphs; the driver runs it with `query` and
    // `update` (whose statistics reload covers registry changes).

    /// SPARQL registering a graph (a JSON term) as `ontology`, `shacl` or `shex`;
    /// `registration` is JSON (`{iri, version, sha256, imports, appliesTo, active}`).
    #[wasm_bindgen(js_name = registerSchemaGraphSparql)]
    pub fn register_schema_graph_sparql(
        &self,
        graph: &str,
        role: &str,
        registration: Option<String>,
    ) -> Result<String, JsError> {
        let role: oxilite_core::registry::SchemaRole = role.parse().map_err(js)?;
        let registration: Value = match registration {
            Some(r) => serde_json::from_str(&r).map_err(js)?,
            None => Value::Null,
        };
        let e =
            oxilite_core::json::schema_graph_from_json(parse_graph(graph)?, role, &registration)
                .map_err(js)?;
        oxilite_core::registry::register_update(&e).map_err(js)
    }

    /// SPARQL removing a registration (the graph's triples stay).
    #[wasm_bindgen(js_name = unregisterSchemaGraphSparql)]
    pub fn unregister_schema_graph_sparql(&self, graph: &str) -> Result<String, JsError> {
        oxilite_core::registry::unregister_update(parse_graph(graph)?.as_ref()).map_err(js)
    }

    /// SPARQL activating or deactivating a registration.
    #[wasm_bindgen(js_name = setSchemaGraphActiveSparql)]
    pub fn set_schema_graph_active_sparql(
        &self,
        graph: &str,
        active: bool,
    ) -> Result<String, JsError> {
        oxilite_core::registry::set_active_update(parse_graph(graph)?.as_ref(), active).map_err(js)
    }

    /// SPARQL removing a registration and every triple of its graph.
    #[wasm_bindgen(js_name = dropSchemaGraphSparql)]
    pub fn drop_schema_graph_sparql(&self, graph: &str) -> Result<String, JsError> {
        oxilite_core::registry::drop_update(parse_graph(graph)?.as_ref()).map_err(js)
    }

    /// SPARQL `ASK`: is the graph registered?
    #[wasm_bindgen(js_name = schemaGraphRegisteredSparql)]
    pub fn schema_graph_registered_sparql(&self, graph: &str) -> Result<String, JsError> {
        oxilite_core::registry::registered_query(parse_graph(graph)?.as_ref()).map_err(js)
    }

    /// SPARQL counting a graph's triples (`?n`).
    #[wasm_bindgen(js_name = graphSizeSparql)]
    pub fn graph_size_sparql(&self, graph: &str) -> Result<String, JsError> {
        Ok(oxilite_core::registry::size_query(
            parse_graph(graph)?.as_ref(),
        ))
    }

    /// SPARQL installing or refreshing the system graphs (the vocabulary, the registry's own
    /// description).
    #[wasm_bindgen(js_name = systemGraphsSparql)]
    pub fn system_graphs_sparql(&self) -> String {
        oxilite_core::registry::system_graphs_update()
    }

    /// SPARQL `ASK`: are the system graphs installed at the current vocabulary version?
    #[wasm_bindgen(js_name = systemGraphsReadySparql)]
    pub fn system_graphs_ready_sparql(&self) -> String {
        oxilite_core::registry::system_graphs_ready_query()
    }

    /// SPARQL reading the registry; parse its output with `schemaGraphsFromOutput`.
    #[wasm_bindgen(js_name = schemaGraphsSparql)]
    pub fn schema_graphs_sparql(&self) -> String {
        oxilite_core::registry::entries_query()
    }

    /// The registry entries (JSON `[{graph, role, iri, version, sha256, imports, appliesTo,
    /// active, loadedAt}]`) from the JSON output of the `schemaGraphsSparql` query.
    #[wasm_bindgen(js_name = schemaGraphsFromOutput)]
    pub fn schema_graphs_from_output(&self, output: &str) -> Result<String, JsError> {
        let v: Value = serde_json::from_str(output).map_err(js)?;
        let rows = v
            .get("rows")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|r| {
                        r.as_array()
                            .map(|cells| {
                                cells
                                    .iter()
                                    .map(|c| (!c.is_null()).then(|| json_to_term(c).ok()).flatten())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let entries = oxilite_core::registry::entries_from_rows(&rows);
        Ok(Value::Array(
            entries
                .iter()
                .map(oxilite_core::json::schema_graph_to_json)
                .collect(),
        )
        .to_string())
    }

    /// The compiled SHACL property shapes: `[{target, path, datatype, minCount, maxCount,
    /// pattern, in, relationship}]`.
    #[wasm_bindgen(js_name = shapeIndex)]
    pub fn shape_index(&self) -> Job {
        self.wrap(ops::shape_index_job(&self.caps()), |i| {
            Ok(oxilite_core::json::shape_index_to_json(&i))
        })
    }

    /// Creates the schema if needed and loads planner statistics.
    pub fn open(&self) -> Job {
        self.stats_job(ops::open_job(&self.options, &self.caps()))
    }

    /// Loads planner statistics only (schema applied by a migration).
    #[wasm_bindgen(js_name = openExisting)]
    pub fn open_existing(&self) -> Job {
        self.stats_job(ops::stats_job(&self.caps()))
    }

    /// Recomputes planner statistics.
    pub fn optimize(&self) -> Job {
        self.stats_job(ops::optimize_job(&self.caps()))
    }

    fn compiled(
        &self,
        sparql: &str,
        options: &JsQueryOptions,
    ) -> Result<oxilite_core::CompiledQuery, JsError> {
        let mut parser = SparqlParser::new();
        if let Some(b) = &options.base_iri {
            parser = parser.with_base_iri(b).map_err(js)?;
        }
        let q = parser.parse_query(sparql).map_err(js)?;
        compile_query(
            &q,
            &self.stats.borrow(),
            &self.caps(),
            &options.to_options().map_err(js)?,
        )
        .map_err(js)
    }

    /// A SPARQL query. `options` (JSON) uses Oxigraph's JS names: `base_iri`,
    /// `use_default_graph_as_union`, `default_graph`, `named_graphs`, `results_format`. The
    /// result is `oxilite_core::json::output_to_json`, or `{"kind": "text"}` when a
    /// `results_format` is given.
    pub fn query(&self, sparql: &str, options: Option<String>) -> Result<Job, JsError> {
        let options: JsQueryOptions = match options {
            Some(o) => serde_json::from_str(&o).map_err(js)?,
            None => JsQueryOptions::default(),
        };
        let format = options.results_format.clone();
        let done = move |o: oxilite_core::QueryOutput| match &format {
            Some(f) => Ok(json!({"kind": "text", "value": output_to_format(&o, f)?})),
            None => Ok(output_to_json(&o)),
        };
        let mut parser = SparqlParser::new();
        if let Some(b) = &options.base_iri {
            parser = parser.with_base_iri(b).map_err(js)?;
        }
        let q = parser.parse_query(sparql).map_err(js)?;
        let core_options = options.to_options().map_err(js)?;
        let refs = version::query_version_refs(&q, &core_options);
        if refs.is_empty() {
            let c = self.compiled(sparql, &options)?;
            return Ok(self.wrap(QueryJob::new(c, self.caps()), done));
        }
        // Resolve the versions the query names, then compile against their ticks.
        let (stats, caps) = (self.stats.borrow().clone(), self.caps());
        let resolve = version::resolve_query_job(refs, stats.version, core_options).map_err(js)?;
        let job = oxilite_core::job::Then::new(resolve, move |o| {
            Ok(QueryJob::new(compile_query(&q, &stats, &caps, &o)?, caps))
        });
        Ok(self.wrap(job, done))
    }

    /// A SPARQL query whose result is SPARQL 1.1 JSON results (`{"kind": "text", "value"}`).
    #[wasm_bindgen(js_name = queryJson)]
    pub fn query_json(&self, sparql: &str) -> Result<Job, JsError> {
        let c = self.compiled(sparql, &JsQueryOptions::default())?;
        Ok(self.wrap(QueryJob::new(c, self.caps()), |o| {
            Ok(json!({"kind": "text", "value": output_to_sparql_json(&o)?}))
        }))
    }

    /// The SQL a query compiles to, with the planner's notes.
    pub fn explain(&self, sparql: &str) -> Result<String, JsError> {
        let q = SparqlParser::new().parse_query(sparql).map_err(js)?;
        Ok(
            match compile_query(
                &q,
                &self.stats.borrow(),
                &self.caps(),
                &QueryOptions::default(),
            ) {
                Ok(c) => c.explain(),
                Err(e) => format!("-- oxilite: unsupported on this backend: {e}"),
            },
        )
    }

    /// A SPARQL update, applied as one atomic request (one D1 batch).
    pub fn update(&self, sparql: &str, base_iri: Option<String>) -> Result<Job, JsError> {
        let mut parser = SparqlParser::new();
        if let Some(b) = base_iri {
            parser = parser.with_base_iri(b).map_err(js)?;
        }
        let u = parser.parse_update(sparql).map_err(js)?;
        let mut stmts = Vec::new();
        for p in plan_update_with(
            &u,
            &self.stats.borrow(),
            &self.caps(),
            &QueryOptions::default(),
        )
        .map_err(js)?
        {
            match p {
                PlannedOp::Sql(s) => stmts.extend(s),
                PlannedOp::Fallback(_, why) => {
                    return Err(js(Error::unsupported(format!("{why} on D1"))))
                }
            }
        }
        let job = self.wrap(
            Sequence::new(if stmts.is_empty() {
                Vec::new()
            } else {
                vec![Request::atomic(stmts)]
            }),
            |_| ok(),
        );
        Ok(self.reloading(job, oxilite_core::reason::update_touches_schema(&u)))
    }

    #[cfg(feature = "cypher-lite")]
    /// A Cypher statement over the property-graph view (see `oxilite-cypher`). `params` and
    /// `options` are JSON (`oxilite_cypher::json`). The result is `{"kind": "cypher",
    /// "columns", "rows", "stats"}`; a writing statement applies its changes as one atomic
    /// request (one D1 batch).
    pub fn cypher(
        &self,
        query: &str,
        params: Option<String>,
        options: Option<String>,
    ) -> Result<Job, JsError> {
        let (job, opts) = self.cypher_job(query, params, options)?;
        let mut sql = oxilite_cypher::SqlCypherJob::new(
            job,
            self.stats.borrow().clone(),
            self.caps(),
            opts.query.clone(),
        );
        let stats = Rc::clone(&self.stats);
        let caps = self.caps();
        let mut value: Option<Value> = None;
        let mut reloader: Option<oxilite_core::job::OneShot<Stats>> = None;
        Ok(Job {
            ctx: Some(self.version_ctx()),
            pending: false,
            step: Box::new(move |r| {
                if let Some(j) = reloader.as_mut() {
                    return match j.step(r)? {
                        Step::Execute(q) => Ok(Step::Execute(q)),
                        Step::Done(s) => {
                            *stats.borrow_mut() = s;
                            Ok(Step::Done(value.take().unwrap_or(Value::Null)))
                        }
                    };
                }
                let step = match sql.step(r) {
                    Ok(s) => s,
                    Err(e) => {
                        return Err(match sql.take_error() {
                            Some(c) => Error::Other(c.to_string()),
                            None => e,
                        })
                    }
                };
                match step {
                    Step::Execute(q) => Ok(Step::Execute(q)),
                    Step::Done(res) => {
                        let mut v = res.to_json();
                        v["kind"] = json!("cypher");
                        if !res.schema_changed {
                            return Ok(Step::Done(v));
                        }
                        // Ontology triples changed: reload the reasoning facts.
                        value = Some(v);
                        match reloader.insert(ops::stats_job(&caps)).step(None)? {
                            Step::Execute(q) => Ok(Step::Execute(q)),
                            Step::Done(s) => {
                                *stats.borrow_mut() = s;
                                Ok(Step::Done(value.take().unwrap_or(Value::Null)))
                            }
                        }
                    }
                }
            }),
        })
    }

    #[cfg(feature = "datalog")]
    /// A Datalog program over the same quads (see `oxilite-datalog`). `options` is JSON
    /// (`oxilite_datalog::json`). The result is `{"kind": "datalog", "columns", "rows",
    /// "rounds"}`.
    ///
    /// A program whose recursion is linear is two requests; a component that has to be
    /// iterated adds one request per round, which `rounds` reports.
    pub fn datalog(&self, program: &str, options: Option<String>) -> Result<Job, JsError> {
        let mut options = datalog_options(options)?;
        options.history = Some(self.stats.borrow().version);
        let done =
            |r: oxilite_datalog::DatalogResult| Ok(oxilite_datalog::json::result_to_json(&r));
        let (whole, atoms) = oxilite_datalog::version_refs(program, &options).map_err(js)?;
        if whole.is_none() && atoms.is_empty() {
            let job = oxilite_datalog::prepare(program, &self.caps(), &options).map_err(js)?;
            return Ok(self.wrap(job, done));
        }
        if options.include_inferred {
            return Err(js(
                "inferences describe the current state only; they cannot be combined with a version",
            ));
        }
        // Resolve the versions first (the program's, then its atoms'), then compile.
        let mut refs: Vec<String> = whole.iter().cloned().collect();
        refs.extend(atoms.iter().cloned());
        let parsed = refs
            .iter()
            .map(|r| r.parse())
            .collect::<Result<Vec<_>, _>>()
            .map_err(js)?;
        let resolve = version::resolve_job(parsed, self.stats.borrow().version);
        let (program, caps) = (program.to_owned(), self.caps());
        let has_whole = whole.is_some();
        let job = oxilite_core::job::Then::new(resolve, move |ticks: Vec<i64>| {
            let mut o = options;
            let mut it = refs.into_iter().zip(ticks);
            if has_whole {
                o.as_of_tick = it.next().map(|(_, t)| t);
            }
            o.versions.extend(it);
            oxilite_datalog::prepare(&program, &caps, &o).map_err(|e| Error::Other(e.to_string()))
        });
        Ok(self.wrap(job, done))
    }

    #[cfg(feature = "datalog")]
    /// Stores what a Datalog program derives as inferences, in the table OWL 2 RL
    /// materialization uses. The result is `{"kind": "datalogMaterialize", "inferred",
    /// "relations"}`.
    pub fn datalog_materialize(
        &self,
        program: &str,
        options: Option<String>,
    ) -> Result<Job, JsError> {
        let mut options = datalog_options(options)?;
        options.history = Some(self.stats.borrow().version);
        let job =
            oxilite_datalog::MaterializeJob::new(program, &self.caps(), &options).map_err(js)?;
        Ok(self.wrap(job, |s| Ok(oxilite_datalog::json::stats_to_json(&s))))
    }

    #[cfg(feature = "datalog")]
    /// Describes how a Datalog program runs: its strata, the strategy chosen for each
    /// recursive component, and the SQL.
    pub fn explain_datalog(
        &self,
        program: &str,
        options: Option<String>,
    ) -> Result<String, JsError> {
        let options = datalog_options(options)?;
        oxilite_datalog::explain(program, &self.caps(), &options).map_err(js)
    }

    #[cfg(feature = "cypher-lite")]
    fn cypher_job(
        &self,
        query: &str,
        params: Option<String>,
        options: Option<String>,
    ) -> Result<(oxilite_cypher::CypherJob, oxilite_cypher::CypherOptions), JsError> {
        let params = match params {
            Some(p) => oxilite_cypher::json::params_from_json(&p).map_err(js)?,
            None => oxilite_cypher::Params::new(),
        };
        let opts = match options {
            Some(o) => oxilite_cypher::json::options_from_json(&o).map_err(js)?,
            None => oxilite_cypher::CypherOptions::default(),
        };
        if opts.query.as_of.is_some() && opts.query.as_of_tick.is_none() {
            return Err(js(
                "resolve the version first (resolveVersion) and pass its tick as asOfTick",
            ));
        }
        let job = oxilite_cypher::prepare_for(query, &params, &opts, &self.caps()).map_err(js)?;
        if job.writes() && opts.query.as_of_tick.is_some() {
            return Err(js(
                "a writing statement cannot run at a past version: writes apply to the current state",
            ));
        }
        Ok((job, opts))
    }

    /// How a Cypher statement runs: its SPARQL, the SQL it compiles to, and what runs in Rust.
    #[cfg(feature = "cypher-lite")]
    #[wasm_bindgen(js_name = explainCypher)]
    pub fn explain_cypher(
        &self,
        query: &str,
        params: Option<String>,
        options: Option<String>,
    ) -> Result<String, JsError> {
        let (job, opts) = self.cypher_job(query, params, options)?;
        let mut out = job.explain();
        let _ = &opts;
        for (q, o) in job.queries() {
            match compile_query(q, &self.stats.borrow(), &self.caps(), &o) {
                Ok(c) => out.push_str(&c.explain()),
                Err(e) => out.push_str(&format!("-- oxilite: unsupported on this backend: {e}")),
            }
            out.push('\n');
        }
        Ok(out)
    }

    /// How an update would run (SQL per operation).
    #[wasm_bindgen(js_name = explainUpdate)]
    pub fn explain_update(&self, sparql: &str) -> Result<String, JsError> {
        let u = SparqlParser::new().parse_update(sparql).map_err(js)?;
        let plan = plan_update_with(
            &u,
            &self.stats.borrow(),
            &self.caps(),
            &QueryOptions::default(),
        )
        .map_err(js)?;
        Ok(explain_plan(&plan))
    }

    fn parse(
        &self,
        data: &str,
        format_name: &str,
        base: Option<String>,
        graph: Option<String>,
    ) -> Result<Vec<Quad>, JsError> {
        let mut parser = RdfParser::from_format(format(format_name)?).rename_blank_nodes();
        if let Some(b) = base {
            parser = parser.with_base_iri(b).map_err(js)?;
        }
        if let Some(g) = graph {
            parser = parser.with_default_graph(
                json_to_graph(&serde_json::from_str(&g).map_err(js)?).map_err(js)?,
            );
        }
        parser
            .for_slice(data.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(js)
    }

    /// Loads a document atomically (one batch). `graph` is a JSON graph term.
    pub fn load(
        &self,
        data: &str,
        format_name: &str,
        base: Option<String>,
        graph: Option<String>,
    ) -> Result<Job, JsError> {
        let quads = self.parse(data, format_name, base, graph)?;
        let mut req = ops::insert_request(quads.iter().map(Quad::as_ref), &self.caps());
        let schema = quads
            .iter()
            .any(|q| oxilite_core::reason::is_schema_quad(q.as_ref()));
        req.statements
            .extend(ops::schema_refresh_for(quads.iter().map(Quad::as_ref)));
        if req.statements.len() > self.caps().max_statements {
            return Err(js(format!(
                "the document needs {} statements, more than one D1 batch allows ({}); use bulkLoad",
                req.statements.len(),
                self.caps().max_statements
            )));
        }
        Ok(self.reloading(self.wrap(Sequence::new(vec![req]), |_| ok()), schema))
    }

    /// Loads a document in several batches (not atomic), then refreshes statistics.
    #[wasm_bindgen(js_name = bulkLoad)]
    pub fn bulk_load(
        &self,
        data: &str,
        format_name: &str,
        base: Option<String>,
        graph: Option<String>,
    ) -> Result<Job, JsError> {
        let quads = self.parse(data, format_name, base, graph)?;
        let mut requests = Vec::new();
        let mut chunk = 2_000usize;
        let mut start = 0;
        while start < quads.len() {
            let end = (start + chunk).min(quads.len());
            let req = ops::insert_request(quads[start..end].iter().map(Quad::as_ref), &self.caps());
            if req.statements.len() > self.caps().max_statements && chunk > 1 {
                chunk /= 2;
                continue;
            }
            requests.push(req);
            start = end;
        }
        let mut load = Sequence::new(requests);
        let mut optimize: Option<Box<dyn CoreJob<Output = Stats>>> = None;
        let caps = self.caps();
        let stats = Rc::clone(&self.stats);
        let mut loading = true;
        Ok(Job {
            ctx: Some(self.version_ctx()),
            pending: false,
            step: Box::new(move |r| {
                if loading {
                    match load.step(r)? {
                        Step::Execute(req) => return Ok(Step::Execute(req)),
                        Step::Done(_) => {
                            loading = false;
                            optimize = Some(Box::new(ops::optimize_job(&caps)));
                            return match optimize.as_mut().expect("set").step(None)? {
                                Step::Execute(req) => Ok(Step::Execute(req)),
                                Step::Done(s) => {
                                    *stats.borrow_mut() = s;
                                    ok().map(Step::Done)
                                }
                            };
                        }
                    }
                }
                match optimize.as_mut().expect("set").step(r)? {
                    Step::Execute(req) => Ok(Step::Execute(req)),
                    Step::Done(s) => {
                        *stats.borrow_mut() = s;
                        ok().map(Step::Done)
                    }
                }
            }),
        })
    }

    /// Inserts quads (JSON array of RDF/JS quads) atomically.
    pub fn add(&self, quads: &str) -> Result<Job, JsError> {
        let quads = parse_quads(quads)?;
        let schema = quads
            .iter()
            .any(|q| oxilite_core::reason::is_schema_quad(q.as_ref()));
        let job = self.wrap(
            ops::insert_job(quads.iter().map(Quad::as_ref), &self.caps()),
            |n| Ok(json!({"kind": "number", "value": n})),
        );
        Ok(self.reloading(job, schema))
    }

    /// Removes quads (JSON array of RDF/JS quads) atomically.
    pub fn delete(&self, quads: &str) -> Result<Job, JsError> {
        let quads = parse_quads(quads)?;
        let schema = quads
            .iter()
            .any(|q| oxilite_core::reason::is_schema_quad(q.as_ref()));
        let job = self.wrap(
            ops::remove_job(quads.iter().map(Quad::as_ref), &self.caps()),
            |n| Ok(json!({"kind": "number", "value": n})),
        );
        Ok(self.reloading(job, schema))
    }

    /// Does the store contain a quad (JSON RDF/JS quad)?
    pub fn has(&self, quad: &str) -> Result<Job, JsError> {
        let q = json_to_quad(&serde_json::from_str(quad).map_err(js)?).map_err(js)?;
        Ok(self.wrap(ops::contains_job(q.as_ref()), |b| {
            Ok(json!({"kind": "boolean", "value": b}))
        }))
    }

    /// Quads matching a pattern; each argument is a JSON term or null.
    #[wasm_bindgen(js_name = "match")]
    pub fn match_(
        &self,
        subject: Option<String>,
        predicate: Option<String>,
        object: Option<String>,
        graph: Option<String>,
    ) -> Result<Job, JsError> {
        let term = |t: Option<String>| -> Result<Option<Term>, JsError> {
            t.map(|t| json_to_term(&serde_json::from_str(&t).map_err(js)?).map_err(js))
                .transpose()
        };
        let s = match term(subject)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(NamedOrBlankNode::from(n)),
            Some(Term::BlankNode(b)) => Some(NamedOrBlankNode::from(b)),
            Some(_) => {
                return Ok(self.wrap(Sequence::new(Vec::new()), |_| {
                    Ok(json!({"kind": "quads", "quads": []}))
                }))
            }
        };
        let p = match term(predicate)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(n),
            Some(_) => {
                return Ok(self.wrap(Sequence::new(Vec::new()), |_| {
                    Ok(json!({"kind": "quads", "quads": []}))
                }))
            }
        };
        let o = term(object)?;
        let g: Option<GraphName> = graph
            .map(|g| json_to_graph(&serde_json::from_str(&g).map_err(js)?).map_err(js))
            .transpose()?;
        Ok(self.wrap(
            ops::scan_job(
                s.as_ref().map(Into::into),
                p.as_ref().map(Into::into),
                o.as_ref().map(Into::into),
                g.as_ref().map(Into::into),
                &self.caps(),
            ),
            |quads| {
                Ok(
                    json!({"kind": "quads", "quads": quads.iter().map(quad_to_json).collect::<Vec<_>>()}),
                )
            },
        ))
    }

    /// Number of quads.
    pub fn size(&self) -> Job {
        self.wrap(ops::len_job(), |n| {
            Ok(json!({"kind": "number", "value": n}))
        })
    }

    /// Serializes the dataset (or one graph, JSON graph term) in an RDF format.
    pub fn dump(&self, format_name: &str, graph: Option<String>) -> Result<Job, JsError> {
        let format = format(format_name)?;
        let g: Option<GraphName> = graph
            .map(|g| json_to_graph(&serde_json::from_str(&g).map_err(js)?).map_err(js))
            .transpose()?;
        let dataset_format = format.supports_datasets();
        Ok(self.wrap(
            ops::scan_job(None, None, None, g.as_ref().map(Into::into), &self.caps()),
            move |quads| {
                let mut s = RdfSerializer::from_format(format).for_writer(Vec::new());
                for q in &quads {
                    if dataset_format {
                        s.serialize_quad(q)?;
                    } else {
                        s.serialize_triple(q.as_ref())?;
                    }
                }
                Ok(json!({"kind": "text", "value": String::from_utf8_lossy(&s.finish()?)}))
            },
        ))
    }

    /// Removes everything.
    /// The versioning level and where the clock and history stand (`oxilite_core::version`).
    pub fn versioning(&self) -> Job {
        self.wrap(version::status_job(self.stats.borrow().version), |s| {
            serde_json::to_value(s).map_err(Error::backend)
        })
    }

    /// Changes the versioning level (`off`, `stamped`, `log`); `change` (JSON) holds
    /// `asOfIndex`, `stampIndex`, `allowLoss`, `author`, `message`. The result is the new
    /// status. On D1, prefer a migration (`levelChangeSql`).
    #[wasm_bindgen(js_name = setVersioning)]
    pub fn set_versioning(&self, level: &str, change: Option<String>) -> Result<Job, JsError> {
        let change: version::LevelChange = match change {
            Some(c) => serde_json::from_str(&c).map_err(js)?,
            None => Default::default(),
        };
        let state = self.stats.borrow().version;
        let job =
            version::level_change_job(&state, level.parse().map_err(js)?, &change, &self.base_caps)
                .map_err(js)?;
        let stats = Rc::clone(&self.stats);
        let job = oxilite_core::job::Then::new(job, move |s: Stats| {
            let state = s.version;
            *stats.borrow_mut() = s;
            Ok(version::status_job(state))
        });
        // A level change must not open a write tick of its own.
        Ok(wrap_raw(job, |s| {
            serde_json::to_value(s).map_err(Error::backend)
        }))
    }

    /// The SQL changing a store from level `from` to `to` (a D1 migration). `state` (JSON,
    /// optional) describes the current store: `stampColumn`, `history` (`none`, `frozen`).
    #[wasm_bindgen(js_name = levelChangeSql)]
    pub fn level_change_sql(
        &self,
        from: &str,
        to: &str,
        change: Option<String>,
        state: Option<String>,
    ) -> Result<String, JsError> {
        let mut st: version::VersionState = match state {
            Some(s) => serde_json::from_str(&s).map_err(js)?,
            None => Default::default(),
        };
        st.level = from.parse().map_err(js)?;
        if st.level >= version::Versioning::Stamped {
            st.stamp_column = true;
        }
        if st.level == version::Versioning::Log {
            st.history = version::History::Live;
        }
        let change: version::LevelChange = match change {
            Some(c) => serde_json::from_str(&c).map_err(js)?,
            None => Default::default(),
        };
        let mut out = String::new();
        for s in version::change_statements(&st, to.parse().map_err(js)?, &change).map_err(js)? {
            out.push_str(&s.sql);
            out.push_str(";\n");
        }
        Ok(out)
    }

    /// Author and message (JSON `{"author", "message"}`) recorded on the ticks of later writes.
    #[wasm_bindgen(js_name = setCommitInfo)]
    pub fn set_commit_info(&self, info: Option<String>) -> Result<(), JsError> {
        *self.info.borrow_mut() = match info {
            Some(i) => serde_json::from_str(&i).map_err(js)?,
            None => CommitInfo::default(),
        };
        Ok(())
    }

    /// The tick a version reference designates: `{"kind": "number", "value": tick}`.
    #[wasm_bindgen(js_name = resolveVersion)]
    pub fn resolve_version(&self, version: &str) -> Result<Job, JsError> {
        let job = version::resolve_job(
            vec![version.parse().map_err(js)?],
            self.stats.borrow().version,
        );
        Ok(wrap_raw(job, |t| {
            Ok(json!({"kind": "number", "value": t[0]}))
        }))
    }

    /// The latest `limit` commits and level changes, newest first.
    pub fn history(&self, limit: usize) -> Result<Job, JsError> {
        let job = version::log_job(self.stats.borrow().version, limit).map_err(js)?;
        Ok(self.wrap(job, |log| serde_json::to_value(log).map_err(Error::backend)))
    }

    /// The changes after tick `after` (up to `until`): `[{"tick", "added", "quad"}]`.
    pub fn changes(&self, after: f64, until: Option<f64>) -> Result<Job, JsError> {
        let job = version::changes_job(
            self.stats.borrow().version,
            after as i64,
            until.map(|u| u as i64),
            &self.caps(),
        )
        .map_err(js)?;
        Ok(self.wrap(job, |c| Ok(changes_to_json(&c))))
    }

    /// The net difference between two versions: `[{"tick", "added", "quad"}]`.
    pub fn diff(&self, from: &str, to: &str) -> Result<Job, JsError> {
        let state = self.stats.borrow().version;
        let refs = vec![from.parse().map_err(js)?, to.parse().map_err(js)?];
        let caps = self.caps();
        let job =
            oxilite_core::job::Then::new(version::resolve_job(refs, state), move |t: Vec<i64>| {
                version::diff_job(state, t[0], t[1], &caps)
            });
        Ok(self.wrap(job, |c| Ok(changes_to_json(&c))))
    }

    /// Removes the quads matching `pattern` (JSON `{"subject", "predicate", "object",
    /// "graph"}` of JSON terms, each optional) from the store and from its whole history,
    /// recording a purge that names no removed content.
    pub fn purge(&self, pattern: &str, reason: Option<String>) -> Result<Job, JsError> {
        let v: Value = serde_json::from_str(pattern).map_err(js)?;
        let term = |k: &str| -> Result<Option<i64>, JsError> {
            match v.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(t) if k == "graph" => Ok(Some(match json_to_graph(t).map_err(js)? {
                    GraphName::DefaultGraph => oxilite_core::encoding::DEFAULT_GRAPH_ID,
                    g => oxilite_core::encoding::graph_id(g.as_ref()),
                })),
                Some(t) => Ok(Some(oxilite_core::encoding::term_id(
                    json_to_term(t).map_err(js)?.as_ref(),
                ))),
            }
        };
        let pattern = [
            term("subject")?,
            term("predicate")?,
            term("object")?,
            term("graph")?,
        ];
        let author = self.info.borrow().author.clone();
        let request = version::purge_request(
            self.stats.borrow().version,
            pattern,
            author.as_deref(),
            reason.as_deref(),
        );
        Ok(wrap_raw(
            oxilite_core::job::OneShot::new(request, |_| Ok(())),
            |()| ok(),
        ))
    }

    pub fn clear(&self) -> Job {
        self.reloading(self.wrap(ops::clear_job(), |_| ok()), true)
    }
}

fn parse_quads(quads: &str) -> Result<Vec<Quad>, JsError> {
    let v: Value = serde_json::from_str(quads).map_err(js)?;
    let arr = v
        .as_array()
        .ok_or_else(|| js("expected a JSON array of quads"))?;
    arr.iter().map(|q| json_to_quad(q).map_err(js)).collect()
}

/// A graph name from its JSON term.
fn parse_graph(graph: &str) -> Result<GraphName, JsError> {
    json_to_graph(&serde_json::from_str::<Value>(graph).map_err(js)?).map_err(js)
}
