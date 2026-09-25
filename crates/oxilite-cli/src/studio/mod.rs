//! `oxilite studio-server`: the language server behind oxilite studio (the VS Code extension),
//! speaking LSP over standard input and output plus custom `oxilite/*` requests.
//!
// @lat: [[architecture#Studio server]]

pub mod check;
pub(crate) mod conn;
mod d1;
mod debug;
mod explorer;
pub(crate) mod index;
mod kgtest;
pub(crate) mod lang;
pub mod manifest;
pub mod mcp;
mod project;
pub(crate) mod scanner;
#[cfg(test)]
mod tests;
mod validate;
mod why;

use conn::{Attached, NeedsConfirmation, Target, Vocab};
use lang::{CompletionContext, Lang};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, References, Request as _,
};
use lsp_types::{
    CompletionOptions, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbolResponse, Hover,
    HoverContents, InitializeParams, Location, MarkupContent, MarkupKind, OneOf, Position,
    PublishDiagnosticsParams, Range, ServerCapabilities, SymbolInformation, SymbolKind,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use project::Project;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Rows returned by `oxilite/query` when the client does not ask for a limit.
const DEFAULT_ROW_LIMIT: usize = 10_000;

/// The error code telling the client an update needs the user's confirmation.
pub const NEEDS_CONFIRMATION: i32 = 1001;

const PROJECT: &str = "project";

/// Serves over standard input and output until the client shuts the server down.
pub fn run(store: Option<String>) -> Result<()> {
    let (connection, io_threads) = Connection::stdio();
    serve(&connection, store)?;
    drop(connection);
    io_threads.join()?;
    Ok(())
}

fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![":".into(), "?".into(), "<".into(), "$".into()]),
            ..CompletionOptions::default()
        }),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        ..ServerCapabilities::default()
    }
}

/// Serves one client on `connection`. `store` overrides the scratch store location.
pub fn serve(connection: &Connection, store: Option<String>) -> Result<()> {
    let params: InitializeParams = serde_json::from_value(connection.initialize(json!({
        "capabilities": serde_json::to_value(capabilities())?,
        "serverInfo": {"name": "oxilite studio-server", "version": env!("CARGO_PKG_VERSION")},
    }))?)?;
    let root = root_of(&params);
    let location = store.unwrap_or_else(|| match &root {
        Some(r) => r
            .join(".oxilite")
            .join("studio.sqlite")
            .display()
            .to_string(),
        None => ":memory:".into(),
    });
    let (jobs, done) = crossbeam_channel::unbounded();
    let mut server = Server {
        connection,
        project: Project::open(root, &location)?,
        attached: Vec::new(),
        active: PROJECT.into(),
        docs: HashMap::new(),
        vocab: HashMap::new(),
        diagnostics: HashMap::new(),
        jobs,
        latest: Arc::new(AtomicU64::new(0)),
        report: json!({"conforms": null, "results": []}),
    };
    server.store_changed()?;
    server.start_validation()?;
    loop {
        crossbeam_channel::select! {
            recv(connection.receiver) -> message => {
                let Ok(message) = message else { return Ok(()) };
                match message {
                    Message::Request(request) => {
                        if connection.handle_shutdown(&request)? {
                            return Ok(());
                        }
                        server.request(request)?;
                    }
                    Message::Notification(n) => server.notification(n)?,
                    Message::Response(_) => {}
                }
            }
            recv(done) -> outcome => {
                if let Ok(outcome) = outcome {
                    server.validation_done(outcome)?;
                }
            }
        }
    }
}

struct Doc {
    lang: Lang,
    text: String,
}

struct Server<'a> {
    connection: &'a Connection,
    project: Project,
    attached: Vec<Attached>,
    active: String,
    docs: HashMap<String, Doc>,
    /// Vocabulary per connection, computed on first use and dropped when the store changes.
    vocab: HashMap<String, Vocab>,
    /// Published diagnostics per file and source (`load`, `syntax`, …), merged on publish.
    diagnostics: HashMap<String, BTreeMap<&'static str, Vec<Diagnostic>>>,
    /// Finished background validations come back here.
    jobs: crossbeam_channel::Sender<validate::Outcome>,
    /// The generation the latest validation was started for; older runs stop early.
    latest: Arc<AtomicU64>,
    /// The last SHACL report, for the report view.
    report: Value,
}

fn param<T: serde::de::DeserializeOwned>(params: Value) -> Result<T> {
    Ok(serde_json::from_value(params)?)
}

impl Server<'_> {
    fn request(&mut self, request: Request) -> Result<()> {
        let id = request.id.clone();
        let result = self.dispatch(request);
        let response = match result {
            Ok(Some(value)) => Response::new_ok(id, value),
            Ok(None) => return Ok(()),
            Err(e) if e.is::<NeedsConfirmation>() => {
                let estimate = e
                    .downcast_ref::<NeedsConfirmation>()
                    .and_then(|n| n.estimate.clone());
                Response {
                    id,
                    result: None,
                    error: Some(lsp_server::ResponseError {
                        code: NEEDS_CONFIRMATION,
                        message: e.to_string(),
                        data: Some(json!({"estimate": estimate})),
                    }),
                }
            }
            Err(e) if e.is::<UnknownMethod>() => {
                Response::new_err(id, ErrorCode::MethodNotFound as i32, e.to_string())
            }
            Err(e) => Response::new_err(id, ErrorCode::RequestFailed as i32, e.to_string()),
        };
        Ok(self.connection.sender.send(response.into())?)
    }

    /// Handles a request: `Ok(None)` when a worker thread will answer it.
    fn dispatch(&mut self, request: Request) -> Result<Option<Value>> {
        let id = request.id.clone();
        let p = request.params;
        // Reads run on a worker thread, so a long query never blocks the editor.
        let reads = matches!(
            request.method.as_str(),
            "oxilite/query" | "oxilite/explain" | "oxilite/datalog"
        );
        if reads {
            let text = p["query"]
                .as_str()
                .ok_or("this request needs a `query` string")?
                .to_string();
            let language = p["language"].as_str().unwrap_or("sparql").to_string();
            let is_read = match request.method.as_str() {
                "oxilite/query" => SparqlParserExt::is_query(&text),
                _ => true,
            };
            if is_read {
                // Cypher names resolve against the vocabulary's main namespace: compute it first.
                if language == "cypher" {
                    let _ = self.active_vocab();
                }
                let target = self.target(p["connection"].as_str())?;
                let limit = p["limit"]
                    .as_u64()
                    .map_or(DEFAULT_ROW_LIMIT, |l| l as usize);
                let method = request.method.clone();
                let cypher = self.cypher_options(&target)?;
                let sender = self.connection.sender.clone();
                std::thread::spawn(move || {
                    let result = match (method.as_str(), language.as_str()) {
                        ("oxilite/explain", "datalog") => target.explain_datalog(&text),
                        ("oxilite/explain", "cypher") => target.explain_cypher(&text, &cypher),
                        ("oxilite/explain", _) => target.explain(&text),
                        ("oxilite/datalog", _) => target.datalog(&text, limit),
                        _ => target.run(&text, limit, false),
                    };
                    let response = match result {
                        Ok(v) => Response::new_ok(id, v),
                        Err(e) => {
                            Response::new_err(id, ErrorCode::RequestFailed as i32, e.to_string())
                        }
                    };
                    let _ = sender.send(response.into());
                });
                return Ok(None);
            }
        }
        self.dispatch_now(&request.method, p).map(Some)
    }

    fn dispatch_now(&mut self, method: &str, p: Value) -> Result<Value> {
        match method {
            "oxilite/query" => {
                let text = p["query"]
                    .as_str()
                    .ok_or("oxilite/query needs a `query` string")?;
                let limit = p["limit"]
                    .as_u64()
                    .map_or(DEFAULT_ROW_LIMIT, |l| l as usize);
                let confirmed = p["confirmed"].as_bool().unwrap_or(false);
                let id = p["connection"].as_str().map(str::to_string);
                let out = self.target(id.as_deref())?.run(text, limit, confirmed)?;
                if out["kind"] == "update" {
                    self.after_write(id.as_deref())?;
                }
                Ok(out)
            }
            "oxilite/cypher" => {
                let text = p["query"]
                    .as_str()
                    .ok_or("oxilite/cypher needs a `query` string")?;
                let _ = self.active_vocab();
                let limit = p["limit"]
                    .as_u64()
                    .map_or(DEFAULT_ROW_LIMIT, |l| l as usize);
                let id = p["connection"].as_str().map(str::to_string);
                let target = self.target(id.as_deref())?;
                let options = self.cypher_options(&target)?;
                let out = target.cypher(
                    text,
                    limit,
                    p["confirmed"].as_bool().unwrap_or(false),
                    &options,
                )?;
                if out["writes"] == true {
                    self.after_write(id.as_deref())?;
                }
                Ok(out)
            }
            "oxilite/import" => {
                let path = p["path"].as_str().ok_or("oxilite/import needs a `path`")?;
                let id = p["connection"].as_str().map(str::to_string);
                let out = self.target(id.as_deref())?.import(
                    path,
                    p["graph"].as_str(),
                    p["confirmed"].as_bool().unwrap_or(false),
                )?;
                self.after_write(id.as_deref())?;
                Ok(out)
            }
            "oxilite/export" => {
                let path = p["path"].as_str().ok_or("oxilite/export needs a `path`")?;
                self.target(p["connection"].as_str())?
                    .export(path, p["graph"].as_str())
            }
            "oxilite/explorer" => {
                let node = p["node"].as_str().unwrap_or("root").to_string();
                let vocab = self.active_vocab()?.clone();
                let target = self.target(None)?;
                let project = (self.active == PROJECT).then_some(&self.project);
                explorer::children(&target, &vocab, project, &node, |iri| self.short(iri))
            }
            "oxilite/setReasoning" => {
                let profile = p["profile"]
                    .as_str()
                    .and_then(manifest::Profile::parse)
                    .ok_or("oxilite/setReasoning needs a `profile`: none, rdfs, owlql or owl2rl")?;
                if self.project.set_profile_setting(profile)? {
                    self.store_changed()?;
                    self.start_validation()?;
                }
                self.project.status()
            }
            "oxilite/validationReport" => Ok(self.report.clone()),
            "oxilite/why" => {
                let term = |k: &str| oxilite_core::json::json_to_term(&p[k]);
                let s = match term("s")? {
                    oxilite::model::Term::NamedNode(n) => oxilite::model::NamedOrBlankNode::from(n),
                    oxilite::model::Term::BlankNode(b) => b.into(),
                    _ => return Err("the subject must be an IRI or a blank node".into()),
                };
                let oxilite::model::Term::NamedNode(pred) = term("p")? else {
                    return Err("the predicate must be an IRI".into());
                };
                let triple = oxilite::model::Triple::new(s, pred, term("o")?);
                let target = self.target(p["connection"].as_str())?;
                let rules = if self.active == PROJECT {
                    self.project
                        .rules()
                        .iter()
                        .map(|(uri, text)| {
                            (self.project.producer_name(uri), uri.clone(), text.clone())
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                why::Explainer {
                    target: &target,
                    index: self.project.index(),
                    rules,
                }
                .why(&triple)
            }
            "oxilite/tests" => {
                let runner = kgtest::Runner {
                    project: &self.project,
                };
                let manifest = self.project.root().map(|r| r.join(manifest::MANIFEST));
                let text = manifest
                    .as_ref()
                    .and_then(|m| std::fs::read_to_string(m).ok())
                    .unwrap_or_default();
                let uri = manifest.as_deref().map(project::file_uri).transpose()?;
                Ok(json!(runner
                    .tests()
                    .iter()
                    .map(|t| {
                        let line = text
                            .lines()
                            .position(|l| l.contains("name") && l.contains(&format!("\"{}\"", t.name)))
                            .unwrap_or(0);
                        json!({"name": t.name, "uri": uri, "line": line, "snapshot": t.query.is_some()})
                    })
                    .collect::<Vec<_>>()))
            }
            "oxilite/runTest" => {
                let name = p["name"].as_str().ok_or("oxilite/runTest needs a `name`")?;
                let runner = kgtest::Runner {
                    project: &self.project,
                };
                let spec = runner
                    .tests()
                    .into_iter()
                    .find(|t| t.name == name)
                    .ok_or_else(|| format!("no test named {name}"))?;
                Ok(runner.run(&spec).to_json())
            }
            "oxilite/updateSnapshot" => {
                let name = p["name"]
                    .as_str()
                    .ok_or("oxilite/updateSnapshot needs a `name`")?;
                let runner = kgtest::Runner {
                    project: &self.project,
                };
                let spec = runner
                    .tests()
                    .into_iter()
                    .find(|t| t.name == name)
                    .ok_or_else(|| format!("no test named {name}"))?;
                Ok(json!({"path": runner.update_snapshot(&spec)?}))
            }
            "oxilite/iriAt" => {
                let p: TextDocumentPositionParams = param(p)?;
                let (uri, at) = position(&p);
                Ok(self
                    .iri_at(&uri, at)
                    .map_or(Value::Null, |(iri, _)| json!(iri)))
            }
            "oxilite/validate" => {
                self.start_validation()?;
                Ok(json!({"started": true}))
            }
            "oxilite/describe" => {
                let iri = p["iri"].as_str().ok_or("oxilite/describe needs an `iri`")?;
                let mut out = self.target(p["connection"].as_str())?.describe(iri, 500)?;
                out["definitions"] = json!(self
                    .project
                    .index()
                    .definitions(iri)
                    .iter()
                    .map(location_json)
                    .collect::<Vec<_>>());
                Ok(out)
            }
            "oxilite/status" => self.project.status(),
            "oxilite/reload" => {
                self.reload()?;
                self.project.status()
            }
            "oxilite/connections" => self.connections(),
            "oxilite/attach" => {
                let path = p["path"].as_str().ok_or("oxilite/attach needs a `path`")?;
                let read_only = p["readOnly"].as_bool().unwrap_or(false);
                let attached = Attached::open(path, read_only)?;
                let id = attached.id.clone();
                self.attached.retain(|a| a.id != id);
                self.attached.push(attached);
                // A pinned document or a notebook attaches without switching everyone else.
                if p["activate"].as_bool().unwrap_or(true) {
                    self.active = id;
                }
                self.connections_changed()?;
                self.connections()
            }
            "oxilite/attachD1" => {
                let field = |k: &str| {
                    p[k].as_str()
                        .ok_or_else(|| format!("oxilite/attachD1 needs `{k}`"))
                };
                let attached = Attached::d1(
                    field("account")?,
                    field("database")?,
                    field("token")?,
                    p["readOnly"].as_bool().unwrap_or(true),
                    p["endpoint"].as_str(),
                )?;
                let id = attached.id.clone();
                self.attached.retain(|a| a.id != id);
                self.attached.push(attached);
                // A pinned document or a notebook attaches without switching everyone else.
                if p["activate"].as_bool().unwrap_or(true) {
                    self.active = id;
                }
                self.connections_changed()?;
                self.connections()
            }
            "oxilite/localD1" => Ok(json!(self
                .project
                .root()
                .map(d1::local_databases)
                .unwrap_or_default()
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>())),
            "oxilite/ontology" => explorer::ontology(&self.target(p["connection"].as_str())?),
            "oxilite/datalogDebug" => {
                let program = p["program"]
                    .as_str()
                    .ok_or("oxilite/datalogDebug needs a `program`")?;
                debug::debug(&self.target(p["connection"].as_str())?, program)
            }
            "oxilite/materialize" => {
                let id = p["connection"].as_str().map(str::to_string);
                let out = self
                    .target(id.as_deref())?
                    .materialize(p["confirmed"].as_bool().unwrap_or(false))?;
                self.after_write(id.as_deref())?;
                Ok(out)
            }
            "oxilite/detach" => {
                let id = p["id"].as_str().ok_or("oxilite/detach needs an `id`")?;
                self.attached.retain(|a| a.id != id);
                self.vocab.remove(id);
                if self.active == id {
                    self.active = PROJECT.into();
                }
                self.connections_changed()?;
                self.connections()
            }
            "oxilite/activate" => {
                let id = p["id"].as_str().ok_or("oxilite/activate needs an `id`")?;
                if id != PROJECT && !self.attached.iter().any(|a| a.id == id) {
                    return Err(format!("no connection {id}").into());
                }
                self.active = id.to_string();
                self.connections_changed()?;
                self.refresh_open_documents()?;
                self.connections()
            }
            m if m == Completion::METHOD => {
                let p: lsp_types::CompletionParams = param(p)?;
                let (uri, at) = position(&p.text_document_position);
                let Some(doc) = self.docs.get(&uri) else {
                    return Ok(Value::Null);
                };
                let (lang, text) = (doc.lang, doc.text.clone());
                let vocab = self.active_vocab().ok().cloned();
                let cypher = self.cypher_options(&self.target(None)?)?.vocabulary;
                let ctx = CompletionContext {
                    vocab: vocab.as_ref(),
                    index: self.project.index(),
                    cypher: Some(&cypher),
                };
                Ok(serde_json::to_value(lang::complete(
                    lang, &text, &uri, at, &ctx,
                ))?)
            }
            m if m == HoverRequest::METHOD => {
                let p: lsp_types::HoverParams = param(p)?;
                let (uri, at) = position(&p.text_document_position_params);
                self.hover(&uri, at)
            }
            m if m == GotoDefinition::METHOD => {
                let p: lsp_types::GotoDefinitionParams = param(p)?;
                let (uri, at) = position(&p.text_document_position_params);
                let Some((iri, _)) = self.iri_at(&uri, at) else {
                    return Ok(Value::Null);
                };
                let locs: Vec<Location> = self
                    .project
                    .index()
                    .definitions(&iri)
                    .iter()
                    .filter_map(lsp_location)
                    .collect();
                Ok(serde_json::to_value(locs)?)
            }
            m if m == References::METHOD => {
                let p: lsp_types::ReferenceParams = param(p)?;
                let (uri, at) = position(&p.text_document_position);
                let Some((iri, _)) = self.iri_at(&uri, at) else {
                    return Ok(Value::Null);
                };
                let locs: Vec<Location> = self
                    .project
                    .index()
                    .references(&iri)
                    .iter()
                    .filter_map(lsp_location)
                    .collect();
                Ok(serde_json::to_value(locs)?)
            }
            m if m == DocumentSymbolRequest::METHOD => {
                let p: lsp_types::DocumentSymbolParams = param(p)?;
                let uri = p.text_document.uri.as_str().to_string();
                let Some(doc) = self.docs.get(&uri).filter(|d| d.lang == Lang::Turtle) else {
                    return Ok(Value::Null);
                };
                #[allow(deprecated)]
                let symbols: Vec<SymbolInformation> = lang::subjects(&doc.text, &uri)
                    .into_iter()
                    .map(|(name, range)| SymbolInformation {
                        name,
                        kind: SymbolKind::OBJECT,
                        tags: None,
                        deprecated: None,
                        location: Location::new(p.text_document.uri.clone(), range),
                        container_name: None,
                    })
                    .collect();
                Ok(serde_json::to_value(DocumentSymbolResponse::Flat(symbols))?)
            }
            m => Err(Box::new(UnknownMethod(m.to_string()))),
        }
    }

    /// After a write: the connection's vocabulary is stale and its views must refresh.
    fn after_write(&mut self, id: Option<&str>) -> Result<()> {
        let id = id.unwrap_or(&self.active).to_string();
        self.vocab.remove(&id);
        if id == PROJECT {
            self.notify("oxilite/storeChanged", self.project.status()?)
        } else {
            self.connections_changed()
        }
    }

    /// Cypher options for a connection: its query options, the workspace's prefixes, and a base
    /// namespace for unprefixed names (the manifest's, else the store's most used namespace).
    fn cypher_options(&self, target: &Target) -> Result<oxilite::cypher::CypherOptions> {
        let base = self
            .project
            .manifest()
            .and_then(|m| m.cypher.base.clone())
            .or_else(|| {
                let v = self.vocab.get(&self.active)?;
                let mut counts: BTreeMap<String, u64> = BTreeMap::new();
                for (iri, n) in &v.predicates {
                    let ns = &iri[..iri.rfind(['#', '/']).map_or(0, |i| i + 1)];
                    if !ns.is_empty() && !lang::WELL_KNOWN.iter().any(|(_, w)| *w == ns) {
                        *counts.entry(ns.to_string()).or_default() += n;
                    }
                }
                counts.into_iter().max_by_key(|(_, n)| *n).map(|(ns, _)| ns)
            })
            .unwrap_or_else(|| "urn:oxilite:pg:".into());
        let mut vocabulary = oxilite::cypher::Vocabulary::new(base);
        for (p, ns) in self
            .project
            .index()
            .prefixes()
            .iter()
            .map(|(p, n)| (p.as_str(), n.as_str()))
            .chain(lang::WELL_KNOWN.iter().copied())
        {
            vocabulary = vocabulary.with_prefix(p, ns);
        }
        Ok(oxilite::cypher::CypherOptions {
            vocabulary,
            query: target.options.clone(),
            ..Default::default()
        })
    }

    /// A short, prefixed form of an IRI for tree labels.
    fn short(&self, iri: &str) -> String {
        for (p, ns) in self
            .project
            .index()
            .prefixes()
            .iter()
            .map(|(p, n)| (p.as_str(), n.as_str()))
            .chain(lang::WELL_KNOWN.iter().copied())
        {
            if let Some(local) = iri.strip_prefix(ns) {
                if !local.is_empty() && !local.contains(['/', '#']) {
                    return format!("{p}:{local}");
                }
            }
        }
        iri.to_string()
    }

    /// Starts SHACL validation of the Project store on a worker thread, if there are shapes.
    fn start_validation(&mut self) -> Result<()> {
        let generation = self.project.generation;
        self.latest.store(generation, Ordering::SeqCst);
        if !self.project.has_shapes() || !self.project.validation_enabled() {
            self.report = json!({"conforms": null, "results": []});
            let stale: Vec<String> = self
                .diagnostics
                .iter()
                .filter(|(_, s)| s.contains_key("shacl"))
                .map(|(u, _)| u.clone())
                .collect();
            for uri in stale {
                self.set_diagnostics(&uri, "shacl", Vec::new())?;
            }
            return self.notify("oxilite/validationChanged", self.report.clone());
        }
        let job = validate::Job {
            store: self.project.store().clone(),
            options: self.project.options(),
            shapes: self.project.shapes_ntriples(),
            shape_files: self.project.shape_files(),
            shex: self.project.shex_jobs(),
            index: self.project.index_arc(),
            inferred: self.project.validate_inferred(),
            generation,
            latest: Arc::clone(&self.latest),
        };
        let jobs = self.jobs.clone();
        self.notify(
            "oxilite/validationStarted",
            json!({"generation": generation}),
        )?;
        std::thread::spawn(move || {
            if let Some(outcome) = job.run() {
                let _ = jobs.send(outcome);
            }
        });
        Ok(())
    }

    fn validation_done(&mut self, outcome: validate::Outcome) -> Result<()> {
        if outcome.generation != self.project.generation {
            return Ok(());
        }
        let stale: Vec<String> = self
            .diagnostics
            .iter()
            .filter(|(u, s)| s.contains_key("shacl") && !outcome.diagnostics.contains_key(*u))
            .map(|(u, _)| u.clone())
            .collect();
        for uri in stale {
            self.set_diagnostics(&uri, "shacl", Vec::new())?;
        }
        for (uri, d) in outcome.diagnostics {
            self.set_diagnostics(&uri, "shacl", d)?;
        }
        self.report = outcome.report;
        self.notify("oxilite/validationChanged", self.report.clone())
    }

    fn notification(&mut self, n: Notification) -> Result<()> {
        match n.method.as_str() {
            m if m == DidOpenTextDocument::METHOD => {
                let p: DidOpenTextDocumentParams = param(n.params)?;
                let uri = p.text_document.uri.as_str().to_string();
                if let Some(lang) = Lang::from_id(&p.text_document.language_id, &uri) {
                    self.docs.insert(
                        uri.clone(),
                        Doc {
                            lang,
                            text: p.text_document.text,
                        },
                    );
                    self.check_document(&uri)?;
                }
            }
            m if m == DidChangeTextDocument::METHOD => {
                let p: DidChangeTextDocumentParams = param(n.params)?;
                let uri = p.text_document.uri.as_str().to_string();
                if let (Some(doc), Some(change)) = (
                    self.docs.get_mut(&uri),
                    p.content_changes.into_iter().last(),
                ) {
                    doc.text = change.text;
                    self.check_document(&uri)?;
                }
            }
            m if m == DidCloseTextDocument::METHOD => {
                let p: DidCloseTextDocumentParams = param(n.params)?;
                let uri = p.text_document.uri.as_str().to_string();
                self.docs.remove(&uri);
                self.set_diagnostics(&uri, "syntax", Vec::new())?;
            }
            m if m == DidChangeWatchedFiles::METHOD => {
                let p: lsp_types::DidChangeWatchedFilesParams = param(n.params)?;
                let paths: Vec<PathBuf> = p
                    .changes
                    .iter()
                    .filter_map(|c| url::Url::parse(c.uri.as_str()).ok()?.to_file_path().ok())
                    .collect();
                self.project.reload_paths(&paths)?;
                self.store_changed()?;
                self.start_validation()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The connection `id`, or the active one.
    fn target(&self, id: Option<&str>) -> Result<Target> {
        let id = id.unwrap_or(&self.active);
        if id == PROJECT {
            return Ok(self.project.target());
        }
        let a = self
            .attached
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("no connection {id}"))?;
        Ok(Target {
            store: a.store.clone(),
            options: oxilite::sparql::QueryOptions {
                union_default_graph: a.union_default_graph,
                ..Default::default()
            },
            ephemeral: false,
            read_only: a.read_only,
        })
    }

    fn active_vocab(&mut self) -> Result<&Vocab> {
        if !self.vocab.contains_key(&self.active) {
            let v = self.target(None)?.vocab()?;
            self.vocab.insert(self.active.clone(), v);
        }
        Ok(&self.vocab[&self.active])
    }

    fn iri_at(&self, uri: &str, at: Position) -> Option<(String, Range)> {
        let doc = self.docs.get(uri)?;
        if !matches!(doc.lang, Lang::Sparql | Lang::Turtle | Lang::Datalog) {
            return None;
        }
        lang::iri_at(&doc.text, uri, at)
    }

    fn hover(&mut self, uri: &str, at: Position) -> Result<Value> {
        let Some((iri, range)) = self.iri_at(uri, at) else {
            return Ok(Value::Null);
        };
        let vocab = self.active_vocab().ok().cloned().unwrap_or_default();
        let mut md = String::new();
        if let Some(l) = vocab.labels.get(&iri) {
            md.push_str(&format!("**{l}**\n\n"));
        }
        md.push_str(&format!("`<{iri}>`\n\n"));
        if let Some(c) = vocab.comments.get(&iri) {
            md.push_str(&format!("{c}\n\n"));
        }
        if let Ok(d) = self.target(None)?.describe(&iri, 50) {
            let types: Vec<String> = d["outgoing"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|t| t["p"]["value"] == scanner::RDF_TYPE)
                .filter_map(|t| t["o"]["value"].as_str().map(|v| format!("`{}`", short(v))))
                .collect();
            if !types.is_empty() {
                md.push_str(&format!("type {}\n\n", types.join(", ")));
            }
        }
        let mut facts = Vec::new();
        if let Some(n) = vocab.predicate_count(&iri) {
            facts.push(format!("{n} statements use it as a predicate"));
        }
        if let Some(n) = vocab.class_count(&iri) {
            facts.push(format!("{n} instances"));
        }
        for f in facts {
            md.push_str(&format!("- {f}\n"));
        }
        for d in self.project.index().definitions(&iri).iter().take(5) {
            let file = d.uri.rsplit('/').next().unwrap_or(&d.uri);
            md.push_str(&format!(
                "- defined in [{file}:{}]({}#L{})\n",
                d.start.line + 1,
                d.uri,
                d.start.line + 1
            ));
        }
        Ok(serde_json::to_value(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }),
            range: Some(range),
        })?)
    }

    fn check_document(&mut self, uri: &str) -> Result<()> {
        let Some(doc) = self.docs.get(uri) else {
            return Ok(());
        };
        let (lang, text) = (doc.lang, doc.text.clone());
        let mut diagnostics = lang::syntax_diagnostics(lang, &text, uri);
        if lang == Lang::Sparql && diagnostics.is_empty() {
            if let Ok(v) = self.active_vocab() {
                diagnostics.extend(lang::vocabulary_warnings(&text, uri, v));
            }
        }
        self.set_diagnostics(uri, "syntax", diagnostics)
    }

    /// Re-checks open documents, whose warnings depend on the active connection.
    fn refresh_open_documents(&mut self) -> Result<()> {
        let uris: Vec<String> = self.docs.keys().cloned().collect();
        for uri in uris {
            self.check_document(&uri)?;
        }
        Ok(())
    }

    fn set_diagnostics(
        &mut self,
        uri: &str,
        source: &'static str,
        d: Vec<Diagnostic>,
    ) -> Result<()> {
        let open = self.docs.contains_key(uri);
        let entry = self.diagnostics.entry(uri.to_string()).or_default();
        if d.is_empty() && !entry.contains_key(source) {
            return Ok(());
        }
        entry.insert(source, d);
        // An open buffer's own syntax errors replace the errors of loading the saved file.
        let merged: Vec<Diagnostic> = entry
            .iter()
            .filter(|(s, _)| !(open && **s == "load" && entry.contains_key("syntax")))
            .flat_map(|(_, d)| d.iter().cloned())
            .collect();
        entry.retain(|_, d| !d.is_empty());
        let params = PublishDiagnosticsParams::new(Uri::from_str(uri)?, merged, None);
        self.notify(PublishDiagnostics::METHOD, serde_json::to_value(params)?)
    }

    fn reload(&mut self) -> Result<()> {
        self.project.reload()?;
        self.store_changed()?;
        self.start_validation()
    }

    /// Publishes load diagnostics (clearing fixed files) and notifies `oxilite/storeChanged`.
    fn store_changed(&mut self) -> Result<()> {
        self.vocab.remove(PROJECT);
        let mut errors: HashMap<String, Vec<Diagnostic>> = HashMap::new();
        let at_start = |message: String| Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("oxilite".into()),
            message,
            ..Diagnostic::default()
        };
        for (uri, e) in self.project.reasoning_errors() {
            errors
                .entry(uri.clone())
                .or_default()
                .push(at_start(e.clone()));
        }
        if let (Some(e), Some(root)) = (self.project.layout_error(), self.project.root()) {
            let uri = project::file_uri(&root.join(manifest::MANIFEST))?;
            errors
                .entry(uri)
                .or_default()
                .push(at_start(format!("oxilite.toml: {e}")));
        }
        for file in self.project.files() {
            let Some(error) = &file.error else { continue };
            let start = error.start.unwrap_or((0, 0));
            let end = error
                .end
                .filter(|e| *e != start)
                .unwrap_or((start.0, start.1 + 1));
            errors
                .entry(file.uri.clone())
                .or_default()
                .push(Diagnostic {
                    range: Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1)),
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("oxilite".into()),
                    message: error.message.clone(),
                    ..Diagnostic::default()
                });
        }
        let stale: Vec<String> = self
            .diagnostics
            .iter()
            .filter(|(uri, s)| s.contains_key("load") && !errors.contains_key(*uri))
            .map(|(uri, _)| uri.clone())
            .collect();
        for uri in stale {
            self.set_diagnostics(&uri, "load", Vec::new())?;
        }
        for (uri, d) in errors {
            self.set_diagnostics(&uri, "load", d)?;
        }
        self.refresh_open_documents()?;
        let status = self.project.status()?;
        self.notify("oxilite/storeChanged", status)
    }

    fn connections(&self) -> Result<Value> {
        let mut list = vec![json!({
            "id": PROJECT,
            "kind": "project",
            "label": "Project store",
            "path": self.project.status()?["store"],
            "readOnly": false,
            "active": self.active == PROJECT,
            "triples": self.project.target().store.len()?,
        })];
        for a in &self.attached {
            list.push(json!({
                "id": a.id,
                "kind": a.kind,
                "label": a.label,
                "path": a.path,
                "readOnly": a.read_only,
                "active": self.active == a.id,
                // Counting a remote store is a billed scan: D1 reports its meter instead.
                "triples": if a.kind == "d1" { Value::Null } else { json!(a.store.len()?) },
                "billing": a.store.meter().map(|m| m.to_json()),
            }));
        }
        Ok(json!(list))
    }

    fn connections_changed(&self) -> Result<()> {
        self.notify("oxilite/connectionsChanged", self.connections()?)
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        let n = Notification::new(method.to_string(), params);
        Ok(self.connection.sender.send(n.into())?)
    }
}

#[derive(Debug)]
struct UnknownMethod(String);

impl std::fmt::Display for UnknownMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown method {}", self.0)
    }
}

impl std::error::Error for UnknownMethod {}

fn position(p: &TextDocumentPositionParams) -> (String, Position) {
    (p.text_document.uri.as_str().to_string(), p.position)
}

fn lsp_location(l: &index::Location) -> Option<Location> {
    Some(Location::new(
        Uri::from_str(&l.uri).ok()?,
        Range::new(
            Position::new(l.start.line, l.start.col),
            Position::new(l.end.line, l.end.col),
        ),
    ))
}

fn location_json(l: &index::Location) -> Value {
    json!({
        "uri": l.uri,
        "range": {"start": {"line": l.start.line, "character": l.start.col}, "end": {"line": l.end.line, "character": l.end.col}},
    })
}

/// A prefixed form of well-known IRIs, for hover text.
fn short(iri: &str) -> String {
    for (p, ns) in lang::WELL_KNOWN {
        if let Some(local) = iri.strip_prefix(ns) {
            return format!("{p}:{local}");
        }
    }
    format!("<{iri}>")
}

/// The first workspace folder, or the deprecated `rootUri`, as a local path.
fn root_of(params: &InitializeParams) -> Option<PathBuf> {
    #[allow(deprecated)]
    let uri = params
        .workspace_folders
        .as_ref()
        .and_then(|f| f.first())
        .map(|f| f.uri.as_str().to_string())
        .or_else(|| params.root_uri.as_ref().map(|u| u.as_str().to_string()))?;
    url::Url::parse(&uri).ok()?.to_file_path().ok()
}

/// Tells a query from an update without running it.
struct SparqlParserExt;

impl SparqlParserExt {
    fn is_query(text: &str) -> bool {
        oxilite::sparql::SparqlParser::new()
            .parse_query(text)
            .is_ok()
            || oxilite::sparql::SparqlParser::new()
                .parse_update(text)
                .is_err()
    }
}
