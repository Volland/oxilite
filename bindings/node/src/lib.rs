//! Node.js bindings: `blocking::Store` (bundled SQLite or a dlopen'ed library) behind a small
//! JSON-in / JSON-out native class, wrapped by the TypeScript `Store` in `ts/index.ts`.
//!
//! Terms, quads and results use the same JSON forms as the wasm core
//! (`oxilite_core::json`), so `@oxilite/node` and `@oxilite/d1` share their conversions.
//!
// @lat: [[architecture#Bindings]]

use napi::{Error, Result, Status};
use napi_derive::napi;
use oxilite::blocking::Store;
use oxilite::dylib::DylibBackend;
use oxilite_core::json::{
    json_to_graph, json_to_quad, json_to_term, output_to_format, output_to_json, quad_to_json,
    JsQueryOptions,
};
use oxilite_core::StoreOptions;
use oxrdf::{GraphName, NamedOrBlankNode, Quad, Term};
use oxrdfio::{RdfFormat, RdfParser, RdfSerializer};
use serde_json::{json, Value};
use spargebra::SparqlParser;

fn err(e: impl std::fmt::Display) -> Error {
    Error::new(Status::GenericFailure, e.to_string())
}

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_str(s).map_err(err)
}

fn format(name: &str) -> Result<RdfFormat> {
    RdfFormat::from_media_type(name)
        .or_else(|| RdfFormat::from_extension(name))
        .ok_or_else(|| err(format!("unknown RDF format {name}")))
}

fn graph(json: Option<String>) -> Result<Option<GraphName>> {
    json.map(|g| json_to_graph(&parse::<Value>(&g)?).map_err(err))
        .transpose()
}

fn cypher_args(
    params: Option<String>,
    options: Option<String>,
) -> Result<(oxilite::cypher::Params, oxilite::cypher::CypherOptions)> {
    let params = match params {
        Some(p) => oxilite::cypher::json::params_from_json(&p).map_err(err)?,
        None => oxilite::cypher::Params::new(),
    };
    let opts = match options {
        Some(o) => oxilite::cypher::json::options_from_json(&o).map_err(err)?,
        None => oxilite::cypher::CypherOptions::default(),
    };
    Ok((params, opts))
}

/// The error of a JSON-LD operation, as `oxilite-jsonld:{"code", "message"}` for the wrapper.
fn jsonld_err(e: oxilite::jsonld::JsonLdError) -> Error {
    err(format!(
        "oxilite-jsonld:{}",
        oxilite::jsonld::json::error_to_json(&e)
    ))
}

fn jsonld_options(o: &Value) -> Result<oxilite::jsonld::JsonLdOptions> {
    jsonld_options_from(o, oxilite::jsonld::JsonLdOptions::default())
}

fn jsonld_options_from(
    o: &Value,
    base: oxilite::jsonld::JsonLdOptions,
) -> Result<oxilite::jsonld::JsonLdOptions> {
    let mut opts = oxilite::jsonld::json::options_from_json(o, base).map_err(jsonld_err)?;
    if o.get("network").and_then(Value::as_bool).unwrap_or(false) {
        opts.fetcher = Some(oxilite::jsonld::http_fetcher());
    }
    Ok(opts)
}

/// One operation of a document handle; `args` is the operation's JSON arguments.
fn document_op<B, S>(
    h: &oxilite::jsonld::JsonLdStore<'_, B, S>,
    op: &str,
    args: &Value,
) -> std::result::Result<Value, oxilite::jsonld::JsonLdError>
where
    B: oxilite_core::SyncBackend + Send + Sync + 'static,
    S: oxilite::jsonld::Loader,
{
    use oxilite::jsonld::json::*;
    use oxilite::jsonld::JsonLdError;
    let s = |k: &str| args.get(k).and_then(Value::as_str);
    let key = || s("key").ok_or_else(|| JsonLdError::Invalid("missing `key`".into()));
    Ok(match op {
        "put" => outcome_to_json(&h.put_documents(inputs_from_json(&args["documents"])?)?),
        "get" => h
            .get_document(key()?)?
            .as_ref()
            .map_or(Value::Null, document_to_json),
        "remove" => json!(h.remove_document(key()?)?),
        "list" => documents_to_json(&h.list_documents(
            s("after"),
            args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize,
        )?),
        "find" => documents_to_json(&h.find_documents(&filter_from_json(args)?)?),
        "graphs" => graphs_to_json(&h.document_graphs(key()?)?),
        "documentForGraph" => {
            let g = json_to_graph(&args["graph"]).map_err(JsonLdError::Store)?;
            h.document_for_graph(&g)?
                .as_ref()
                .map_or(Value::Null, document_to_json)
        }
        "putContext" => {
            let ctx = match &args["context"] {
                Value::String(t) => t.clone(),
                v => v.to_string(),
            };
            h.put_context(s("iri").unwrap_or_default(), &ctx)?;
            Value::Null
        }
        "removeContext" => {
            h.remove_context(s("iri").unwrap_or_default())?;
            Value::Null
        }
        "contexts" => json!(h.contexts()?),
        "rebuild" => json!(h.rebuild_graph(key()?)?),
        "check" => drifts_to_json(&h.check_documents()?),
        other => return Err(JsonLdError::Invalid(format!("unknown operation {other}"))),
    })
}

/// A JSON-LD or (with `"credentials": true` in the options) a Verifiable Credentials operation.
fn jsonld_op<B>(store: &Store<B>, op: &str, args: &Value, opts: &Value) -> Result<Value>
where
    B: oxilite_core::SyncBackend + Send + Sync + 'static,
{
    use oxilite::jsonld::json::documents_to_json;
    if opts
        .get("credentials")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut co = oxilite::vc::CredentialOptions::default();
        co.jsonld = jsonld_options_from(opts, co.jsonld)?;
        co.embed_presentation_credentials = opts
            .get("embedCredentials")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let h = store.credentials_with(co).map_err(jsonld_err)?;
        let json = || args.get("json").and_then(Value::as_str).unwrap_or_default();
        let r = match op {
            "putCredential" => match args.get("key").and_then(Value::as_str) {
                Some(k) => h.put_credential_with_key(k, json()).map(|k| json!(k)),
                None => h.put_credential(json()).map(|k| json!(k)),
            },
            "putPresentation" => h
                .put_presentation(json())
                .map(|k| json!({"key": k.key, "credentials": k.credentials})),
            "find" => oxilite::jsonld::json::filter_from_json(args)
                .and_then(|f| h.find_credentials(&f))
                .map(|d| documents_to_json(&d)),
            _ => document_op(h.documents(), op, args),
        };
        return r.map_err(jsonld_err);
    }
    let h = store
        .jsonld_with(jsonld_options(opts)?)
        .map_err(jsonld_err)?;
    document_op(&h, op, args).map_err(jsonld_err)
}

enum Backend {
    Native(Store),
    Library(Store<DylibBackend>),
}

/// Runs `$body` with `$s` bound to the store, whatever its backend.
macro_rules! with_store {
    ($self:expr, $s:ident => $body:expr) => {
        match &$self.inner {
            Backend::Native($s) => $body,
            Backend::Library($s) => $body,
        }
    };
}

/// A store; every argument and result that carries RDF is JSON (see `oxilite_core::json`).
#[napi]
pub struct NativeStore {
    inner: Backend,
}

#[napi]
impl NativeStore {
    /// `path`: a database file (in memory when absent); `library`: a SQLite shared library to
    /// load instead of the bundled SQLite; `options`: store options as JSON (`{"graphIndex"}`).
    #[napi(constructor)]
    pub fn new(
        path: Option<String>,
        library: Option<String>,
        options: Option<String>,
    ) -> Result<Self> {
        let options: StoreOptions = options
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or_default();
        let inner = match library {
            Some(lib) => Backend::Library(
                Store::with_backend_and_options(
                    DylibBackend::open(lib, path.as_deref().unwrap_or(":memory:")).map_err(err)?,
                    &options,
                )
                .map_err(err)?,
            ),
            None => Backend::Native(match path {
                Some(p) => Store::open_with_options(p, options).map_err(err)?,
                None => Store::with_backend_and_options(
                    oxilite::rusqlite::RusqliteBackend::memory().map_err(err)?,
                    &options,
                )
                .map_err(err)?,
            }),
        };
        Ok(Self { inner })
    }

    /// A SPARQL query; returns the output JSON, or `{"kind": "text"}` with `results_format`.
    #[napi]
    pub fn query(&self, sparql: String, options: Option<String>) -> Result<String> {
        let options: JsQueryOptions = options
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or_default();
        let mut parser = SparqlParser::new();
        if let Some(b) = &options.base_iri {
            parser = parser.with_base_iri(b).map_err(err)?;
        }
        let q = parser.parse_query(&sparql).map_err(err)?;
        let core = options.to_options().map_err(err)?;
        let out = with_store!(self, s => s.query_output(q, &core)).map_err(err)?;
        let v = match &options.results_format {
            Some(f) => json!({"kind": "text", "value": output_to_format(&out, f).map_err(err)?}),
            None => output_to_json(&out),
        };
        Ok(v.to_string())
    }

    /// A Cypher statement; `params` and `options` are JSON (`oxilite_cypher::json`). Returns
    /// `{"kind": "cypher", "columns", "rows", "stats"}` as JSON.
    #[napi]
    pub fn cypher(
        &self,
        query: String,
        params: Option<String>,
        options: Option<String>,
    ) -> Result<String> {
        let (params, opts) = cypher_args(params, options)?;
        let r = with_store!(self, s => s.cypher_with(&query, &params, &opts)).map_err(err)?;
        let mut v = r.to_json();
        v["kind"] = json!("cypher");
        Ok(v.to_string())
    }

    /// How a Cypher statement runs: its SPARQL, the SQL, and what runs in Rust.
    #[napi]
    pub fn explain_cypher(
        &self,
        query: String,
        params: Option<String>,
        options: Option<String>,
    ) -> Result<String> {
        let (params, opts) = cypher_args(params, options)?;
        with_store!(self, s => s.explain_cypher(&query, &params, &opts)).map_err(err)
    }

    /// The SQL a query compiles to, with the planner's notes.
    #[napi]
    pub fn explain(&self, sparql: String) -> Result<String> {
        with_store!(self, s => s.explain(sparql.as_str())).map_err(err)
    }

    /// A SPARQL update, applied atomically.
    #[napi]
    pub fn update(&self, sparql: String, base_iri: Option<String>) -> Result<()> {
        let mut parser = SparqlParser::new();
        if let Some(b) = base_iri {
            parser = parser.with_base_iri(b).map_err(err)?;
        }
        let u = parser.parse_update(&sparql).map_err(err)?;
        with_store!(self, s => s.update(u)).map_err(err)
    }

    /// How an update runs: the SQL of each operation, or why it needs the fallback.
    #[napi]
    pub fn explain_update(&self, sparql: String) -> Result<String> {
        with_store!(self, s => s.explain_update(sparql.as_str())).map_err(err)
    }

    /// Loads RDF: atomically, or in chunks (`bulk`) followed by a statistics refresh.
    #[napi]
    pub fn load(
        &self,
        data: String,
        format_name: String,
        base_iri: Option<String>,
        to_graph: Option<String>,
        bulk: bool,
    ) -> Result<()> {
        let mut parser = RdfParser::from_format(format(&format_name)?);
        if let Some(b) = base_iri {
            parser = parser.with_base_iri(b).map_err(err)?;
        }
        if let Some(g) = graph(to_graph)? {
            parser = parser.with_default_graph(g);
        }
        with_store!(self, s => if bulk {
            let mut loader = s.bulk_loader();
            match loader.load_from_slice(parser, data.as_bytes()) {
                Ok(()) => loader.commit(),
                Err(e) => Err(e),
            }
        } else {
            s.load_from_slice(parser, data.as_bytes())
        })
        .map_err(err)
    }

    /// Inserts quads (a JSON array) atomically.
    #[napi]
    pub fn add(&self, quads: String) -> Result<()> {
        let quads = parse::<Vec<Value>>(&quads)?
            .iter()
            .map(json_to_quad)
            .collect::<oxilite_core::Result<Vec<Quad>>>()
            .map_err(err)?;
        with_store!(self, s => s.extend(quads)).map_err(err)
    }

    /// Removes quads (a JSON array).
    #[napi]
    pub fn delete(&self, quads: String) -> Result<()> {
        for q in parse::<Vec<Value>>(&quads)? {
            let q = json_to_quad(&q).map_err(err)?;
            with_store!(self, s => s.remove(&q)).map_err(err)?;
        }
        Ok(())
    }

    #[napi]
    pub fn has(&self, quad: String) -> Result<bool> {
        let q = json_to_quad(&parse(&quad)?).map_err(err)?;
        with_store!(self, s => s.contains(&q)).map_err(err)
    }

    /// Quads matching a pattern (JSON terms or null); returns `{"kind": "quads"}`.
    #[napi(js_name = "match")]
    pub fn match_(
        &self,
        subject: Option<String>,
        predicate: Option<String>,
        object: Option<String>,
        graph_name: Option<String>,
    ) -> Result<String> {
        let term = |t: Option<String>| -> Result<Option<Term>> {
            t.map(|t| json_to_term(&parse(&t)?).map_err(err))
                .transpose()
        };
        let s = match term(subject)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(NamedOrBlankNode::from(n)),
            Some(Term::BlankNode(b)) => Some(NamedOrBlankNode::from(b)),
            Some(_) => return Ok(json!({"kind": "quads", "quads": []}).to_string()),
        };
        let p = match term(predicate)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(n),
            Some(_) => return Ok(json!({"kind": "quads", "quads": []}).to_string()),
        };
        let o = term(object)?;
        let g = graph(graph_name)?;
        let quads = with_store!(self, st => st
            .quads_for_pattern(s.as_ref().map(Into::into), p.as_ref().map(Into::into), o.as_ref().map(Into::into), g.as_ref().map(Into::into))
            .collect::<oxilite_core::Result<Vec<Quad>>>())
        .map_err(err)?;
        Ok(
            json!({"kind": "quads", "quads": quads.iter().map(quad_to_json).collect::<Vec<_>>()})
                .to_string(),
        )
    }

    #[napi]
    pub fn size(&self) -> Result<f64> {
        Ok(with_store!(self, s => s.len()).map_err(err)? as f64)
    }

    /// Serializes the dataset, or one graph when `from_graph` (JSON) is given.
    #[napi]
    pub fn dump(&self, format_name: String, from_graph: Option<String>) -> Result<String> {
        let serializer = RdfSerializer::from_format(format(&format_name)?);
        let bytes = match graph(from_graph)? {
            Some(g) => with_store!(self, s => s.dump_graph_to_writer(&g, serializer, Vec::new())),
            None => with_store!(self, s => s.dump_to_writer(serializer, Vec::new())),
        }
        .map_err(err)?;
        String::from_utf8(bytes).map_err(err)
    }

    /// Computes the OWL 2 RL closure into the inference table, with SQL rules or (`reasonable`)
    /// in memory; returns the number of inferred triples.
    #[napi]
    pub fn materialize(&self, reasonable: Option<bool>) -> Result<f64> {
        let n = if reasonable.unwrap_or(false) {
            with_store!(self, s => s.materialize_with_reasonable())
        } else {
            with_store!(self, s => s.materialize())
        };
        Ok(n.map_err(err)? as f64)
    }

    /// Removes every materialized inference.
    #[napi]
    pub fn clear_inferences(&self) -> Result<()> {
        with_store!(self, s => s.clear_inferences()).map_err(err)
    }

    /// Refreshes planner statistics (run after large imports).
    #[napi]
    pub fn optimize(&self) -> Result<()> {
        with_store!(self, s => s.optimize()).map_err(err)
    }

    #[napi]
    pub fn clear(&self) -> Result<()> {
        with_store!(self, s => s.clear()).map_err(err)
    }

    /// A JSON-LD document or credential operation (`put`, `get`, `remove`, `list`, `find`,
    /// `graphs`, `documentForGraph`, `putContext`, `removeContext`, `contexts`, `rebuild`,
    /// `check`, `putCredential`, `putPresentation`); arguments, options and result are JSON.
    #[napi]
    pub fn jsonld(&self, op: String, args: String, options: Option<String>) -> Result<String> {
        let args: Value = parse(&args)?;
        let opts: Value = options
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or(Value::Null);
        let v = with_store!(self, s => jsonld_op(s, &op, &args, &opts))?;
        Ok(v.to_string())
    }

    /// Writes a consistent copy of the database to `path` (`VACUUM INTO`).
    #[napi]
    pub fn backup(&self, path: String) -> Result<()> {
        with_store!(self, s => s.backup(path)).map_err(err)
    }
}

/// The schema as a SQL script.
#[napi]
pub fn schema_sql(options: Option<String>) -> Result<String> {
    let options: StoreOptions = options
        .as_deref()
        .map(parse)
        .transpose()?
        .unwrap_or_default();
    Ok(oxilite_core::schema::schema_sql(&options))
}
