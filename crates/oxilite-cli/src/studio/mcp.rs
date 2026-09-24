//! `oxilite mcp`: the studio's operations as Model Context Protocol tools over standard input
//! and output, so an agent can query a project, read its schema, validate it and ask why an
//! inference holds. The project loads like the Project store (from `--root`), or a store file
//! opens read-only (`--location`).
//!
// @lat: [[architecture#Studio server#Agent tools]]

use super::conn::{Target, Vocab};
use super::d1::Handle;
use super::project::Project;
use super::{validate, why};
use oxilite::model::{NamedNode, NamedOrBlankNode, Term, Triple};
use oxilite::sparql::QueryOptions;
use oxilite::store::Store;
use rudof_rdf::rdf_core::RDFFormat;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub struct Mcp {
    project: Option<Project>,
    store: Option<Target>,
}

impl Mcp {
    pub fn open(root: Option<PathBuf>, location: Option<String>) -> Result<Self> {
        match (root, location) {
            (_, Some(location)) => Ok(Self {
                project: None,
                store: Some(Target {
                    store: Handle::Sqlite(Store::open_read_only(&location)?),
                    options: QueryOptions::default(),
                    ephemeral: false,
                    read_only: true,
                }),
            }),
            (root, None) => {
                let root = root.unwrap_or(std::env::current_dir()?).canonicalize()?;
                Ok(Self {
                    project: Some(Project::open(Some(root), ":memory:")?),
                    store: None,
                })
            }
        }
    }

    fn target(&self) -> Target {
        match (&self.project, &self.store) {
            (Some(p), _) => p.target(),
            (None, Some(t)) => t.clone(),
            (None, None) => unreachable!("a project or a store is always open"),
        }
    }

    pub fn tools() -> Value {
        let text = |d: &str| json!({"type": "string", "description": d});
        json!([
            {"name": "sparql_query", "description": "Run a SPARQL 1.1 query (SELECT, ASK, CONSTRUCT, DESCRIBE) against the oxilite store, with the project's reasoning. Returns a tab-separated table or N-Triples.",
             "inputSchema": {"type": "object", "properties": {"query": text("The SPARQL query"), "limit": {"type": "integer", "description": "Maximum rows (default 100)"}}, "required": ["query"]}},
            {"name": "datalog_query", "description": "Run an oxilite Datalog program (rules plus a `?- goal.`) over the store.",
             "inputSchema": {"type": "object", "properties": {"program": text("The Datalog program with a goal"), "limit": {"type": "integer"}}, "required": ["program"]}},
            {"name": "schema", "description": "Summarize the store's vocabulary: prefixes, the most used predicates and classes with counts, and labels. Call this before writing queries.",
             "inputSchema": {"type": "object", "properties": {}}},
            {"name": "validate", "description": "Validate the project's data against its SHACL shapes and list the violations with their files and lines.",
             "inputSchema": {"type": "object", "properties": {}}},
            {"name": "why", "description": "Explain why a triple holds: the rules and premises that derive it, down to asserted statements and their source lines. Terms are IRIs (<…> or bare) or N-Triples literals.",
             "inputSchema": {"type": "object", "properties": {"subject": text("Subject IRI"), "predicate": text("Predicate IRI"), "object": text("Object IRI or literal")}, "required": ["subject", "predicate", "object"]}},
            {"name": "reload", "description": "Reload the project from its files after they changed.",
             "inputSchema": {"type": "object", "properties": {}}}
        ])
    }

    /// Runs one tool; the text is what the agent reads.
    pub fn call(&mut self, name: &str, args: &Value) -> Result<String> {
        let limit = args["limit"].as_u64().unwrap_or(100) as usize;
        match name {
            "sparql_query" => {
                let q = args["query"].as_str().ok_or("`query` is required")?;
                table(&self.target().run(q, limit, false)?)
            }
            "datalog_query" => {
                let p = args["program"].as_str().ok_or("`program` is required")?;
                table(&self.target().datalog(p, limit)?)
            }
            "schema" => {
                let t = self.target();
                let v = Vocab::compute(&t.store, &t.options)?;
                let mut out = String::new();
                if let Some(p) = &self.project {
                    out.push_str("Prefixes:\n");
                    for (k, ns) in p.index().prefixes() {
                        out.push_str(&format!("  {k}: <{ns}>\n"));
                    }
                    out.push_str(&format!("Reasoning: {}\n", p.profile().name()));
                }
                let label = |iri: &str| {
                    v.labels
                        .get(iri)
                        .map(|l| format!(" \"{l}\""))
                        .unwrap_or_default()
                };
                out.push_str("Predicates (uses):\n");
                for (p, n) in v.predicates.iter().take(200) {
                    out.push_str(&format!("  <{p}> {n}{}\n", label(p)));
                }
                out.push_str("Classes (instances):\n");
                for (c, n) in v.classes.iter().take(200) {
                    out.push_str(&format!("  <{c}> {n}{}\n", label(c)));
                }
                Ok(out)
            }
            "validate" => {
                let p = self
                    .project
                    .as_ref()
                    .ok_or("validation needs a project (--root)")?;
                if !p.has_shacl() {
                    return Ok("The project has no SHACL shapes.".into());
                }
                let report = validate::report(
                    p.store(),
                    &p.options(),
                    &p.shapes_ntriples(),
                    &RDFFormat::NTriples,
                    p.validate_inferred(),
                    &|| false,
                )?
                .ok_or("validation was cancelled")?;
                if report.conforms() {
                    return Ok("The data conforms to the shapes.".into());
                }
                let owners = validate::owners(&p.shapes_ntriples());
                let mut out = format!("{} results:\n", report.results().len());
                for r in report.results() {
                    let focus = r.focus_node().to_string();
                    let at = match r.focus_node() {
                        rudof_rdf::rdf_core::term::Object::Iri(i) => {
                            p.index().triple_location(i.as_str(), None)
                        }
                        _ => None,
                    };
                    out.push_str(&format!(
                        "- {focus}{}: {} (shape {}{})\n",
                        r.path().map(|p| format!(" {p}")).unwrap_or_default(),
                        r.message()
                            .iter()
                            .next()
                            .map(|(_, m)| m.clone())
                            .unwrap_or_else(|| r.constraint_component().to_string()),
                        validate::shape_of(r.source(), &owners)
                            .unwrap_or_else(|| "anonymous".into()),
                        at.map(|l| format!(", {}:{}", l.uri, l.start.line + 1))
                            .unwrap_or_default(),
                    ));
                }
                Ok(out)
            }
            "why" => {
                let term = |k: &str| -> Result<Term> {
                    let s = args[k]
                        .as_str()
                        .ok_or_else(|| format!("`{k}` is required"))?
                        .trim();
                    if s.starts_with('"') {
                        let line = format!("<urn:s> <urn:p> {s} .");
                        let q =
                            oxilite::io::RdfParser::from_format(oxilite::io::RdfFormat::NTriples)
                                .for_slice(line.as_bytes())
                                .next()
                                .ok_or("not a literal")??;
                        return Ok(q.object);
                    }
                    Ok(NamedNode::new(s.trim_start_matches('<').trim_end_matches('>'))?.into())
                };
                let s = match term("subject")? {
                    Term::NamedNode(n) => NamedOrBlankNode::from(n),
                    _ => return Err("the subject must be an IRI".into()),
                };
                let Term::NamedNode(p) = term("predicate")? else {
                    return Err("the predicate must be an IRI".into());
                };
                let triple = Triple::new(s, p, term("object")?);
                let target = self.target();
                let empty = super::index::SourceIndex::default();
                let (index, rules) = match &self.project {
                    Some(p) => (
                        p.index(),
                        p.rules()
                            .iter()
                            .map(|(u, t)| (p.producer_name(u), u.clone(), t.clone()))
                            .collect(),
                    ),
                    None => (&empty, Vec::new()),
                };
                let tree = why::Explainer {
                    target: &target,
                    index,
                    rules,
                }
                .why(&triple)?;
                Ok(serde_json::to_string_pretty(&tree)?)
            }
            "reload" => match &mut self.project {
                Some(p) => {
                    p.reload()?;
                    Ok(format!("Reloaded: {} triples.", p.store().len()?))
                }
                None => Ok("A store file does not reload.".into()),
            },
            other => Err(format!("unknown tool {other}").into()),
        }
    }

    /// Answers one JSON-RPC message; `None` for notifications.
    pub fn handle(&mut self, message: &Value) -> Option<Value> {
        let id = message.get("id")?.clone();
        let method = message["method"].as_str().unwrap_or("");
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "oxilite", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "An oxilite RDF store. Call `schema` first to learn the vocabulary, then `sparql_query`. Use `why` to explain an inferred triple and `validate` for SHACL results.",
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": Self::tools()})),
            "tools/call" => {
                let name = message["params"]["name"].as_str().unwrap_or("");
                let args = message["params"]["arguments"].clone();
                Ok(match self.call(name, &args) {
                    Ok(text) => json!({"content": [{"type": "text", "text": text}]}),
                    Err(e) => {
                        json!({"content": [{"type": "text", "text": e.to_string()}], "isError": true})
                    }
                })
            }
            m => Err(json!({"code": -32601, "message": format!("unknown method {m}")})),
        };
        Some(match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        })
    }
}

/// A query payload as text: a tab-separated table, a boolean, or N-Triples.
fn table(payload: &Value) -> Result<String> {
    let term = |t: &Value| -> String {
        match t["termType"].as_str() {
            Some("NamedNode") => format!("<{}>", t["value"].as_str().unwrap_or("")),
            Some("BlankNode") => format!("_:{}", t["value"].as_str().unwrap_or("")),
            Some("Literal") => {
                let v = serde_json::to_string(&t["value"]).unwrap_or_default();
                match (t["language"].as_str(), t["datatype"]["value"].as_str()) {
                    (Some(l), _) if !l.is_empty() => format!("{v}@{l}"),
                    (_, Some(d)) if d != "http://www.w3.org/2001/XMLSchema#string" => {
                        format!("{v}^^<{d}>")
                    }
                    _ => v,
                }
            }
            _ => String::new(),
        }
    };
    let mut out = String::new();
    match payload["kind"].as_str() {
        Some("boolean") => out.push_str(&payload["value"].to_string()),
        Some("quads") => {
            for q in payload["quads"].as_array().into_iter().flatten() {
                out.push_str(&format!(
                    "{} {} {} .\n",
                    term(&q["subject"]),
                    term(&q["predicate"]),
                    term(&q["object"])
                ));
            }
        }
        Some("update") => out.push_str("updated"),
        _ => {
            let vars: Vec<&str> = payload["variables"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            out.push_str(
                &vars
                    .iter()
                    .map(|v| format!("?{v}"))
                    .collect::<Vec<_>>()
                    .join("\t"),
            );
            for row in payload["rows"].as_array().into_iter().flatten() {
                out.push('\n');
                out.push_str(
                    &row.as_array()
                        .into_iter()
                        .flatten()
                        .map(term)
                        .collect::<Vec<_>>()
                        .join("\t"),
                );
            }
        }
    }
    if payload["truncated"] == true {
        out.push_str("\n(more rows: raise `limit`)");
    }
    Ok(out)
}

/// Serves newline-delimited JSON-RPC on standard input and output until input ends.
pub fn serve(root: Option<PathBuf>, location: Option<String>) -> Result<()> {
    let mut mcp = Mcp::open(root, location)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})
            }
        };
        if message.get("error").is_some() && message.get("method").is_none() {
            writeln!(stdout, "{message}")?;
            continue;
        }
        if let Some(answer) = mcp.handle(&message) {
            writeln!(stdout, "{answer}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}
