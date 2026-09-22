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
use oxilite_core::{
    ops, Capabilities, Error, QueryOptions, Request, Response, Stats, StoreOptions,
};
use oxrdf::{GraphName, NamedOrBlankNode, Quad, Term};
use oxrdfio::{RdfFormat, RdfParser, RdfSerializer};
use serde_json::{json, Value};
use spargebra::SparqlParser;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

fn js(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

type Stepper = Box<dyn FnMut(Option<Response>) -> oxilite_core::Result<Step<Value>>>;

/// A resumable operation (see the crate documentation).
#[wasm_bindgen]
pub struct Job {
    step: Stepper,
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
        Ok(match (self.step)(response).map_err(js)? {
            Step::Execute(r) => json!({ "execute": r }).to_string(),
            Step::Done(v) => json!({ "done": v }).to_string(),
        })
    }
}

fn wrap<J: CoreJob + 'static>(
    mut job: J,
    done: impl Fn(J::Output) -> oxilite_core::Result<Value> + 'static,
) -> Job {
    Job {
        step: Box::new(move |r| match job.step(r)? {
            Step::Execute(req) => Ok(Step::Execute(req)),
            Step::Done(o) => Ok(Step::Done(done(o)?)),
        }),
    }
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
    caps: Capabilities,
    options: StoreOptions,
    stats: Rc<RefCell<Stats>>,
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
            caps,
            options,
            stats: Rc::default(),
        })
    }

    /// The schema as a SQL script (for `wrangler d1 migrations`).
    #[wasm_bindgen(js_name = schemaSql)]
    pub fn schema_sql(&self) -> String {
        oxilite_core::schema::schema_sql(&self.options)
    }

    fn stats_job<J: CoreJob<Output = Stats> + 'static>(&self, job: J) -> Job {
        let stats = Rc::clone(&self.stats);
        wrap(job, move |s| {
            *stats.borrow_mut() = s;
            ok()
        })
    }

    /// Creates the schema if needed and loads planner statistics.
    pub fn open(&self) -> Job {
        self.stats_job(ops::open_job(&self.options, &self.caps))
    }

    /// Loads planner statistics only (schema applied by a migration).
    #[wasm_bindgen(js_name = openExisting)]
    pub fn open_existing(&self) -> Job {
        self.stats_job(ops::stats_job(&self.caps))
    }

    /// Recomputes planner statistics.
    pub fn optimize(&self) -> Job {
        self.stats_job(ops::optimize_job(&self.caps))
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
            &self.caps,
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
        let c = self.compiled(sparql, &options)?;
        let format = options.results_format.clone();
        Ok(wrap(
            QueryJob::new(c, self.caps.clone()),
            move |o| match &format {
                Some(f) => Ok(json!({"kind": "text", "value": output_to_format(&o, f)?})),
                None => Ok(output_to_json(&o)),
            },
        ))
    }

    /// A SPARQL query whose result is SPARQL 1.1 JSON results (`{"kind": "text", "value"}`).
    #[wasm_bindgen(js_name = queryJson)]
    pub fn query_json(&self, sparql: &str) -> Result<Job, JsError> {
        let c = self.compiled(sparql, &JsQueryOptions::default())?;
        Ok(wrap(QueryJob::new(c, self.caps.clone()), |o| {
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
                &self.caps,
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
            &self.caps,
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
        Ok(wrap(
            Sequence::new(if stmts.is_empty() {
                Vec::new()
            } else {
                vec![Request::atomic(stmts)]
            }),
            |_| ok(),
        ))
    }

    /// How an update would run (SQL per operation).
    #[wasm_bindgen(js_name = explainUpdate)]
    pub fn explain_update(&self, sparql: &str) -> Result<String, JsError> {
        let u = SparqlParser::new().parse_update(sparql).map_err(js)?;
        let plan = plan_update_with(
            &u,
            &self.stats.borrow(),
            &self.caps,
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
        let req = ops::insert_request(quads.iter().map(Quad::as_ref), &self.caps);
        if req.statements.len() > self.caps.max_statements {
            return Err(js(format!(
                "the document needs {} statements, more than one D1 batch allows ({}); use bulkLoad",
                req.statements.len(),
                self.caps.max_statements
            )));
        }
        Ok(wrap(Sequence::new(vec![req]), |_| ok()))
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
            let req = ops::insert_request(quads[start..end].iter().map(Quad::as_ref), &self.caps);
            if req.statements.len() > self.caps.max_statements && chunk > 1 {
                chunk /= 2;
                continue;
            }
            requests.push(req);
            start = end;
        }
        let mut load = Sequence::new(requests);
        let mut optimize: Option<Box<dyn CoreJob<Output = Stats>>> = None;
        let caps = self.caps.clone();
        let stats = Rc::clone(&self.stats);
        let mut loading = true;
        Ok(Job {
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
        Ok(wrap(
            ops::insert_job(quads.iter().map(Quad::as_ref), &self.caps),
            |n| Ok(json!({"kind": "number", "value": n})),
        ))
    }

    /// Removes quads (JSON array of RDF/JS quads) atomically.
    pub fn delete(&self, quads: &str) -> Result<Job, JsError> {
        let quads = parse_quads(quads)?;
        Ok(wrap(
            ops::remove_job(quads.iter().map(Quad::as_ref), &self.caps),
            |n| Ok(json!({"kind": "number", "value": n})),
        ))
    }

    /// Does the store contain a quad (JSON RDF/JS quad)?
    pub fn has(&self, quad: &str) -> Result<Job, JsError> {
        let q = json_to_quad(&serde_json::from_str(quad).map_err(js)?).map_err(js)?;
        Ok(wrap(ops::contains_job(q.as_ref()), |b| {
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
                return Ok(wrap(Sequence::new(Vec::new()), |_| {
                    Ok(json!({"kind": "quads", "quads": []}))
                }))
            }
        };
        let p = match term(predicate)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(n),
            Some(_) => {
                return Ok(wrap(Sequence::new(Vec::new()), |_| {
                    Ok(json!({"kind": "quads", "quads": []}))
                }))
            }
        };
        let o = term(object)?;
        let g: Option<GraphName> = graph
            .map(|g| json_to_graph(&serde_json::from_str(&g).map_err(js)?).map_err(js))
            .transpose()?;
        Ok(wrap(
            ops::scan_job(
                s.as_ref().map(Into::into),
                p.as_ref().map(Into::into),
                o.as_ref().map(Into::into),
                g.as_ref().map(Into::into),
                &self.caps,
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
        wrap(ops::len_job(), |n| {
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
        Ok(wrap(
            ops::scan_job(None, None, None, g.as_ref().map(Into::into), &self.caps),
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
    pub fn clear(&self) -> Job {
        wrap(ops::clear_job(), |_| ok())
    }
}

fn parse_quads(quads: &str) -> Result<Vec<Quad>, JsError> {
    let v: Value = serde_json::from_str(quads).map_err(js)?;
    let arr = v
        .as_array()
        .ok_or_else(|| js("expected a JSON array of quads"))?;
    arr.iter().map(|q| json_to_quad(q).map_err(js)).collect()
}
