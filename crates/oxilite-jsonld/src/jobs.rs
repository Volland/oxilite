//! Step machines of document storage: write (put, replace, remove), read, check, rebuild.
//!
//! Every write is one atomic request (one D1 batch). Reads happen only when needed: the
//! persisted contexts a document imports, and — for graph strategies where documents share
//! a graph — the previous version of a document, whose exact quads are deleted.
//!
// @lat: [[architecture#JSON-LD documents#Write path]]

use crate::convert::{convert, Attempt, Converted};
use crate::error::{JsonLdError, Result};
use crate::options::{hex, sha256, JsonLdOptions};
use json_ld::Loader;
use json_syntax::Parse;
use oxilite_core::encoding::{graph_id, tag_of, EncodedRows, Tag, DEFAULT_GRAPH_ID};
use oxilite_core::job::{Job, Step};
use oxilite_core::sql::{col, quote_str, sql_f64, sql_str};
use oxilite_core::writer::{atomic_request, quad_delete_statements, EncodedQuads};
use oxilite_core::{Capabilities, Request, Response, SqlValue, Statement};
use oxrdf::{BlankNode, GraphName, GraphNameRef, NamedNode, Quad};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// Maximum rounds of context loading (each round may discover imported contexts).
pub const MAX_CONTEXT_ROUNDS: usize = 8;

/// `stored_at` as epoch seconds, computed by SQLite (no clock needed in wasm).
const NOW: &str = "((julianday('now') - 2440587.5) * 86400.0)";

/// Metadata stored next to a document (profiles such as credentials fill it).
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentMeta {
    /// `jsonld` for generic documents; `vc1`, `vc2`, `vp1`, `vp2` for credentials.
    pub profile: String,
    pub issuer: Option<String>,
    pub subject: Option<String>,
    pub types: Vec<String>,
    /// Epoch seconds.
    pub valid_from: Option<f64>,
    /// Epoch seconds.
    pub valid_until: Option<f64>,
    /// Keys of documents this one embeds (a presentation's credentials).
    pub refs: Vec<String>,
}

impl Default for DocumentMeta {
    fn default() -> Self {
        Self {
            profile: "jsonld".into(),
            issuer: None,
            subject: None,
            types: Vec::new(),
            valid_from: None,
            valid_until: None,
            refs: Vec::new(),
        }
    }
}

/// A document to write.
#[derive(Debug, Clone, Default)]
pub struct DocumentInput {
    /// The JSON text, stored verbatim.
    pub json: String,
    /// The key, required with `KeyStrategy::Explicit`, overriding the strategy otherwise.
    pub key: Option<String>,
    pub meta: DocumentMeta,
}

impl DocumentInput {
    pub fn new(json: impl Into<String>) -> Self {
        Self {
            json: json.into(),
            ..Default::default()
        }
    }
}

/// A stored document.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredDocument {
    pub key: String,
    /// The graph its default-graph triples were written to.
    pub graph: GraphName,
    /// The JSON exactly as it was supplied.
    pub json: String,
    /// Hex SHA-256 of `json`.
    pub sha256: String,
    pub meta: DocumentMeta,
    /// Epoch seconds.
    pub stored_at: f64,
}

/// Result of a write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteOutcome {
    /// Keys of the documents written, in input order.
    pub keys: Vec<String>,
    /// For each removed key, whether it existed.
    pub removed: Vec<bool>,
}

/// Credential-style lookup over the metadata columns.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentFilter {
    pub issuer: Option<String>,
    pub subject: Option<String>,
    /// A value of the `types` array (e.g. `UniversityDegreeCredential`).
    pub type_: Option<String>,
    /// Valid at this instant (epoch seconds): `valid_from <= t < valid_until`, open ends allowed.
    pub valid_at: Option<f64>,
    pub profile: Option<String>,
    /// Keyset paging: only keys greater than this.
    pub after: Option<String>,
    pub limit: usize,
}

impl Default for DocumentFilter {
    fn default() -> Self {
        Self {
            issuer: None,
            subject: None,
            type_: None,
            valid_at: None,
            profile: None,
            after: None,
            limit: 100,
        }
    }
}

/// A report entry of [`CheckJob`]: a document whose graphs differ from its conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    pub key: String,
    /// Quads the raw document yields that are missing from the store.
    pub missing: usize,
    /// Quads in the document's own graphs that the raw document does not yield.
    pub extra: usize,
}

// ---------------------------------------------------------------------------------------
// Write

struct Prepared {
    key: String,
    target: GraphName,
    doc: json_syntax::Value,
    raw: String,
    meta: DocumentMeta,
    converted: Option<Converted>,
}

struct Old {
    key: String,
    target: GraphName,
    doc: json_syntax::Value,
    converted: Option<Converted>,
}

enum Phase {
    Start,
    ReadOld,
    ReadContexts,
    Write(Vec<usize>),
    Done,
}

/// Puts and removes documents in one atomic request.
///
/// `S` is the first context loader (e.g. bundled credential contexts); use
/// [`crate::NoContexts`] for none.
pub struct WriteJob<S> {
    options: JsonLdOptions,
    first: S,
    caps: Capabilities,
    puts: Vec<Prepared>,
    removes: Vec<String>,
    old: Vec<Old>,
    known: BTreeMap<String, String>,
    fetched: BTreeMap<String, String>,
    queried: BTreeSet<String>,
    rounds: usize,
    phase: Phase,
}

impl<S: Loader> WriteJob<S> {
    /// Prepares a write; keys and target graphs are resolved (and validated) here.
    pub fn new(
        puts: Vec<DocumentInput>,
        removes: Vec<String>,
        options: JsonLdOptions,
        first: S,
        caps: Capabilities,
    ) -> Result<Self> {
        let mut prepared: Vec<Prepared> = Vec::with_capacity(puts.len());
        for input in puts {
            let (doc, _) = json_syntax::Value::parse_str(&input.json)
                .map_err(|e| JsonLdError::Json(e.to_string()))?;
            let key = options.key_for(&input.json, &doc, input.key.as_deref())?;
            let target = options.graph_for(&key)?;
            // The last version of a key wins (a presentation may embed a credential twice).
            prepared.retain(|p| p.key != key);
            prepared.push(Prepared {
                key,
                target,
                doc,
                raw: input.json,
                meta: input.meta,
                converted: None,
            });
        }
        let known = options.contexts.clone();
        Ok(Self {
            options,
            first,
            caps,
            puts: prepared,
            removes,
            old: Vec::new(),
            known,
            fetched: BTreeMap::new(),
            queried: BTreeSet::new(),
            rounds: 0,
            phase: Phase::Start,
        })
    }

    /// Keys of the documents being written.
    pub fn keys(&self) -> Vec<String> {
        self.puts.iter().map(|p| p.key.clone()).collect()
    }

    fn all_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.puts.iter().map(|p| p.key.clone()).collect();
        for k in &self.removes {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        keys
    }

    /// Converts whatever is not converted yet; asks for contexts or builds the write.
    fn resolve(&mut self) -> Result<Step<WriteOutcome>> {
        loop {
            let mut misses = BTreeSet::new();
            for p in self.puts.iter_mut().filter(|p| p.converted.is_none()) {
                match convert(
                    &p.doc,
                    &p.key,
                    &p.target,
                    &self.options,
                    &self.known,
                    &self.first,
                )? {
                    Attempt::Done(c) => p.converted = Some(c),
                    Attempt::Missing(m) => misses.extend(m),
                }
            }
            for o in self.old.iter_mut().filter(|o| o.converted.is_none()) {
                // An old version that no longer converts is replaced without the exact delete.
                match convert(
                    &o.doc,
                    &o.key,
                    &o.target,
                    &self.options,
                    &self.known,
                    &self.first,
                ) {
                    Ok(Attempt::Done(c)) => o.converted = Some(c),
                    Ok(Attempt::Missing(m)) => misses.extend(m),
                    Err(_) => o.converted = Some(Converted::default()),
                }
            }
            if misses.is_empty() {
                return self.write();
            }
            self.rounds += 1;
            if self.rounds > MAX_CONTEXT_ROUNDS {
                return Err(JsonLdError::ContextNotFound(
                    misses.into_iter().next().unwrap_or_default(),
                ));
            }
            let unqueried: Vec<String> = misses.difference(&self.queried).cloned().collect();
            if !unqueried.is_empty() {
                self.queried.extend(unqueried.iter().cloned());
                self.phase = Phase::ReadContexts;
                return Ok(Step::Execute(contexts_request(&unqueried)));
            }
            let Some(fetch) = self.options.fetcher.clone() else {
                return Err(JsonLdError::ContextNotFound(
                    misses.into_iter().next().unwrap_or_default(),
                ));
            };
            for iri in misses {
                let text = fetch(&iri)?;
                json_syntax::Value::parse_str(&text).map_err(|e| {
                    JsonLdError::ContextNotFound(format!("{iri}: invalid JSON: {e}"))
                })?;
                if self.options.cache_fetched {
                    self.fetched.insert(iri.clone(), text.clone());
                }
                self.known.insert(iri, text);
            }
        }
    }

    fn write(&mut self) -> Result<Step<WriteOutcome>> {
        let caps = &self.caps;
        let mut prefix = Vec::new();
        for key in self.all_keys() {
            let k = sql_str(&key);
            prefix.push(Statement::new(format!(
                "DELETE FROM quads WHERE g IN (SELECT g FROM jsonld_graphs WHERE key = {k})"
            )));
            prefix.push(Statement::new(format!(
                "DELETE FROM graphs WHERE id IN (SELECT g FROM jsonld_graphs WHERE key = {k})"
            )));
            prefix.push(Statement::new(format!(
                "DELETE FROM jsonld_graphs WHERE key = {k}"
            )));
        }
        // Shared graphs: delete exactly the previous version's triples.
        let mut old_ids = Vec::new();
        for o in &self.old {
            let mut rows = EncodedRows::default();
            if let Some(c) = &o.converted {
                for q in c.quads.iter().filter(|q| q.graph_name == o.target) {
                    old_ids.push(rows.quad(q.as_ref()));
                }
            }
        }
        if !old_ids.is_empty() {
            prefix.extend(quad_delete_statements(&old_ids, caps));
        }

        let mut enc = EncodedQuads::default();
        let mut owned: Vec<(String, i64)> = Vec::new();
        let mut suffix = Vec::new();
        for p in &self.puts {
            let c = p.converted.as_ref().expect("converted before write");
            for q in &c.quads {
                let ids = enc.rows.quad(q.as_ref());
                enc.quads.push(ids);
            }
            let mut graphs: Vec<&GraphName> = c.nested_graphs.iter().collect();
            if self.options.graph.owns_target() {
                graphs.push(&p.target);
            }
            for g in graphs {
                let id = enc.rows.graph(g.as_ref());
                if id != DEFAULT_GRAPH_ID && !owned.iter().any(|(_, x)| *x == id) {
                    owned.push((p.key.clone(), id));
                }
            }
        }
        enc.rows.dedup();
        if !owned.is_empty() {
            let mut graphs = String::from("INSERT OR IGNORE INTO graphs(id) VALUES ");
            let mut links = String::from("INSERT INTO jsonld_graphs(key, g) VALUES ");
            for (i, (k, id)) in owned.iter().enumerate() {
                if i > 0 {
                    graphs.push(',');
                    links.push(',');
                }
                let _ = write!(graphs, "({id})");
                links.push('(');
                quote_str(&mut links, k);
                let _ = write!(links, ",{id})");
            }
            suffix.push(Statement::new(graphs));
            suffix.push(Statement::new(links));
        }
        for p in &self.puts {
            suffix.extend(document_statements(p, caps));
        }
        let mut removed_at = Vec::new();
        for k in &self.removes {
            if self.puts.iter().any(|p| &p.key == k) {
                continue;
            }
            removed_at.push(usize::MAX);
            suffix.push(Statement::new(format!(
                "DELETE FROM jsonld_documents WHERE key = {}",
                sql_str(k)
            )));
            let n = suffix.len() - 1;
            *removed_at.last_mut().unwrap() = n;
        }
        for (iri, text) in &self.fetched {
            suffix.push(Statement::new(format!(
                "INSERT OR REPLACE INTO jsonld_contexts(iri, doc) VALUES ({}, {})",
                sql_str(iri),
                sql_str(text)
            )));
        }
        let before = prefix.len() + enc_statement_count(&enc, caps);
        let request = atomic_request(prefix, &enc, suffix, caps).map_err(|n| {
            JsonLdError::DocumentTooLarge {
                statements: n,
                limit: caps.max_statements,
            }
        })?;
        let removed_at = removed_at.into_iter().map(|i| before + i).collect();
        self.phase = Phase::Write(removed_at);
        Ok(Step::Execute(request))
    }
}

fn enc_statement_count(enc: &EncodedQuads, caps: &Capabilities) -> usize {
    enc.insert_statements(caps).len()
}

/// `INSERT OR REPLACE` of the document row; long documents are appended in chunks
/// (`doc = doc || …`) so every statement stays below the backend's SQL-length limit.
fn document_statements(p: &Prepared, caps: &Capabilities) -> Vec<Statement> {
    // Quotes may double in SQL, so a chunk is at most half the available length.
    let max = (caps.max_sql_len.saturating_sub(4096) / 2).max(1024);
    let chunks = split_chars(&p.raw, max);
    let m = &p.meta;
    let mut first = String::from(
        "INSERT OR REPLACE INTO jsonld_documents(key, graph, doc, sha256, profile, issuer, subject, types, valid_from, valid_until, refs, stored_at) VALUES (",
    );
    quote_str(&mut first, &p.key);
    let _ = write!(first, ",{},", graph_id(p.target.as_ref()));
    quote_str(&mut first, chunks.first().copied().unwrap_or(""));
    first.push(',');
    quote_str(&mut first, &hex(&sha256(&p.raw)));
    first.push(',');
    quote_str(&mut first, &m.profile);
    let _ = write!(
        first,
        ",{},{},{},{},{},{},{NOW})",
        opt(m.issuer.as_deref()),
        opt(m.subject.as_deref()),
        if m.types.is_empty() {
            "NULL".into()
        } else {
            sql_str(&json_array(&m.types))
        },
        m.valid_from.map_or_else(|| "NULL".into(), sql_f64),
        m.valid_until.map_or_else(|| "NULL".into(), sql_f64),
        if m.refs.is_empty() {
            "NULL".into()
        } else {
            sql_str(&json_array(&m.refs))
        },
    );
    let mut out = vec![Statement::new(first)];
    for c in chunks.iter().skip(1) {
        out.push(Statement::new(format!(
            "UPDATE jsonld_documents SET doc = doc || {} WHERE key = {}",
            sql_str(c),
            sql_str(&p.key)
        )));
    }
    out
}

fn opt(v: Option<&str>) -> String {
    v.map_or_else(|| "NULL".into(), sql_str)
}

fn split_chars(s: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while rest.len() > max {
        let mut cut = max;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    out.push(rest);
    out
}

/// A JSON array of strings.
pub fn json_array(items: &[String]) -> String {
    let mut out = String::from("[");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_string(&mut out, s);
    }
    out.push(']');
    out
}

fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn parse_json_array(s: &str) -> Vec<String> {
    match json_syntax::Value::parse_str(s) {
        Ok((json_syntax::Value::Array(a), _)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn contexts_request(iris: &[String]) -> Request {
    let list = iris
        .iter()
        .map(|i| sql_str(i))
        .collect::<Vec<_>>()
        .join(",");
    Request::read(vec![Statement::new(format!(
        "SELECT iri, doc FROM jsonld_contexts WHERE iri IN ({list})"
    ))])
}

impl<S: Loader> Job for WriteJob<S> {
    type Output = WriteOutcome;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<WriteOutcome>> {
        self.step_jsonld(response).map_err(into_core)
    }
}

impl<S: Loader> WriteJob<S> {
    /// [`Job::step`] with this crate's error type.
    pub fn step_jsonld(&mut self, response: Option<Response>) -> Result<Step<WriteOutcome>> {
        match std::mem::replace(&mut self.phase, Phase::Done) {
            Phase::Start => {
                let keys = self.all_keys();
                if !self.options.graph.owns_target() && !keys.is_empty() {
                    self.phase = Phase::ReadOld;
                    let list = keys
                        .iter()
                        .map(|k| sql_str(k))
                        .collect::<Vec<_>>()
                        .join(",");
                    return Ok(Step::Execute(Request::read(vec![Statement::new(format!(
                        "SELECT key, doc FROM jsonld_documents WHERE key IN ({list})"
                    ))])));
                }
                self.resolve()
            }
            Phase::ReadOld => {
                let response = response.unwrap_or_default();
                for row in response.into_iter().next().unwrap_or_default().rows {
                    let (Some(key), Some(doc)) = (row[0].as_str(), row[1].as_str()) else {
                        continue;
                    };
                    let Ok((doc, _)) = json_syntax::Value::parse_str(doc) else {
                        continue;
                    };
                    let Ok(target) = self.options.graph_for(key) else {
                        continue;
                    };
                    self.old.push(Old {
                        key: key.to_owned(),
                        target,
                        doc,
                        converted: None,
                    });
                }
                self.resolve()
            }
            Phase::ReadContexts => {
                let response = response.unwrap_or_default();
                for row in response.into_iter().next().unwrap_or_default().rows {
                    if let (Some(iri), Some(doc)) = (row[0].as_str(), row[1].as_str()) {
                        self.known.insert(iri.to_owned(), doc.to_owned());
                    }
                }
                self.resolve()
            }
            Phase::Write(removed_at) => {
                let response = response.unwrap_or_default();
                let removed = removed_at
                    .iter()
                    .map(|i| response.get(*i).is_some_and(|r| r.changes > 0))
                    .collect();
                Ok(Step::Done(WriteOutcome {
                    keys: self.keys(),
                    removed,
                }))
            }
            Phase::Done => Err(JsonLdError::Store(oxilite_core::Error::Other(
                "write job resumed after completion".into(),
            ))),
        }
    }
}

/// Carries a [`JsonLdError`] through `oxilite_core::Error` (see [`from_core`]).
pub fn into_core(e: JsonLdError) -> oxilite_core::Error {
    match e {
        JsonLdError::Store(e) => e,
        e => oxilite_core::Error::Other(format!("{ERROR_TAG}{}", encode_error(&e))),
    }
}

const ERROR_TAG: &str = "oxilite-jsonld:";

/// Recovers a [`JsonLdError`] from an error returned by a job driver.
pub fn from_core(e: oxilite_core::Error) -> JsonLdError {
    if let oxilite_core::Error::Other(m) = &e {
        if let Some(rest) = m.strip_prefix(ERROR_TAG) {
            if let Some(e) = decode_error(rest) {
                return e;
            }
        }
    }
    JsonLdError::from_store(e)
}

fn encode_error(e: &JsonLdError) -> String {
    match e {
        JsonLdError::Json(m) => format!("json\u{1}{m}"),
        JsonLdError::JsonLd { code, message } => format!("jsonld\u{1}{code}\u{1}{message}"),
        JsonLdError::ContextNotFound(m) => format!("context\u{1}{m}"),
        JsonLdError::MissingKey(m) => format!("key\u{1}{m}"),
        JsonLdError::InvalidGraphName(m) => format!("graph\u{1}{m}"),
        JsonLdError::GraphOwned(m) => format!("owned\u{1}{m}"),
        JsonLdError::DocumentTooLarge { statements, limit } => {
            format!("large\u{1}{statements}\u{1}{limit}")
        }
        JsonLdError::Invalid(m) => format!("invalid\u{1}{m}"),
        JsonLdError::Store(e) => format!("store\u{1}{e}"),
    }
}

fn decode_error(s: &str) -> Option<JsonLdError> {
    let mut parts = s.split('\u{1}');
    let kind = parts.next()?;
    let a = parts.next().unwrap_or_default().to_owned();
    let b = parts.next().unwrap_or_default().to_owned();
    Some(match kind {
        "json" => JsonLdError::Json(a),
        "jsonld" => JsonLdError::JsonLd {
            code: a,
            message: b,
        },
        "context" => JsonLdError::ContextNotFound(a),
        "key" => JsonLdError::MissingKey(a),
        "graph" => JsonLdError::InvalidGraphName(a),
        "owned" => JsonLdError::GraphOwned(a),
        "large" => JsonLdError::DocumentTooLarge {
            statements: a.parse().ok()?,
            limit: b.parse().ok()?,
        },
        "invalid" => JsonLdError::Invalid(a),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------
// Reads

const DOC_COLUMNS: &str = "d.key, CAST(d.graph AS TEXT), d.doc, d.sha256, d.profile, d.issuer, d.subject, d.types, d.valid_from, d.valid_until, d.refs, d.stored_at, t.lex";
const DOC_FROM: &str = "jsonld_documents d LEFT JOIN terms t ON t.id = d.graph";

fn decode_graph(id: i64, lex: Option<&str>) -> oxilite_core::Result<GraphName> {
    if id == DEFAULT_GRAPH_ID {
        return Ok(GraphName::DefaultGraph);
    }
    let lex =
        lex.ok_or_else(|| oxilite_core::Error::corrupted(format!("graph {id} has no term")))?;
    Ok(match tag_of(id) {
        Some(Tag::BlankNode) => GraphName::BlankNode(BlankNode::new_unchecked(lex)),
        _ => GraphName::NamedNode(NamedNode::new_unchecked(lex)),
    })
}

fn decode_document(row: &[SqlValue]) -> oxilite_core::Result<StoredDocument> {
    let s = |i: usize| -> oxilite_core::Result<Option<String>> {
        Ok(col(row, i)?.clone().into_string())
    };
    let graph_id = col(row, 1)?
        .as_i64()
        .ok_or_else(|| oxilite_core::Error::corrupted("document graph id"))?;
    Ok(StoredDocument {
        key: s(0)?.unwrap_or_default(),
        graph: decode_graph(graph_id, col(row, 12)?.as_str())?,
        json: s(2)?.unwrap_or_default(),
        sha256: s(3)?.unwrap_or_default(),
        meta: DocumentMeta {
            profile: s(4)?.unwrap_or_else(|| "jsonld".into()),
            issuer: s(5)?,
            subject: s(6)?,
            types: s(7)?.map(|t| parse_json_array(&t)).unwrap_or_default(),
            valid_from: col(row, 8)?.as_f64(),
            valid_until: col(row, 9)?.as_f64(),
            refs: s(10)?.map(|t| parse_json_array(&t)).unwrap_or_default(),
        },
        stored_at: col(row, 11)?.as_f64().unwrap_or_default(),
    })
}

fn documents_job(sql: String) -> oxilite_core::job::OneShot<Vec<StoredDocument>> {
    oxilite_core::job::OneShot::new(Request::read(vec![Statement::new(sql)]), |r| {
        r.into_iter()
            .next()
            .unwrap_or_default()
            .rows
            .iter()
            .map(|row| decode_document(row))
            .collect()
    })
}

/// Reads one document by key.
pub fn get_job(key: &str) -> impl Job<Output = Vec<StoredDocument>> {
    documents_job(format!(
        "SELECT {DOC_COLUMNS} FROM {DOC_FROM} WHERE d.key = {}",
        sql_str(key)
    ))
}

/// Lists documents by key (keyset paging).
pub fn list_job(after: Option<&str>, limit: usize) -> impl Job<Output = Vec<StoredDocument>> {
    find_job(&DocumentFilter {
        after: after.map(str::to_owned),
        limit,
        ..Default::default()
    })
}

/// Finds documents by metadata, answered from the document table in one request.
pub fn find_job(filter: &DocumentFilter) -> impl Job<Output = Vec<StoredDocument>> {
    documents_job(find_sql(filter))
}

/// The SQL of [`find_job`].
pub fn find_sql(f: &DocumentFilter) -> String {
    let mut w = Vec::new();
    if let Some(v) = &f.issuer {
        w.push(format!("d.issuer = {}", sql_str(v)));
    }
    if let Some(v) = &f.subject {
        w.push(format!("d.subject = {}", sql_str(v)));
    }
    if let Some(v) = &f.type_ {
        // Exact match of one array element: the JSON-quoted string, without JSON1.
        let mut quoted = String::new();
        json_string(&mut quoted, v);
        w.push(format!("instr(d.types, {}) > 0", sql_str(&quoted)));
    }
    if let Some(t) = f.valid_at {
        let t = sql_f64(t);
        w.push(format!(
            "(d.valid_from IS NULL OR d.valid_from <= {t}) AND (d.valid_until IS NULL OR d.valid_until > {t})"
        ));
    }
    if let Some(v) = &f.profile {
        w.push(format!("d.profile = {}", sql_str(v)));
    }
    if let Some(v) = &f.after {
        w.push(format!("d.key > {}", sql_str(v)));
    }
    let mut sql = format!("SELECT {DOC_COLUMNS} FROM {DOC_FROM}");
    if !w.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&w.join(" AND "));
    }
    let _ = write!(sql, " ORDER BY d.key LIMIT {}", f.limit.max(1));
    sql
}

/// The document owning a graph (or, for shared graphs, the first document targeting it).
pub fn document_for_graph_job(graph: GraphNameRef<'_>) -> impl Job<Output = Vec<StoredDocument>> {
    let id = graph_id(graph);
    documents_job(if id == DEFAULT_GRAPH_ID {
        // The default graph is never a document's own graph.
        format!("SELECT {DOC_COLUMNS} FROM {DOC_FROM} WHERE 0")
    } else {
        format!(
            "SELECT {DOC_COLUMNS} FROM {DOC_FROM} WHERE d.key = (SELECT key FROM jsonld_graphs WHERE g = {id}) \
             UNION ALL SELECT * FROM (SELECT {DOC_COLUMNS} FROM {DOC_FROM} WHERE d.graph = {id} \
             AND NOT EXISTS (SELECT 1 FROM jsonld_graphs WHERE g = {id}) ORDER BY d.key LIMIT 1)"
        )
    })
}

/// The graphs a document owns (its target graph when it has one of its own, and the graphs
/// it defines).
pub fn document_graphs_job(key: &str) -> impl Job<Output = Vec<GraphName>> {
    oxilite_core::job::OneShot::new(
        Request::read(vec![Statement::new(format!(
            "SELECT CAST(jg.g AS TEXT), t.lex FROM jsonld_graphs jg LEFT JOIN terms t ON t.id = jg.g WHERE jg.key = {} ORDER BY t.lex",
            sql_str(key)
        ))]),
        |r| {
            r.into_iter()
                .next()
                .unwrap_or_default()
                .rows
                .iter()
                .map(|row| {
                    let id = col(row, 0)?
                        .as_i64()
                        .ok_or_else(|| oxilite_core::Error::corrupted("graph id"))?;
                    decode_graph(id, col(row, 1)?.as_str())
                })
                .collect()
        },
    )
}

/// Saves a context for offline loading.
pub fn put_context_request(iri: &str, json: &str) -> Result<Request> {
    json_syntax::Value::parse_str(json).map_err(|e| JsonLdError::Json(e.to_string()))?;
    Ok(Request::atomic(vec![Statement::new(format!(
        "INSERT OR REPLACE INTO jsonld_contexts(iri, doc) VALUES ({}, {})",
        sql_str(iri),
        sql_str(json)
    ))]))
}

/// Removes a persisted context.
pub fn remove_context_request(iri: &str) -> Request {
    Request::atomic(vec![Statement::new(format!(
        "DELETE FROM jsonld_contexts WHERE iri = {}",
        sql_str(iri)
    ))])
}

/// IRIs of the persisted contexts.
pub fn list_contexts_job() -> impl Job<Output = Vec<String>> {
    oxilite_core::job::OneShot::new(
        Request::read(vec!["SELECT iri FROM jsonld_contexts ORDER BY iri".into()]),
        |r| {
            Ok(r.into_iter()
                .next()
                .unwrap_or_default()
                .rows
                .into_iter()
                .filter_map(|row| row.into_iter().next()?.into_string())
                .collect())
        },
    )
}

// ---------------------------------------------------------------------------------------
// Rebuild and check

/// Regenerates a document's graphs from its stored JSON, keeping its metadata.
pub struct RebuildJob<S> {
    key: String,
    state: Option<(JsonLdOptions, S, Capabilities)>,
    inner: Option<WriteJob<S>>,
}

impl<S: Loader> RebuildJob<S> {
    pub fn new(
        key: impl Into<String>,
        options: JsonLdOptions,
        first: S,
        caps: Capabilities,
    ) -> Self {
        Self {
            key: key.into(),
            state: Some((options, first, caps)),
            inner: None,
        }
    }

    /// [`Job::step`] with this crate's error type; `Done(None)` when the key is unknown.
    pub fn step_jsonld(
        &mut self,
        response: Option<Response>,
    ) -> Result<Step<Option<WriteOutcome>>> {
        if let Some(inner) = &mut self.inner {
            return Ok(match inner.step_jsonld(response)? {
                Step::Execute(r) => Step::Execute(r),
                Step::Done(o) => Step::Done(Some(o)),
            });
        }
        let Some(response) = response else {
            return Ok(Step::Execute(Request::read(vec![Statement::new(format!(
                "SELECT {DOC_COLUMNS} FROM {DOC_FROM} WHERE d.key = {}",
                sql_str(&self.key)
            ))])));
        };
        let rows = response.into_iter().next().unwrap_or_default().rows;
        let Some(row) = rows.first() else {
            return Ok(Step::Done(None));
        };
        let doc = decode_document(row)?;
        let (options, first, caps) = self.state.take().expect("rebuild started twice");
        let input = DocumentInput {
            json: doc.json,
            key: Some(doc.key),
            meta: doc.meta,
        };
        let mut inner = WriteJob::new(vec![input], Vec::new(), options, first, caps)?;
        let step = inner.step_jsonld(None)?;
        self.inner = Some(inner);
        Ok(match step {
            Step::Execute(r) => Step::Execute(r),
            Step::Done(o) => Step::Done(Some(o)),
        })
    }
}

impl<S: Loader> Job for RebuildJob<S> {
    type Output = Option<WriteOutcome>;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<Self::Output>> {
        self.step_jsonld(response).map_err(into_core)
    }
}

/// Compares every document's graphs with a fresh conversion of its stored JSON.
///
/// One read per page of documents, then one read per document; persisted contexts are
/// read once, up front.
pub struct CheckJob<S> {
    options: JsonLdOptions,
    first: S,
    known: BTreeMap<String, String>,
    after: Option<String>,
    queue: Vec<(String, String, i64, Vec<i64>)>,
    current: Option<Pending>,
    drifts: Vec<Drift>,
    phase: CheckPhase,
}

struct Pending {
    key: String,
    /// Expected quads in the document's own graphs.
    own: BTreeSet<[i64; 4]>,
    /// Expected quads in shared graphs (only presence is checked).
    shared: usize,
    has_own: bool,
}

enum CheckPhase {
    Contexts,
    Page,
    Quads,
}

const CHECK_PAGE: usize = 100;

impl<S: Loader> CheckJob<S> {
    pub fn new(options: JsonLdOptions, first: S) -> Self {
        let known = options.contexts.clone();
        Self {
            options,
            first,
            known,
            after: None,
            queue: Vec::new(),
            current: None,
            drifts: Vec::new(),
            phase: CheckPhase::Contexts,
        }
    }

    fn page_request(&self) -> Request {
        let mut sql = String::from(
            "SELECT d.key, d.doc, CAST(d.graph AS TEXT), (SELECT group_concat(CAST(g AS TEXT)) FROM jsonld_graphs WHERE key = d.key) FROM jsonld_documents d",
        );
        if let Some(a) = &self.after {
            let _ = write!(sql, " WHERE d.key > {}", sql_str(a));
        }
        let _ = write!(sql, " ORDER BY d.key LIMIT {CHECK_PAGE}");
        Request::read(vec![Statement::new(sql)])
    }

    /// Converts the next queued document and asks for its stored quads.
    fn next(&mut self) -> Result<Step<Vec<Drift>>> {
        while let Some((key, json, target_id, owned)) = self.queue.pop() {
            let expected: BTreeSet<[i64; 4]> = match (
                json_syntax::Value::parse_str(&json),
                self.options.graph_for(&key),
            ) {
                (Ok((doc, _)), Ok(target)) if graph_id(target.as_ref()) == target_id => {
                    match convert(&doc, &key, &target, &self.options, &self.known, &self.first)? {
                        Attempt::Done(c) => {
                            let mut rows = EncodedRows::default();
                            c.quads.iter().map(|q| rows.quad(q.as_ref())).collect()
                        }
                        Attempt::Missing(m) => {
                            return Err(JsonLdError::ContextNotFound(
                                m.into_iter().next().unwrap_or_default(),
                            ))
                        }
                    }
                }
                _ => {
                    // Not reproducible with these options: report it whole.
                    self.drifts.push(Drift {
                        key,
                        missing: 0,
                        extra: 0,
                    });
                    continue;
                }
            };
            let mut stmts = Vec::new();
            if !owned.is_empty() {
                let list = owned
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                stmts.push(Statement::new(format!(
                    "SELECT CAST(s AS TEXT), CAST(p AS TEXT), CAST(o AS TEXT), CAST(g AS TEXT) FROM quads WHERE g IN ({list})"
                )));
            }
            let (own, shared): (BTreeSet<[i64; 4]>, BTreeSet<[i64; 4]>) =
                expected.into_iter().partition(|q| owned.contains(&q[3]));
            if !shared.is_empty() {
                let values = shared
                    .iter()
                    .map(|[s, p, o, g]| format!("({s},{p},{o},{g})"))
                    .collect::<Vec<_>>()
                    .join(",");
                stmts.push(Statement::new(format!(
                    "SELECT count(*) FROM quads WHERE (s, p, o, g) IN (VALUES {values})"
                )));
            }
            if stmts.is_empty() {
                continue;
            }
            self.current = Some(Pending {
                key,
                own,
                shared: shared.len(),
                has_own: !owned.is_empty(),
            });
            self.phase = CheckPhase::Quads;
            return Ok(Step::Execute(Request::read(stmts)));
        }
        match self.after {
            Some(_) => {
                self.phase = CheckPhase::Page;
                Ok(Step::Execute(self.page_request()))
            }
            None => Ok(Step::Done(std::mem::take(&mut self.drifts))),
        }
    }

    /// [`Job::step`] with this crate's error type.
    pub fn step_jsonld(&mut self, response: Option<Response>) -> Result<Step<Vec<Drift>>> {
        let Some(response) = response else {
            return Ok(Step::Execute(Request::read(vec![
                "SELECT iri, doc FROM jsonld_contexts".into(),
            ])));
        };
        match self.phase {
            CheckPhase::Contexts => {
                for row in response.into_iter().next().unwrap_or_default().rows {
                    if let (Some(i), Some(d)) = (row[0].as_str(), row[1].as_str()) {
                        self.known
                            .entry(i.to_owned())
                            .or_insert_with(|| d.to_owned());
                    }
                }
                self.phase = CheckPhase::Page;
                Ok(Step::Execute(self.page_request()))
            }
            CheckPhase::Page => {
                let rows = response.into_iter().next().unwrap_or_default().rows;
                self.after = if rows.len() == CHECK_PAGE {
                    rows.last().and_then(|r| r[0].as_str().map(str::to_owned))
                } else {
                    None
                };
                for row in rows.into_iter().rev() {
                    let key = row[0].as_str().unwrap_or_default().to_owned();
                    let json = row[1].as_str().unwrap_or_default().to_owned();
                    let target = row[2].as_i64().unwrap_or_default();
                    let owned = row[3]
                        .as_str()
                        .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
                        .unwrap_or_default();
                    self.queue.push((key, json, target, owned));
                }
                self.next()
            }
            CheckPhase::Quads => {
                let p = self.current.take().expect("current document");
                let mut sets = response.into_iter();
                let (mut missing, mut extra) = (0, 0);
                if p.has_own {
                    let actual: BTreeSet<[i64; 4]> = sets
                        .next()
                        .unwrap_or_default()
                        .rows
                        .iter()
                        .filter_map(|r| {
                            Some([
                                r[0].as_i64()?,
                                r[1].as_i64()?,
                                r[2].as_i64()?,
                                r[3].as_i64()?,
                            ])
                        })
                        .collect();
                    missing += p.own.difference(&actual).count();
                    extra += actual.difference(&p.own).count();
                }
                if p.shared > 0 {
                    let found = sets
                        .next()
                        .unwrap_or_default()
                        .rows
                        .first()
                        .and_then(|r| r.first())
                        .and_then(SqlValue::as_i64)
                        .unwrap_or_default() as usize;
                    missing += p.shared.saturating_sub(found);
                }
                if missing > 0 || extra > 0 {
                    self.drifts.push(Drift {
                        key: p.key,
                        missing,
                        extra,
                    });
                }
                self.next()
            }
        }
    }
}

impl<S: Loader> Job for CheckJob<S> {
    type Output = Vec<Drift>;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<Vec<Drift>>> {
        self.step_jsonld(response).map_err(into_core)
    }
}

/// Quads of a document as the store would hold them (for tests and diagnostics).
pub fn document_quads<S: Loader>(
    json: &str,
    key: &str,
    options: &JsonLdOptions,
    first: &S,
) -> Result<Vec<Quad>> {
    let (doc, _) =
        json_syntax::Value::parse_str(json).map_err(|e| JsonLdError::Json(e.to_string()))?;
    let target = options.graph_for(key)?;
    match convert(&doc, key, &target, options, &options.contexts, first)? {
        Attempt::Done(c) => Ok(c.quads),
        Attempt::Missing(m) => Err(JsonLdError::ContextNotFound(
            m.into_iter().next().unwrap_or_default(),
        )),
    }
}
