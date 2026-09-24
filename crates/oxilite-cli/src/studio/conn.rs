//! Connections: the Project store and attached stores behind one interface, plus the vocabulary
//! (predicates, classes, labels) each one offers to completion and hover.
//!
// @lat: [[architecture#Studio server#Connections]]

use super::d1::{D1Http, Handle};
use oxilite::model::{NamedNode, Term};
use oxilite::sparql::{QueryOptions, SparqlParser};
use oxilite::store::Store;
use oxilite_core::QueryOutput;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A store the user attached: its updates persist.
pub struct Attached {
    pub id: String,
    /// `sqlite`, `d1-local` (a `wrangler dev` database file) or `d1`.
    pub kind: &'static str,
    pub label: String,
    pub path: String,
    pub read_only: bool,
    pub store: Handle,
    pub union_default_graph: bool,
}

impl Attached {
    pub fn open(path: &str, read_only: bool) -> Result<Self> {
        if !std::path::Path::new(path).exists() && read_only {
            return Err(format!("{path} does not exist").into());
        }
        let store = if read_only {
            Store::open_read_only(path)?
        } else {
            Store::open(path)?
        };
        let local_d1 = path.contains(".wrangler/state") && path.contains("/d1/");
        Ok(Self {
            id: format!("attached:{path}"),
            kind: if local_d1 { "d1-local" } else { "sqlite" },
            label: if local_d1 {
                "D1 (wrangler dev)".into()
            } else {
                path.rsplit('/').next().unwrap_or(path).to_string()
            },
            path: path.to_string(),
            read_only,
            store: Handle::Sqlite(store),
            union_default_graph: false,
        })
    }

    /// A D1 database over the Cloudflare API (or `endpoint`, for tests and proxies).
    pub fn d1(
        account: &str,
        database: &str,
        token: &str,
        read_only: bool,
        endpoint: Option<&str>,
    ) -> Result<Self> {
        let backend = match endpoint {
            Some(e) => D1Http::with_endpoint(e, token),
            None => D1Http::new(account, database, token),
        };
        let meter = std::sync::Arc::clone(&backend.meter);
        let store = Store::with_backend(backend)?;
        Ok(Self {
            id: format!("d1:{account}/{database}"),
            kind: "d1",
            label: format!("D1 {}", &database[..database.len().min(8)]),
            path: format!("d1://{account}/{database}"),
            read_only,
            store: Handle::D1(store, meter),
            union_default_graph: false,
        })
    }
}

/// What the views need from any connection. It owns a handle on the store (a cheap clone), so
/// a read can run on another thread.
#[derive(Clone)]
pub struct Target {
    pub store: Handle,
    pub options: QueryOptions,
    /// Updates are lost on the next reload (Project store).
    pub ephemeral: bool,
    pub read_only: bool,
}

impl Target {
    /// Runs a query, or an update when the text is one. Updates on a persistent store need
    /// `confirmed`; the error code tells the client to ask.
    pub fn run(&self, text: &str, limit: usize, confirmed: bool) -> Result<Value> {
        let start = Instant::now();
        let metered = self.store.meter().map(|m| m.snapshot());
        let query_error = match SparqlParser::new().parse_query(text) {
            Ok(q) => {
                let mut out = self.store.query_output(q, &self.options)?;
                let truncated = match &mut out {
                    QueryOutput::Solutions { rows, .. } => cap(rows, limit),
                    QueryOutput::Graph(triples) => cap(triples, limit),
                    QueryOutput::Boolean(_) => false,
                };
                let mut payload = oxilite_core::json::output_to_json(&out);
                payload["elapsedMs"] = json!(elapsed(start));
                payload["truncated"] = json!(truncated);
                self.add_billing(&mut payload, metered);
                return Ok(payload);
            }
            Err(e) => e,
        };
        let Ok(update) = SparqlParser::new().parse_update(text) else {
            return Err(query_error.into());
        };
        if self.read_only {
            return Err("this connection is read-only".into());
        }
        if !self.ephemeral && !confirmed {
            return Err(Box::new(NeedsConfirmation {
                estimate: self.store.meter().map(|_| write_estimate(&update)),
            }));
        }
        let metered = self.store.meter().map(|m| m.snapshot());
        let before = self.store.len()?;
        self.store.update(update)?;
        let after = self.store.len()?;
        let mut out = json!({
            "kind": "update",
            "elapsedMs": elapsed(start),
            "truncated": false,
            "ephemeral": self.ephemeral,
            "delta": after as i64 - before as i64,
        });
        self.add_billing(&mut out, metered);
        Ok(out)
    }

    /// On D1, what the request cost: requests made, rows read and written.
    fn add_billing(&self, out: &mut Value, before: Option<(u64, u64, u64)>) {
        if let (Some(m), Some((r0, read0, w0))) = (self.store.meter(), before) {
            let (r, read, w) = m.snapshot();
            out["d1"] =
                json!({"requests": r - r0, "rowsRead": read - read0, "rowsWritten": w - w0});
        }
    }

    /// Materializes the OWL 2 RL closure; on a persistent store it needs `confirmed`.
    pub fn materialize(&self, confirmed: bool) -> Result<Value> {
        if self.read_only {
            return Err("this connection is read-only".into());
        }
        if !self.ephemeral && !confirmed {
            return Err(Box::new(NeedsConfirmation {
                estimate: self.store.meter().map(|_| {
                    "OWL 2 RL materialization writes every inferred triple (about 5 billed rows each) \
                     over several rounds, one request each, and is not atomic across rounds"
                        .to_string()
                }),
            }));
        }
        let start = Instant::now();
        let metered = self.store.meter().map(|m| m.snapshot());
        let inferred = self.store.materialize()?;
        let mut out = json!({"inferred": inferred, "elapsedMs": elapsed(start)});
        self.add_billing(&mut out, metered);
        Ok(out)
    }

    pub fn explain(&self, text: &str) -> Result<Value> {
        let q = SparqlParser::new().parse_query(text)?;
        let plan = self.store.explain_opt(q, &self.options)?;
        Ok(json!({ "kind": "sparql", "text": plan }))
    }

    pub fn vocab(&self) -> Result<Vocab> {
        Vocab::compute(&self.store, &self.options)
    }

    /// Labels, comments, types and statement counts for hover and the resource view.
    pub fn describe(&self, iri: &str, limit: usize) -> Result<Value> {
        let node = NamedNode::new(iri)?;
        let q = |text: String| -> Result<Vec<Vec<Option<Term>>>> {
            match self
                .store
                .query_output(SparqlParser::new().parse_query(&text)?, &self.options)?
            {
                QueryOutput::Solutions { rows, .. } => Ok(rows),
                _ => Ok(Vec::new()),
            }
        };
        let out = q(format!(
            "SELECT ?p ?o WHERE {{ {node} ?p ?o }} LIMIT {}",
            limit + 1
        ))?;
        let inc = q(format!(
            "SELECT ?s ?p WHERE {{ ?s ?p {node} }} LIMIT {}",
            limit + 1
        ))?;
        // Rows the asserted data alone does not give are inferred.
        let entailing = self.options.reasoning != oxilite::sparql::Reasoning::None
            || self.options.include_inferred;
        let asserted = |text: String| -> Result<std::collections::HashSet<Vec<Option<Term>>>> {
            if !entailing {
                return Ok(Default::default());
            }
            let plain = QueryOptions {
                reasoning: oxilite::sparql::Reasoning::None,
                include_inferred: false,
                ..self.options.clone()
            };
            match self
                .store
                .query_output(SparqlParser::new().parse_query(&text)?, &plain)?
            {
                QueryOutput::Solutions { rows, .. } => Ok(rows.into_iter().collect()),
                _ => Ok(Default::default()),
            }
        };
        let out_asserted = asserted(format!("SELECT ?p ?o WHERE {{ {node} ?p ?o }}"))?;
        let inc_asserted = asserted(format!("SELECT ?s ?p WHERE {{ ?s ?p {node} }}"))?;
        let producers = |s: &Option<Term>, p: &Option<Term>, o: &Option<Term>| -> Value {
            let quad = (|| {
                let s = match s.clone()? {
                    Term::NamedNode(n) => oxilite::model::NamedOrBlankNode::from(n),
                    Term::BlankNode(b) => b.into(),
                    _ => return None,
                };
                let Term::NamedNode(p) = p.clone()? else {
                    return None;
                };
                Some(oxilite::model::Quad::new(
                    s,
                    p,
                    o.clone()?,
                    oxilite::model::GraphName::DefaultGraph,
                ))
            })();
            quad.and_then(|q| self.store.inference_producers(&q).ok())
                .map_or(Value::Null, |v| json!(v))
        };
        let term = |t: &Option<Term>| {
            t.as_ref()
                .map_or(Value::Null, oxilite_core::json::term_to_json)
        };
        let subject = Some(Term::NamedNode(node.clone()));
        Ok(json!({
            "iri": iri,
            "outgoing": out.iter().take(limit).map(|r| {
                let inferred = entailing && !out_asserted.contains(r);
                json!({"p": term(&r[0]), "o": term(&r[1]), "inferred": inferred,
                       "producers": if inferred { producers(&subject, &r[0], &r[1]) } else { Value::Null }})
            }).collect::<Vec<_>>(),
            "incoming": inc.iter().take(limit).map(|r| {
                let inferred = entailing && !inc_asserted.contains(r);
                json!({"s": term(&r[0]), "p": term(&r[1]), "inferred": inferred,
                       "producers": if inferred { producers(&r[0], &r[1], &subject) } else { Value::Null }})
            }).collect::<Vec<_>>(),
            "truncated": out.len() > limit || inc.len() > limit,
        }))
    }
}

/// The error an update on a persistent store returns until the user confirms it, with what
/// it will cost when the store bills writes (D1).
#[derive(Debug)]
pub struct NeedsConfirmation {
    pub estimate: Option<String>,
}

/// D1 writes about 4.8 rows per quad (the quad and its index entries, see the write-cost
/// benchmark); only `INSERT DATA` and `DELETE DATA` know their size in advance.
fn write_estimate(update: &oxilite::sparql::Update) -> String {
    use spargebra::GraphUpdateOperation;
    let mut quads = 0usize;
    let mut known = true;
    for op in &update.operations {
        match op {
            GraphUpdateOperation::InsertData { data } => quads += data.len(),
            GraphUpdateOperation::DeleteData { data } => quads += data.len(),
            _ => known = false,
        }
    }
    if known {
        format!(
            "about {} billed rows written ({quads} quads)",
            (quads as f64 * 4.8).ceil() as u64
        )
    } else {
        "the number of rows written depends on the data the update matches".into()
    }
}

impl std::fmt::Display for NeedsConfirmation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this update changes a persistent store and needs confirmation")
    }
}

impl std::error::Error for NeedsConfirmation {}

fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn cap<T>(v: &mut Vec<T>, limit: usize) -> bool {
    let over = v.len() > limit;
    v.truncate(limit);
    over
}

const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

/// The terms a store actually uses, most frequent first.
#[derive(Debug, Default, Clone)]
pub struct Vocab {
    pub predicates: Vec<(String, u64)>,
    pub classes: Vec<(String, u64)>,
    pub labels: HashMap<String, String>,
    pub comments: HashMap<String, String>,
}

impl Vocab {
    pub fn compute(store: &Handle, options: &QueryOptions) -> Result<Self> {
        let rows = |text: &str| -> Result<Vec<Vec<Option<Term>>>> {
            match store.query_output(SparqlParser::new().parse_query(text)?, options)? {
                QueryOutput::Solutions { rows, .. } => Ok(rows),
                _ => Ok(Vec::new()),
            }
        };
        let counted = |rows: Vec<Vec<Option<Term>>>| -> Vec<(String, u64)> {
            rows.into_iter()
                .filter_map(|r| match (&r[0], r.get(1).cloned().flatten()) {
                    (Some(Term::NamedNode(n)), Some(Term::Literal(c))) => {
                        Some((n.as_str().to_string(), c.value().parse().unwrap_or(0)))
                    }
                    (Some(Term::NamedNode(n)), None) => Some((n.as_str().to_string(), 0)),
                    _ => None,
                })
                .collect()
        };
        let predicates = counted(rows(
            "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 5000",
        )?);
        let mut classes = counted(rows(
            "SELECT ?c (COUNT(?s) AS ?n) WHERE { ?s a ?c } GROUP BY ?c ORDER BY DESC(?n) LIMIT 5000",
        )?);
        // Declared classes without instances still complete after `a`.
        let declared = counted(rows(
            "PREFIX owl: <http://www.w3.org/2002/07/owl#> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
             SELECT DISTINCT ?c WHERE { { ?c a owl:Class } UNION { ?c a rdfs:Class } UNION { ?c rdfs:subClassOf ?x } UNION { ?x rdfs:subClassOf ?c } } LIMIT 5000",
        )?);
        for (c, _) in declared {
            if !classes.iter().any(|(k, _)| *k == c) {
                classes.push((c, 0));
            }
        }
        let mut labels = HashMap::new();
        let mut comments = HashMap::new();
        for r in rows(
            "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
             SELECT ?x ?p ?l WHERE { VALUES ?p { rdfs:label skos:prefLabel rdfs:comment skos:definition } ?x ?p ?l FILTER(isIRI(?x) && (lang(?l) = \"\" || langMatches(lang(?l), \"en\"))) } LIMIT 50000",
        )? {
            if let (Some(Term::NamedNode(x)), Some(Term::NamedNode(p)), Some(Term::Literal(l))) =
                (&r[0], &r[1], &r[2])
            {
                let target = if p.as_str() == RDFS_LABEL || p.as_str().ends_with("prefLabel") {
                    &mut labels
                } else {
                    &mut comments
                };
                target
                    .entry(x.as_str().to_string())
                    .or_insert_with(|| l.value().to_string());
            }
        }
        Ok(Self {
            predicates,
            classes,
            labels,
            comments,
        })
    }

    pub fn predicate_count(&self, iri: &str) -> Option<u64> {
        self.predicates
            .iter()
            .find(|(p, _)| p == iri)
            .map(|(_, n)| *n)
    }

    pub fn class_count(&self, iri: &str) -> Option<u64> {
        self.classes.iter().find(|(c, _)| c == iri).map(|(_, n)| *n)
    }
}

/// A Cypher statement that writes (so it needs confirmation on a persistent store).
pub fn cypher_writes(text: &str) -> bool {
    regex::Regex::new(r"(?i)\b(CREATE|MERGE|SET|DELETE|DETACH|REMOVE)\b")
        .expect("valid regex")
        .is_match(text)
}

impl Target {
    fn datalog_options(&self) -> oxilite::datalog::Options {
        oxilite::datalog::Options {
            union_default_graph: self.options.union_default_graph,
            include_inferred: self.options.include_inferred,
            ..Default::default()
        }
    }

    /// Runs a Datalog program's goal; the payload is a SPARQL-like solutions table.
    pub fn datalog(&self, program: &str, limit: usize) -> Result<Value> {
        let start = Instant::now();
        let r = self.store.datalog_with(program, &self.datalog_options())?;
        let truncated = r.rows.len() > limit;
        let term = |t: &Option<Term>| {
            t.as_ref()
                .map_or(Value::Null, oxilite_core::json::term_to_json)
        };
        Ok(json!({
            "kind": "solutions",
            "variables": r.variables,
            "rows": r.rows.iter().take(limit).map(|row| row.iter().map(term).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "elapsedMs": elapsed(start),
            "truncated": truncated,
        }))
    }

    pub fn explain_datalog(&self, program: &str) -> Result<Value> {
        Ok(json!({"kind": "datalog", "text": self.store.explain_datalog(program)?}))
    }

    /// Runs a Cypher statement; writes on a persistent store need `confirmed`.
    pub fn cypher(
        &self,
        text: &str,
        limit: usize,
        confirmed: bool,
        options: &oxilite::cypher::CypherOptions,
    ) -> Result<Value> {
        let writes = cypher_writes(text);
        if writes && self.read_only {
            return Err("this connection is read-only".into());
        }
        if writes && !self.ephemeral && !confirmed {
            return Err(Box::new(NeedsConfirmation { estimate: None }));
        }
        let start = Instant::now();
        let metered = self.store.meter().map(|m| m.snapshot());
        let r = self
            .store
            .cypher_with(text, &oxilite::cypher::Params::new(), options)?;
        let mut v = r.to_json();
        let rows = v["rows"].as_array().cloned().unwrap_or_default();
        v["rows"] = json!(rows.iter().take(limit).collect::<Vec<_>>());
        v["kind"] = json!("cypher");
        v["elapsedMs"] = json!(elapsed(start));
        v["truncated"] = json!(rows.len() > limit);
        v["ephemeral"] = json!(self.ephemeral);
        v["writes"] = json!(writes);
        self.add_billing(&mut v, metered);
        Ok(v)
    }

    pub fn explain_cypher(
        &self,
        text: &str,
        options: &oxilite::cypher::CypherOptions,
    ) -> Result<Value> {
        Ok(json!({
            "kind": "cypher",
            "text": self.store.explain_cypher(text, &oxilite::cypher::Params::new(), options)?,
        }))
    }

    /// Loads an RDF file (format from its extension) into `graph` or its default graph.
    pub fn import(&self, path: &str, graph: Option<&str>, confirmed: bool) -> Result<Value> {
        if self.read_only {
            return Err("this connection is read-only".into());
        }
        if !self.ephemeral && !confirmed {
            return Err(Box::new(NeedsConfirmation {
                estimate: self
                    .store
                    .meter()
                    .map(|_| "about 5 billed rows written per imported triple".to_string()),
            }));
        }
        let format = super::project::format_of(std::path::Path::new(path))
            .or_else(|| {
                std::path::Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .and_then(oxilite::io::RdfFormat::from_extension)
            })
            .ok_or_else(|| format!("unknown RDF format of {path}"))?;
        let data = std::fs::read(path)?;
        let mut parser = oxilite::io::RdfParser::from_format(format);
        if let Some(g) = graph {
            parser = parser.with_default_graph(NamedNode::new(g)?);
        }
        let before = self.store.len()?;
        self.store.load(parser, &data)?;
        Ok(json!({"added": self.store.len()? as i64 - before as i64, "ephemeral": self.ephemeral}))
    }

    /// Writes the store (a dataset format) or one graph (any format) to a file.
    pub fn export(&self, path: &str, graph: Option<&str>) -> Result<Value> {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("nq");
        let format = oxilite::io::RdfFormat::from_extension(ext)
            .ok_or_else(|| format!("unknown RDF format .{ext}"))?;
        if graph.is_none() && !format.supports_datasets() {
            return Err(format!(
                "{} holds one graph: pick a graph, or export as .nq or .trig",
                format.name()
            )
            .into());
        }
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let g = graph.map(NamedNode::new).transpose()?;
        self.store
            .dump(format, g.as_ref().map(|g| g.as_ref().into()), file)?;
        Ok(json!({"path": path}))
    }
}
