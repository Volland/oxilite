//! Python bindings: `blocking::Store` (bundled SQLite or a dlopen'ed library) behind a small
//! JSON-in / JSON-out native class, wrapped by the pure-Python package in `python/oxilite`.
//!
//! Terms, quads, options and results use the JSON forms shared with `@oxilite/node` and the
//! wasm engine (`oxilite_core::json` and the `json` modules of the Cypher, Datalog and JSON-LD
//! crates), so every binding returns the same values. Store work runs with the GIL released.
//!
// @lat: [[architecture#Bindings#Python]]

use oxilite::blocking::Store;
use oxilite::dylib::DylibBackend;
use oxilite_core::json::{
    json_to_graph, json_to_quad, json_to_term, output_to_format, output_to_json, quad_to_json,
    term_to_json, JsQueryOptions,
};
use oxilite_core::query::QueryOutput;
use oxilite_core::StoreOptions;
use oxrdf::{GraphName, NamedOrBlankNode, Quad, Term, Variable};
use oxrdfio::{RdfFormat, RdfParseError, RdfParser, RdfSerializer};
use pyo3::create_exception;
use pyo3::exceptions::{
    PyNotImplementedError, PyOSError, PyRuntimeError, PySyntaxError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use serde_json::{json, Value};
use sparesults::{QueryResultsFormat, QueryResultsParser, SliceQueryResultsParserOutput};
use spargebra::algebra::GraphPattern;
use spargebra::term::{GroundTerm, GroundTriple};
use spargebra::{Query, SparqlParser};
use std::ops::Range;
use std::path::{Path, PathBuf};

create_exception!(
    _native,
    JsonLdError,
    PyValueError,
    "A JSON-LD or Verifiable Credentials error; `code` is the JSON-LD error code."
);

// ---------------------------------------------------------------------------------- errors

/// A 0-based text range as the `(lineno, offset, end_lineno, end_offset)` Python expects.
struct Location {
    line: u64,
    column: u64,
    end_line: u64,
    end_column: u64,
}

fn syntax_error(message: String, filename: Option<&str>, location: Option<Location>) -> PyErr {
    match location {
        Some(location) => PySyntaxError::new_err(SyntaxErrorArgs {
            message,
            filename: filename.map(str::to_owned),
            location,
        }),
        None => PySyntaxError::new_err(message),
    }
}

/// `SyntaxError(message, details)`: Python 3.9 takes a 4-tuple of details and raises
/// `IndexError` on the 6-tuple (with the end position) that 3.10 and later accept.
struct SyntaxErrorArgs {
    message: String,
    filename: Option<String>,
    location: Location,
}

impl<'py> IntoPyObject<'py> for SyntaxErrorArgs {
    type Target = pyo3::types::PyTuple;
    type Output = Bound<'py, Self::Target>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        let (l, text) = (self.location, Option::<String>::None);
        let details = if py.version_info() >= (3, 10) {
            let end = (l.end_line + 1, l.end_column + 1);
            (self.filename, l.line + 1, l.column + 1, text, end.0, end.1).into_pyobject(py)?
        } else {
            (self.filename, l.line + 1, l.column + 1, text).into_pyobject(py)?
        };
        (self.message, details).into_pyobject(py)
    }
}

fn rdf_location(r: Range<oxrdfio::TextPosition>) -> Location {
    Location {
        line: r.start.line,
        column: r.start.column,
        end_line: r.end.line,
        end_column: r.end.column,
    }
}

fn rdf_parse_error(e: RdfParseError, filename: Option<&str>) -> PyErr {
    match e {
        RdfParseError::Io(e) => PyOSError::new_err(e.to_string()),
        RdfParseError::Syntax(e) => {
            syntax_error(e.to_string(), filename, e.location().map(rdf_location))
        }
    }
}

/// The Python exception of a store error (see the design's exception table).
fn store_error(e: oxilite::Error) -> PyErr {
    use oxilite::Error as E;
    match e {
        E::Syntax(e) => PySyntaxError::new_err(e.to_string()),
        E::Parse(e) => rdf_parse_error(e, None),
        E::Backend(_) | E::Corrupted(_) | E::Io(_) => PyOSError::new_err(e.to_string()),
        E::Unsupported(_) => PyNotImplementedError::new_err(e.to_string()),
        E::Other(_) => PyValueError::new_err(e.to_string()),
        // Collisions, evaluation errors, and kinds added later.
        #[allow(unreachable_patterns)]
        _ => PyRuntimeError::new_err(e.to_string()),
    }
}

fn cypher_error(e: oxilite::cypher::CypherError) -> PyErr {
    use oxilite::cypher::CypherError as E;
    match e {
        E::Store(e) => store_error(e),
        E::Syntax { .. } => PySyntaxError::new_err(e.to_string()),
        E::Unsupported(_) => PyNotImplementedError::new_err(e.to_string()),
        E::Runtime(_) => PyRuntimeError::new_err(e.to_string()),
        _ => PyValueError::new_err(e.to_string()),
    }
}

fn datalog_error(e: oxilite::datalog::DatalogError) -> PyErr {
    use oxilite::datalog::DatalogError as E;
    match e {
        E::Store(e) => store_error(e),
        E::Parse { .. } => PySyntaxError::new_err(e.to_string()),
        E::Unsupported(_) => PyNotImplementedError::new_err(e.to_string()),
        E::IterationLimit { .. } => PyRuntimeError::new_err(e.to_string()),
        _ => PyValueError::new_err(e.to_string()),
    }
}

fn synalog_error(e: oxilite::synalog::SynalogError) -> PyErr {
    use oxilite::synalog::SynalogError as E;
    match e {
        E::Store(e) => store_error(e),
        E::Parse(_) | E::Pragma { .. } => PySyntaxError::new_err(e.to_string()),
        E::Unsupported(_) => PyNotImplementedError::new_err(e.to_string()),
        _ => PyValueError::new_err(e.to_string()),
    }
}

fn jsonld_error(e: oxilite::jsonld::JsonLdError) -> PyErr {
    if let oxilite::jsonld::JsonLdError::Store(e) = e {
        return store_error(e);
    }
    let v = oxilite::jsonld::json::error_to_json(&e);
    let code = v["code"].as_str().unwrap_or("invalid").to_owned();
    let message = v["message"].as_str().unwrap_or_default().to_owned();
    Python::attach(|py| {
        let err = JsonLdError::new_err(message);
        // The attribute is informative; failing to set it must not hide the error itself.
        let _ = err.value(py).setattr("code", code);
        err
    })
}

/// An argument that is not what the Python wrapper promised.
fn invalid(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(e.to_string())
}

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> PyResult<T> {
    serde_json::from_str(s).map_err(invalid)
}

fn parse_opt<T: serde::de::DeserializeOwned + Default>(s: Option<&str>) -> PyResult<T> {
    s.map(parse).transpose().map(Option::unwrap_or_default)
}

// ------------------------------------------------------------------------- JSON arguments

fn rdf_format(name: &str) -> PyResult<RdfFormat> {
    RdfFormat::from_media_type(name)
        .or_else(|| RdfFormat::from_extension(name))
        .ok_or_else(|| invalid(format!("unknown RDF format {name}")))
}

fn results_format(name: &str) -> PyResult<QueryResultsFormat> {
    QueryResultsFormat::from_media_type(name)
        .or_else(|| QueryResultsFormat::from_extension(name))
        .ok_or_else(|| invalid(format!("unknown query results format {name}")))
}

fn graph(json: Option<&str>) -> PyResult<Option<GraphName>> {
    json.map(|g| json_to_graph(&parse::<Value>(g)?).map_err(store_error))
        .transpose()
}

fn required_graph(json: &str) -> PyResult<GraphName> {
    json_to_graph(&parse::<Value>(json)?).map_err(store_error)
}

fn named_graph(json: &str) -> PyResult<NamedOrBlankNode> {
    match required_graph(json)? {
        GraphName::NamedNode(n) => Ok(n.into()),
        GraphName::BlankNode(b) => Ok(b.into()),
        GraphName::DefaultGraph => Err(invalid("the default graph is not a named graph")),
    }
}

fn quads(json: &str) -> PyResult<Vec<Quad>> {
    parse::<Vec<Value>>(json)?
        .iter()
        .map(|q| json_to_quad(q).map_err(store_error))
        .collect()
}

fn quads_json(quads: &[Quad]) -> String {
    Value::Array(quads.iter().map(quad_to_json).collect()).to_string()
}

/// A SPARQL parser with the base IRI and `{"prefix": "iri"}` of the options.
fn sparql_parser(base_iri: Option<&str>, prefixes: Option<&Value>) -> PyResult<SparqlParser> {
    let mut parser = SparqlParser::new();
    if let Some(b) = base_iri {
        parser = parser.with_base_iri(b).map_err(invalid)?;
    }
    if let Some(Value::Object(p)) = prefixes {
        for (name, iri) in p {
            let iri = iri
                .as_str()
                .ok_or_else(|| invalid("prefix IRIs must be strings"))?;
            parser = parser.with_prefix(name, iri).map_err(invalid)?;
        }
    }
    Ok(parser)
}

fn ground_term(t: Term) -> PyResult<GroundTerm> {
    Ok(match t {
        Term::NamedNode(n) => GroundTerm::NamedNode(n),
        Term::Literal(l) => GroundTerm::Literal(l),
        Term::Triple(t) => {
            let NamedOrBlankNode::NamedNode(subject) = t.subject else {
                return Err(invalid("a substituted triple cannot contain blank nodes"));
            };
            GroundTerm::Triple(Box::new(GroundTriple {
                subject,
                predicate: t.predicate,
                object: ground_term(t.object)?,
            }))
        }
        Term::BlankNode(_) => return Err(invalid("a blank node cannot be substituted")),
    })
}

/// Binds variables before evaluation: a one-row `VALUES` joined with the pattern below the
/// solution modifiers, the projection, grouping, and the `BIND`s and `FILTER`s wrapping it.
fn substitute(pattern: &mut GraphPattern, values: &GraphPattern) {
    match pattern {
        GraphPattern::Slice { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Group { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::Filter { inner, .. } => substitute(inner, values),
        other => {
            let inner = std::mem::take(other);
            *other = GraphPattern::Join {
                left: Box::new(values.clone()),
                right: Box::new(inner),
            };
        }
    }
}

fn apply_substitutions(query: &mut Query, substitutions: &[Value]) -> PyResult<()> {
    if substitutions.is_empty() {
        return Ok(());
    }
    let mut variables = Vec::new();
    let mut row = Vec::new();
    for pair in substitutions {
        let (Some(name), Some(term)) = (pair.get(0).and_then(Value::as_str), pair.get(1)) else {
            return Err(invalid("substitutions are [variable, term] pairs"));
        };
        variables.push(Variable::new(name).map_err(invalid)?);
        row.push(Some(ground_term(json_to_term(term).map_err(store_error)?)?));
    }
    let values = GraphPattern::Values {
        variables,
        bindings: vec![row],
    };
    match query {
        Query::Select { pattern, .. }
        | Query::Construct { pattern, .. }
        | Query::Describe { pattern, .. }
        | Query::Ask { pattern, .. } => substitute(pattern, &values),
    }
    Ok(())
}

/// A query output from its JSON form (the inverse of `output_to_json`).
fn json_to_output(v: &Value) -> PyResult<QueryOutput> {
    let term = |t: &Value| json_to_term(t).map_err(store_error);
    Ok(match v["kind"].as_str() {
        Some("boolean") => QueryOutput::Boolean(v["value"].as_bool().unwrap_or(false)),
        Some("solutions") => QueryOutput::Solutions {
            variables: v["variables"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|n| Variable::new(n.as_str().unwrap_or_default()).map_err(invalid))
                .collect::<PyResult<_>>()?,
            rows: v["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| {
                    row.as_array()
                        .into_iter()
                        .flatten()
                        .map(|t| {
                            if t.is_null() {
                                Ok(None)
                            } else {
                                term(t).map(Some)
                            }
                        })
                        .collect::<PyResult<Vec<_>>>()
                })
                .collect::<PyResult<_>>()?,
        },
        Some("quads") => QueryOutput::Graph(
            v["quads"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|q| json_to_quad(q).map(|q| q.into()).map_err(store_error))
                .collect::<PyResult<_>>()?,
        ),
        other => return Err(invalid(format!("unexpected output kind {other:?}"))),
    })
}

fn read_input(data: Option<Vec<u8>>, path: Option<&Path>) -> PyResult<Vec<u8>> {
    match (data, path) {
        (Some(d), _) => Ok(d),
        (None, Some(p)) => std::fs::read(p).map_err(PyErr::from),
        (None, None) => Err(invalid("either an input or a path is required")),
    }
}

// ------------------------------------------------------------------------ JSON-LD dispatch

fn jsonld_options(
    o: &Value,
) -> Result<oxilite::jsonld::JsonLdOptions, oxilite::jsonld::JsonLdError> {
    jsonld_options_from(o, oxilite::jsonld::JsonLdOptions::default())
}

fn jsonld_options_from(
    o: &Value,
    base: oxilite::jsonld::JsonLdOptions,
) -> Result<oxilite::jsonld::JsonLdOptions, oxilite::jsonld::JsonLdError> {
    let mut opts = oxilite::jsonld::json::options_from_json(o, base)?;
    if o.get("network").and_then(Value::as_bool).unwrap_or(false) {
        opts.fetcher = Some(oxilite::jsonld::http_fetcher());
    }
    Ok(opts)
}

/// One operation of a document handle; `args` is the operation's JSON arguments. The same
/// operations as `@oxilite/node`, with the same JSON.
fn document_op<B, S>(
    h: &oxilite::jsonld::JsonLdStore<'_, B, S>,
    op: &str,
    args: &Value,
) -> Result<Value, oxilite::jsonld::JsonLdError>
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
fn jsonld_op<B>(
    store: &Store<B>,
    op: &str,
    args: &Value,
    opts: &Value,
) -> Result<Value, oxilite::jsonld::JsonLdError>
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
        let h = store.credentials_with(co)?;
        let json = || args.get("json").and_then(Value::as_str).unwrap_or_default();
        return match op {
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
    }
    let h = store.jsonld_with(jsonld_options(opts)?)?;
    document_op(&h, op, args)
}

fn changes_json(changes: &[oxilite::version::Change]) -> String {
    Value::Array(
        changes
            .iter()
            .map(|c| json!({"tick": c.tick, "added": c.added, "quad": quad_to_json(&c.quad)}))
            .collect(),
    )
    .to_string()
}

// --------------------------------------------------------------------------------- store

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
#[pyclass(frozen, module = "oxilite._native")]
struct NativeStore {
    inner: Backend,
}

/// The database file of `path`: `oxilite.sqlite` inside it when it is a directory (the form of
/// pyoxigraph's paths).
fn database_path(path: PathBuf) -> PathBuf {
    if path.is_dir() {
        path.join("oxilite.sqlite")
    } else {
        path
    }
}

#[pymethods]
impl NativeStore {
    /// `path`: a database file or directory (in memory when absent); `library`: a SQLite shared
    /// library to load instead of the bundled SQLite; `options`: store options as JSON.
    #[new]
    #[pyo3(signature = (path=None, library=None, options=None, read_only=false))]
    fn new(
        py: Python<'_>,
        path: Option<PathBuf>,
        library: Option<String>,
        options: Option<String>,
        read_only: bool,
    ) -> PyResult<Self> {
        let options: StoreOptions = parse_opt(options.as_deref())?;
        let path = path.map(database_path);
        let inner = py
            .detach(|| -> oxilite::Result<Backend> {
                Ok(match (library, path) {
                    (Some(lib), path) => {
                        let db = path
                            .as_deref()
                            .map_or(":memory:".into(), |p| p.to_string_lossy().into_owned());
                        Backend::Library(Store::with_backend_and_options(
                            DylibBackend::open(lib, &db)?,
                            &options,
                        )?)
                    }
                    (None, Some(p)) if read_only => Backend::Native(Store::open_read_only(p)?),
                    (None, Some(p)) => Backend::Native(Store::open_with_options(p, options)?),
                    (None, None) => Backend::Native(Store::with_backend_and_options(
                        oxilite::rusqlite::RusqliteBackend::memory()?,
                        &options,
                    )?),
                })
            })
            .map_err(store_error)?;
        Ok(Self { inner })
    }

    /// A SPARQL query; `options` is JSON with Oxigraph's JavaScript names plus `prefixes`
    /// (`{"name": "iri"}`) and `substitutions` (`[["var", term], …]`). Returns the output JSON.
    #[pyo3(signature = (sparql, options=None))]
    fn query(&self, py: Python<'_>, sparql: &str, options: Option<&str>) -> PyResult<String> {
        let raw: Value = options.map(parse).transpose()?.unwrap_or(Value::Null);
        let options: JsQueryOptions = if raw.is_null() {
            JsQueryOptions::default()
        } else {
            serde_json::from_value(raw.clone()).map_err(invalid)?
        };
        let mut q = sparql_parser(options.base_iri.as_deref(), raw.get("prefixes"))?
            .parse_query(sparql)
            .map_err(|e| PySyntaxError::new_err(e.to_string()))?;
        if let Some(Value::Array(subs)) = raw.get("substitutions") {
            apply_substitutions(&mut q, subs)?;
        }
        let core = options.to_options().map_err(store_error)?;
        let out = py
            .detach(|| with_store!(self, s => s.query_output(q, &core)))
            .map_err(store_error)?;
        Ok(output_to_json(&out).to_string())
    }

    /// The SQL a query compiles to, with the planner's notes.
    fn explain(&self, py: Python<'_>, sparql: &str) -> PyResult<String> {
        py.detach(|| with_store!(self, s => s.explain(sparql)))
            .map_err(store_error)
    }

    /// A SPARQL update, applied atomically; `prefixes` is JSON (`{"name": "iri"}`).
    #[pyo3(signature = (sparql, base_iri=None, prefixes=None))]
    fn update(
        &self,
        py: Python<'_>,
        sparql: &str,
        base_iri: Option<&str>,
        prefixes: Option<&str>,
    ) -> PyResult<()> {
        let prefixes: Option<Value> = prefixes.map(parse).transpose()?;
        let u = sparql_parser(base_iri, prefixes.as_ref())?
            .parse_update(sparql)
            .map_err(|e| PySyntaxError::new_err(e.to_string()))?;
        py.detach(|| with_store!(self, s => s.update(u)))
            .map_err(store_error)
    }

    /// How an update runs: the SQL of each operation, or why it needs the fallback.
    fn explain_update(&self, py: Python<'_>, sparql: &str) -> PyResult<String> {
        py.detach(|| with_store!(self, s => s.explain_update(sparql)))
            .map_err(store_error)
    }

    /// Loads RDF from `data` or the file at `path`: atomically, or in chunks (`bulk`) followed
    /// by a statistics refresh.
    #[pyo3(signature = (data, path, format, base_iri=None, to_graph=None, bulk=false))]
    #[allow(clippy::too_many_arguments)]
    fn load(
        &self,
        py: Python<'_>,
        data: Option<Vec<u8>>,
        path: Option<PathBuf>,
        format: &str,
        base_iri: Option<&str>,
        to_graph: Option<&str>,
        bulk: bool,
    ) -> PyResult<()> {
        let mut parser = RdfParser::from_format(rdf_format(format)?);
        if let Some(b) = base_iri {
            parser = parser.with_base_iri(b).map_err(invalid)?;
        }
        if let Some(g) = graph(to_graph)? {
            parser = parser.with_default_graph(g);
        }
        let filename = path.as_ref().map(|p| p.to_string_lossy().into_owned());
        py.detach(|| -> PyResult<()> {
            let data = read_input(data, path.as_deref())?;
            with_store!(self, s => if bulk {
                let mut loader = s.bulk_loader();
                match loader.load_from_slice(parser, &data) {
                    Ok(()) => loader.commit(),
                    Err(e) => Err(e),
                }
            } else {
                s.load_from_slice(parser, &data)
            })
            .map_err(|e| match e {
                oxilite::Error::Parse(e) => rdf_parse_error(e, filename.as_deref()),
                e => store_error(e),
            })
        })
    }

    /// Inserts quads (a JSON array) atomically, or in chunks with `bulk`.
    #[pyo3(signature = (quads_json, bulk=false))]
    fn add(&self, py: Python<'_>, quads_json: &str, bulk: bool) -> PyResult<()> {
        let quads = quads(quads_json)?;
        py.detach(|| {
            with_store!(self, s => if bulk {
                let mut loader = s.bulk_loader();
                match loader.load_quads(quads) {
                    Ok(()) => loader.commit(),
                    Err(e) => Err(e),
                }
            } else {
                s.extend(quads)
            })
        })
        .map_err(store_error)
    }

    /// Removes quads (a JSON array).
    fn remove(&self, py: Python<'_>, quads_json: &str) -> PyResult<()> {
        let quads = quads(quads_json)?;
        py.detach(|| -> oxilite::Result<()> {
            for q in &quads {
                with_store!(self, s => s.remove(q))?;
            }
            Ok(())
        })
        .map_err(store_error)
    }

    fn contains(&self, py: Python<'_>, quad_json: &str) -> PyResult<bool> {
        let q = json_to_quad(&parse(quad_json)?).map_err(store_error)?;
        py.detach(|| with_store!(self, s => s.contains(&q)))
            .map_err(store_error)
    }

    /// Quads matching a pattern (JSON terms or `None`; a `None` graph matches every graph);
    /// returns a JSON array of quads.
    #[pyo3(signature = (subject=None, predicate=None, object=None, graph_name=None))]
    fn quads_for_pattern(
        &self,
        py: Python<'_>,
        subject: Option<&str>,
        predicate: Option<&str>,
        object: Option<&str>,
        graph_name: Option<&str>,
    ) -> PyResult<String> {
        let term = |t: Option<&str>| -> PyResult<Option<Term>> {
            t.map(|t| json_to_term(&parse(t)?).map_err(store_error))
                .transpose()
        };
        let s = match term(subject)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(NamedOrBlankNode::from(n)),
            Some(Term::BlankNode(b)) => Some(NamedOrBlankNode::from(b)),
            Some(_) => return Ok("[]".into()),
        };
        let p = match term(predicate)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(n),
            Some(_) => return Ok("[]".into()),
        };
        let o = term(object)?;
        let g = graph(graph_name)?;
        let quads = py
            .detach(|| {
                with_store!(self, st => st
                    .quads_for_pattern(
                        s.as_ref().map(Into::into),
                        p.as_ref().map(Into::into),
                        o.as_ref().map(Into::into),
                        g.as_ref().map(Into::into),
                    )
                    .collect::<oxilite::Result<Vec<Quad>>>())
            })
            .map_err(store_error)?;
        Ok(quads_json(&quads))
    }

    fn len(&self, py: Python<'_>) -> PyResult<usize> {
        py.detach(|| with_store!(self, s => s.len()))
            .map_err(store_error)
    }

    /// Serializes the dataset, or one graph when `from_graph` (JSON) is given.
    #[pyo3(signature = (format, from_graph=None))]
    fn dump<'py>(
        &self,
        py: Python<'py>,
        format: &str,
        from_graph: Option<&str>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let serializer = RdfSerializer::from_format(rdf_format(format)?);
        let from_graph = graph(from_graph)?;
        let bytes = py
            .detach(|| match &from_graph {
                Some(g) => {
                    with_store!(self, s => s.dump_graph_to_writer(g, serializer, Vec::new()))
                }
                None => with_store!(self, s => s.dump_to_writer(serializer, Vec::new())),
            })
            .map_err(store_error)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// The named graphs, as a JSON array of terms.
    fn named_graphs(&self, py: Python<'_>) -> PyResult<String> {
        let graphs = py
            .detach(
                || with_store!(self, s => s.named_graphs().collect::<oxilite::Result<Vec<_>>>()),
            )
            .map_err(store_error)?;
        Ok(Value::Array(
            graphs
                .into_iter()
                .map(|g| term_to_json(&Term::from(g)))
                .collect(),
        )
        .to_string())
    }

    fn contains_named_graph(&self, py: Python<'_>, graph_json: &str) -> PyResult<bool> {
        let g = named_graph(graph_json)?;
        py.detach(|| with_store!(self, s => s.contains_named_graph(&g)))
            .map_err(store_error)
    }

    fn add_graph(&self, py: Python<'_>, graph_json: &str) -> PyResult<()> {
        let g = named_graph(graph_json)?;
        py.detach(|| with_store!(self, s => s.insert_named_graph(&g)))
            .map(|_| ())
            .map_err(store_error)
    }

    fn clear_graph(&self, py: Python<'_>, graph_json: &str) -> PyResult<()> {
        let g = required_graph(graph_json)?;
        py.detach(|| with_store!(self, s => s.clear_graph(&g)))
            .map_err(store_error)
    }

    fn remove_graph(&self, py: Python<'_>, graph_json: &str) -> PyResult<()> {
        match required_graph(graph_json)? {
            GraphName::DefaultGraph => py
                .detach(|| with_store!(self, s => s.clear_graph(GraphName::DefaultGraph.as_ref()))),
            g => {
                let g = named_graph(
                    &term_to_json(&match g {
                        GraphName::NamedNode(n) => Term::from(n),
                        GraphName::BlankNode(b) => Term::from(b),
                        GraphName::DefaultGraph => unreachable!(),
                    })
                    .to_string(),
                )?;
                py.detach(|| with_store!(self, s => s.remove_named_graph(&g)).map(|_| ()))
            }
        }
        .map_err(store_error)
    }

    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| with_store!(self, s => s.clear()))
            .map_err(store_error)
    }

    fn flush(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| with_store!(self, s => s.flush()))
            .map_err(store_error)
    }

    /// Refreshes planner statistics (run after large imports).
    fn optimize(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| with_store!(self, s => s.optimize()))
            .map_err(store_error)
    }

    /// Writes a consistent copy of the database to `path` (`VACUUM INTO`).
    fn backup(&self, py: Python<'_>, path: PathBuf) -> PyResult<()> {
        py.detach(|| with_store!(self, s => s.backup(&path)))
            .map_err(store_error)
    }

    /// A Cypher statement; `params` and `options` are JSON (`oxilite_cypher::json`). Returns
    /// `{"columns", "rows", "stats"}` as JSON.
    #[pyo3(signature = (query, params=None, options=None))]
    fn cypher(
        &self,
        py: Python<'_>,
        query: &str,
        params: Option<&str>,
        options: Option<&str>,
    ) -> PyResult<String> {
        let (params, opts) = cypher_args(params, options)?;
        let r = py
            .detach(|| with_store!(self, s => s.cypher_with(query, &params, &opts)))
            .map_err(cypher_error)?;
        Ok(r.to_json().to_string())
    }

    /// How a Cypher statement runs: its SPARQL, the SQL, and what runs in Rust.
    #[pyo3(signature = (query, params=None, options=None))]
    fn explain_cypher(
        &self,
        py: Python<'_>,
        query: &str,
        params: Option<&str>,
        options: Option<&str>,
    ) -> PyResult<String> {
        let (params, opts) = cypher_args(params, options)?;
        py.detach(|| with_store!(self, s => s.explain_cypher(query, &params, &opts)))
            .map_err(cypher_error)
    }

    /// A Datalog program; `options` is JSON (`oxilite_datalog::json`). Returns
    /// `{"kind": "datalog", "columns", "rows", "rounds"}`.
    #[pyo3(signature = (program, options=None))]
    fn datalog(&self, py: Python<'_>, program: &str, options: Option<&str>) -> PyResult<String> {
        let opts = datalog_args(options)?;
        let r = py
            .detach(|| with_store!(self, s => s.datalog_with(program, &opts)))
            .map_err(datalog_error)?;
        Ok(oxilite::datalog::json::result_to_json(&r).to_string())
    }

    /// Stores what a Datalog program derives as inferences, beside the OWL ones.
    #[pyo3(signature = (program, options=None))]
    fn datalog_materialize(
        &self,
        py: Python<'_>,
        program: &str,
        options: Option<&str>,
    ) -> PyResult<String> {
        let opts = datalog_args(options)?;
        let s = py
            .detach(|| with_store!(self, s => s.datalog_materialize_with(program, &opts)))
            .map_err(datalog_error)?;
        Ok(oxilite::datalog::json::stats_to_json(&s).to_string())
    }

    /// How a Datalog program runs: its strata, the strategy per recursive component, the SQL.
    fn explain_datalog(&self, py: Python<'_>, program: &str) -> PyResult<String> {
        py.detach(|| with_store!(self, s => s.explain_datalog(program)))
            .map_err(datalog_error)
    }

    /// A Synalog program over the store as relational tables; `options` is JSON
    /// (`oxilite_synalog::json`). Returns `{"kind": "synalog", "columns", "rows"}`.
    #[pyo3(signature = (program, predicate, options=None))]
    fn synalog(
        &self,
        py: Python<'_>,
        program: &str,
        predicate: &str,
        options: Option<&str>,
    ) -> PyResult<String> {
        let opts = synalog_args(options)?;
        let r = py
            .detach(|| with_store!(self, s => s.synalog_with(program, predicate, &opts)))
            .map_err(synalog_error)?;
        Ok(oxilite::synalog::json::result_to_json(&r).to_string())
    }

    /// The SQL a Synalog predicate compiles to on this store.
    #[pyo3(signature = (program, predicate, options=None))]
    fn synalog_sql(
        &self,
        py: Python<'_>,
        program: &str,
        predicate: &str,
        options: Option<&str>,
    ) -> PyResult<String> {
        let opts = synalog_args(options)?;
        py.detach(|| with_store!(self, s => s.synalog_sql_with(program, predicate, &opts)))
            .map_err(synalog_error)
    }

    /// Computes the OWL 2 RL closure into the inference table, with SQL rules or (`reasonable`)
    /// in memory; returns the number of inferred triples.
    #[pyo3(signature = (reasonable=false))]
    fn materialize(&self, py: Python<'_>, reasonable: bool) -> PyResult<u64> {
        py.detach(|| {
            if reasonable {
                with_store!(self, s => s.materialize_with_reasonable())
            } else {
                with_store!(self, s => s.materialize())
            }
        })
        .map_err(store_error)
    }

    /// Removes every materialized inference.
    fn clear_inferences(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| with_store!(self, s => s.clear_inferences()))
            .map_err(store_error)
    }

    /// Declares a graph (JSON term) to hold an ontology, SHACL shapes or a ShEx schema;
    /// `registration` is JSON (`{iri, version, sha256, imports, appliesTo, active}`).
    #[pyo3(signature = (graph_json, role, registration=None))]
    fn register_schema_graph(
        &self,
        py: Python<'_>,
        graph_json: &str,
        role: &str,
        registration: Option<&str>,
    ) -> PyResult<()> {
        let graph = required_graph(graph_json)?;
        let role: oxilite::schema::SchemaRole = role.parse().map_err(invalid)?;
        let v: Value = registration.map(parse).transpose()?.unwrap_or(Value::Null);
        let e = oxilite_core::json::schema_graph_from_json(graph.clone(), role, &v)
            .map_err(store_error)?;
        let r = oxilite::schema::Registration {
            iri: e.iri,
            version: e.version,
            sha256: e.sha256,
            imports: e.imports,
            applies_to: e.applies_to,
            active: e.active,
        };
        py.detach(|| with_store!(self, s => s.register_schema_graph(&graph, role, &r)))
            .map_err(store_error)
    }

    /// The registry as JSON: `[{graph, role, iri, version, sha256, imports, appliesTo, active,
    /// loadedAt}]`.
    fn schema_graphs(&self, py: Python<'_>) -> PyResult<String> {
        let rows = py
            .detach(|| with_store!(self, s => s.schema_graphs()))
            .map_err(store_error)?;
        let v: Vec<Value> = rows
            .iter()
            .map(|e| oxilite_core::json::schema_graph_to_json(&e.to_entry()))
            .collect();
        Ok(Value::Array(v).to_string())
    }

    /// Activates or deactivates a registration; returns whether one was found.
    fn set_schema_graph_active(
        &self,
        py: Python<'_>,
        graph_json: &str,
        active: bool,
    ) -> PyResult<bool> {
        let graph = required_graph(graph_json)?;
        py.detach(|| with_store!(self, s => s.set_schema_graph_active(&graph, active)))
            .map_err(store_error)
    }

    /// Removes a registration, keeping the triples; returns whether one was found.
    fn unregister_schema_graph(&self, py: Python<'_>, graph_json: &str) -> PyResult<bool> {
        let graph = required_graph(graph_json)?;
        py.detach(|| with_store!(self, s => s.unregister_schema_graph(&graph)))
            .map_err(store_error)
    }

    /// Removes a registration and every quad of its graph; returns the number removed.
    fn drop_schema_graph(&self, py: Python<'_>, graph_json: &str) -> PyResult<u64> {
        let graph = required_graph(graph_json)?;
        py.detach(|| with_store!(self, s => s.drop_schema_graph(&graph)))
            .map_err(store_error)
    }

    /// The compiled SHACL property shapes as JSON.
    fn shape_index(&self, py: Python<'_>) -> PyResult<String> {
        let i = py
            .detach(|| with_store!(self, s => s.shape_index()))
            .map_err(store_error)?;
        Ok(oxilite_core::json::shape_index_to_json(&i).to_string())
    }

    /// Installs or refreshes the system graphs; returns `false` when they were already current.
    fn install_system_graphs(&self, py: Python<'_>) -> PyResult<bool> {
        py.detach(|| with_store!(self, s => s.install_system_graphs()))
            .map_err(store_error)
    }

    /// A JSON-LD document or credential operation (`put`, `get`, `remove`, `list`, `find`,
    /// `graphs`, `documentForGraph`, `putContext`, `removeContext`, `contexts`, `rebuild`,
    /// `check`, `putCredential`, `putPresentation`); arguments, options and result are JSON.
    #[pyo3(signature = (op, args, options=None))]
    fn jsonld(
        &self,
        py: Python<'_>,
        op: &str,
        args: &str,
        options: Option<&str>,
    ) -> PyResult<String> {
        let args: Value = parse(args)?;
        let opts: Value = options.map(parse).transpose()?.unwrap_or(Value::Null);
        let v = py
            .detach(|| with_store!(self, s => jsonld_op(s, op, &args, &opts)))
            .map_err(jsonld_error)?;
        Ok(v.to_string())
    }

    /// The versioning level and where the clock and history stand, as JSON.
    fn versioning(&self, py: Python<'_>) -> PyResult<String> {
        let s = py
            .detach(|| with_store!(self, s => s.versioning()))
            .map_err(store_error)?;
        serde_json::to_string(&s).map_err(invalid)
    }

    /// Changes the versioning level (`off`, `stamped`, `log`); `change` is JSON (`asOfIndex`,
    /// `stampIndex`, `allowLoss`, `author`, `message`). Returns the status JSON.
    #[pyo3(signature = (level, change=None))]
    fn set_versioning(
        &self,
        py: Python<'_>,
        level: &str,
        change: Option<&str>,
    ) -> PyResult<String> {
        let change: oxilite::version::LevelChange = parse_opt(change)?;
        let level = level.parse().map_err(invalid)?;
        let s = py
            .detach(|| with_store!(self, s => s.set_versioning(level, change.clone())))
            .map_err(store_error)?;
        serde_json::to_string(&s).map_err(invalid)
    }

    /// Author and message (JSON) recorded on the commits of later writes.
    #[pyo3(signature = (info=None))]
    fn set_commit_info(&self, info: Option<&str>) -> PyResult<()> {
        let info: oxilite::version::CommitInfo = parse_opt(info)?;
        with_store!(self, s => s.set_commit_info(info.clone()));
        Ok(())
    }

    /// The latest `limit` commits and level changes, newest first, as JSON.
    fn history(&self, py: Python<'_>, limit: usize) -> PyResult<String> {
        let log = py
            .detach(|| with_store!(self, s => s.history(limit)))
            .map_err(store_error)?;
        serde_json::to_string(&log).map_err(invalid)
    }

    /// The changes after tick `after` (up to `until`): `[{"tick", "added", "quad"}]`.
    #[pyo3(signature = (after, until=None))]
    fn changes(&self, py: Python<'_>, after: i64, until: Option<i64>) -> PyResult<String> {
        let c = py
            .detach(|| with_store!(self, s => s.changes(after, until)))
            .map_err(store_error)?;
        Ok(changes_json(&c))
    }

    /// The net difference between two versions: `[{"tick", "added", "quad"}]`.
    fn diff(&self, py: Python<'_>, from: &str, to: &str) -> PyResult<String> {
        let c = py
            .detach(|| with_store!(self, s => s.diff(from, to)))
            .map_err(store_error)?;
        Ok(changes_json(&c))
    }

    /// Removes the quads matching `pattern` (JSON terms `subject`, `predicate`, `object`,
    /// `graph`, each optional) from the store and its whole history.
    #[pyo3(signature = (pattern, reason=None))]
    fn purge(&self, py: Python<'_>, pattern: &str, reason: Option<&str>) -> PyResult<()> {
        let v: Value = parse(pattern)?;
        let term = |k: &str| -> PyResult<Option<Term>> {
            v.get(k)
                .filter(|t| !t.is_null())
                .map(|t| json_to_term(t).map_err(store_error))
                .transpose()
        };
        let subject = match term("subject")? {
            None => None,
            Some(Term::NamedNode(n)) => Some(NamedOrBlankNode::NamedNode(n)),
            Some(Term::BlankNode(b)) => Some(NamedOrBlankNode::BlankNode(b)),
            Some(_) => return Err(invalid("a purge subject is an IRI or a blank node")),
        };
        let predicate = match term("predicate")? {
            None => None,
            Some(Term::NamedNode(n)) => Some(n),
            Some(_) => return Err(invalid("a purge predicate is an IRI")),
        };
        let object = term("object")?;
        let graph = v
            .get("graph")
            .filter(|t| !t.is_null())
            .map(|g| json_to_graph(g).map_err(store_error))
            .transpose()?;
        py.detach(|| {
            with_store!(self, s => s.purge(
                subject.as_ref().map(Into::into),
                predicate.as_ref().map(Into::into),
                object.as_ref().map(Into::into),
                graph.as_ref().map(Into::into),
                reason,
            ))
        })
        .map_err(store_error)
    }
}

fn cypher_args(
    params: Option<&str>,
    options: Option<&str>,
) -> PyResult<(oxilite::cypher::Params, oxilite::cypher::CypherOptions)> {
    let params = match params {
        Some(p) => oxilite::cypher::json::params_from_json(p).map_err(cypher_error)?,
        None => oxilite::cypher::Params::new(),
    };
    let opts = match options {
        Some(o) => oxilite::cypher::json::options_from_json(o).map_err(cypher_error)?,
        None => oxilite::cypher::CypherOptions::default(),
    };
    Ok((params, opts))
}

fn datalog_args(options: Option<&str>) -> PyResult<oxilite::datalog::Options> {
    let value: Value = options.map(parse).transpose()?.unwrap_or(Value::Null);
    oxilite::datalog::json::options_from_json(&value).map_err(datalog_error)
}

fn synalog_args(options: Option<&str>) -> PyResult<oxilite::synalog::Options> {
    let value: Value = options.map(parse).transpose()?.unwrap_or(Value::Null);
    oxilite::synalog::json::options_from_json(&value).map_err(synalog_error)
}

// ----------------------------------------------------------------------- module functions

/// Parses RDF from `data` or the file at `path`; returns a JSON array of quads.
#[pyfunction]
#[pyo3(signature = (data, path, format, base_iri=None, without_named_graphs=false, rename_blank_nodes=false, lenient=false))]
#[allow(clippy::too_many_arguments)]
fn parse_rdf(
    py: Python<'_>,
    data: Option<Vec<u8>>,
    path: Option<PathBuf>,
    format: &str,
    base_iri: Option<&str>,
    without_named_graphs: bool,
    rename_blank_nodes: bool,
    lenient: bool,
) -> PyResult<String> {
    let mut parser = RdfParser::from_format(rdf_format(format)?);
    if let Some(b) = base_iri {
        parser = parser.with_base_iri(b).map_err(invalid)?;
    }
    if without_named_graphs {
        parser = parser.without_named_graphs();
    }
    if rename_blank_nodes {
        parser = parser.rename_blank_nodes();
    }
    if lenient {
        parser = parser.lenient();
    }
    let filename = path.as_ref().map(|p| p.to_string_lossy().into_owned());
    py.detach(|| {
        let data = read_input(data, path.as_deref())?;
        let quads = parser
            .for_slice(&data)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                syntax_error(
                    e.to_string(),
                    filename.as_deref(),
                    e.location().map(rdf_location),
                )
            })?;
        Ok(quads_json(&quads))
    })
}

/// A term from JSON without validation: the terms a lenient parse returns (relative IRIs,
/// over-long language tags) must serialize back as they were read.
fn unchecked_term(v: &Value) -> PyResult<Term> {
    use oxrdf::{BlankNode, Literal, NamedNode, Triple};
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
    Ok(match s("termType") {
        "NamedNode" => NamedNode::new_unchecked(s("value")).into(),
        "BlankNode" => BlankNode::new_unchecked(s("value")).into(),
        "Literal" => {
            let lang = s("language");
            let dt = v["datatype"]["value"].as_str().unwrap_or_default();
            if lang.is_empty() {
                Literal::new_typed_literal(s("value"), NamedNode::new_unchecked(dt)).into()
            } else {
                Literal::new_language_tagged_literal_unchecked(s("value"), lang).into()
            }
        }
        "Quad" => {
            let subject = match unchecked_term(&v["subject"])? {
                Term::NamedNode(n) => NamedOrBlankNode::from(n),
                Term::BlankNode(b) => NamedOrBlankNode::from(b),
                _ => return Err(invalid("invalid triple subject")),
            };
            let Term::NamedNode(p) = unchecked_term(&v["predicate"])? else {
                return Err(invalid("invalid triple predicate"));
            };
            Triple::new(subject, p, unchecked_term(&v["object"])?).into()
        }
        other => return Err(invalid(format!("unsupported termType {other}"))),
    })
}

fn unchecked_quad(v: &Value) -> PyResult<Quad> {
    let Term::Triple(t) = unchecked_term(&json!({
        "termType": "Quad", "subject": v["subject"], "predicate": v["predicate"], "object": v["object"],
    }))?
    else {
        unreachable!("a Quad JSON is a triple term")
    };
    let graph = match v.get("graph").filter(|g| !g.is_null()) {
        None => GraphName::DefaultGraph,
        Some(g) if g["termType"] == "DefaultGraph" => GraphName::DefaultGraph,
        Some(g) => match unchecked_term(g)? {
            Term::NamedNode(n) => n.into(),
            Term::BlankNode(b) => b.into(),
            _ => return Err(invalid("invalid graph name")),
        },
    };
    Ok(t.in_graph(graph))
}

/// Serializes quads (a JSON array) in an RDF format.
#[pyfunction]
fn serialize_rdf<'py>(
    py: Python<'py>,
    quads_json: &str,
    format: &str,
) -> PyResult<Bound<'py, PyBytes>> {
    let quads = parse::<Vec<Value>>(quads_json)?
        .iter()
        .map(unchecked_quad)
        .collect::<PyResult<Vec<_>>>()?;
    let mut s = RdfSerializer::from_format(rdf_format(format)?).for_writer(Vec::new());
    for q in &quads {
        s.serialize_quad(q)?;
    }
    Ok(PyBytes::new(py, &s.finish()?))
}

/// Serializes a query output (JSON) in a results format, or in an RDF format for triples.
#[pyfunction]
fn serialize_results<'py>(
    py: Python<'py>,
    output_json: &str,
    format: &str,
) -> PyResult<Bound<'py, PyBytes>> {
    let out = json_to_output(&parse(output_json)?)?;
    let text = output_to_format(&out, format).map_err(store_error)?;
    Ok(PyBytes::new(py, text.as_bytes()))
}

/// Parses SPARQL query results from `data` or the file at `path`; returns the output JSON.
#[pyfunction]
#[pyo3(signature = (data, path, format))]
fn parse_query_results(
    py: Python<'_>,
    data: Option<Vec<u8>>,
    path: Option<PathBuf>,
    format: &str,
) -> PyResult<String> {
    let format = results_format(format)?;
    let filename = path.as_ref().map(|p| p.to_string_lossy().into_owned());
    let err = |e: sparesults::QueryResultsSyntaxError| {
        let location = e.location().map(|r| Location {
            line: r.start.line,
            column: r.start.column,
            end_line: r.end.line,
            end_column: r.end.column,
        });
        syntax_error(e.to_string(), filename.as_deref(), location)
    };
    py.detach(|| {
        let data = read_input(data, path.as_deref())?;
        let out = match QueryResultsParser::from_format(format)
            .for_slice(&data)
            .map_err(err)?
        {
            SliceQueryResultsParserOutput::Boolean(b) => QueryOutput::Boolean(b),
            SliceQueryResultsParserOutput::Solutions(solutions) => {
                let variables = solutions.variables().to_vec();
                let rows = solutions
                    .map(|s| {
                        let s = s.map_err(err)?;
                        Ok(variables.iter().map(|v| s.get(v).cloned()).collect())
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                QueryOutput::Solutions { variables, rows }
            }
        };
        Ok(output_to_json(&out).to_string())
    })
}

/// The schema as a SQL script; `options` is store options JSON.
#[pyfunction]
#[pyo3(signature = (options=None))]
fn schema_sql(options: Option<&str>) -> PyResult<String> {
    let options: StoreOptions = parse_opt(options)?;
    Ok(oxilite_core::schema::schema_sql(&options))
}

/// Checks an IRI (`ValueError` when invalid).
#[pyfunction]
fn check_iri(value: &str) -> PyResult<()> {
    oxrdf::NamedNode::new(value).map(|_| ()).map_err(invalid)
}

/// Checks a blank node identifier.
#[pyfunction]
fn check_blank_node(value: &str) -> PyResult<()> {
    oxrdf::BlankNode::new(value).map(|_| ()).map_err(invalid)
}

/// Checks a language tag.
#[pyfunction]
fn check_language(tag: &str) -> PyResult<()> {
    oxrdf::Literal::new_language_tagged_literal("", tag)
        .map(|_| ())
        .map_err(invalid)
}

/// Checks a variable name.
#[pyfunction]
fn check_variable(name: &str) -> PyResult<()> {
    Variable::new(name).map(|_| ()).map_err(invalid)
}

/// A fresh, random blank node identifier.
#[pyfunction]
fn new_blank_node_id() -> String {
    oxrdf::BlankNode::default().into_string()
}

/// The XSD 1.1 canonical `xsd:double` lexical form of a float (`1.0E-1`, `-0.0E0`, `INF`,
/// `NaN`), as pyoxigraph writes it.
#[pyfunction]
fn double_lexical(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "INF" } else { "-INF" }.into();
    }
    // `{:E}` is the shortest round-tripping form (`1E-1`); the canonical mantissa has a dot.
    let s = format!("{value:E}");
    match s.split_once('E') {
        Some((m, e)) if !m.contains('.') => format!("{m}.0E{e}"),
        _ => s,
    }
}

#[pymodule]
#[pyo3(name = "_native")]
fn native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<NativeStore>()?;
    m.add("JsonLdError", m.py().get_type::<JsonLdError>())?;
    m.add_function(wrap_pyfunction!(parse_rdf, m)?)?;
    m.add_function(wrap_pyfunction!(serialize_rdf, m)?)?;
    m.add_function(wrap_pyfunction!(serialize_results, m)?)?;
    m.add_function(wrap_pyfunction!(parse_query_results, m)?)?;
    m.add_function(wrap_pyfunction!(schema_sql, m)?)?;
    m.add_function(wrap_pyfunction!(check_iri, m)?)?;
    m.add_function(wrap_pyfunction!(check_blank_node, m)?)?;
    m.add_function(wrap_pyfunction!(check_language, m)?)?;
    m.add_function(wrap_pyfunction!(check_variable, m)?)?;
    m.add_function(wrap_pyfunction!(new_blank_node_id, m)?)?;
    m.add_function(wrap_pyfunction!(double_lexical, m)?)?;
    Ok(())
}
