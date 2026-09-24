//! SHACL validation of the Project store as a background job: rudof validates asserted (and,
//! by default, inferred) triples, and each result becomes a diagnostic on the source line of the
//! statement it is about.
//!
// @lat: [[architecture#Studio server#Live validation]]

use super::index::SourceIndex;
use super::scanner::Pos;
use lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Position, Range, Uri,
};
use oxilite::model::GraphName;
use oxilite::sparql::{QueryOptions, SparqlParser};
use oxilite::store::Store;
use oxilite_core::QueryOutput;
use oxilite_validate::{shacl_schema, validate_shacl_graph, ShaclValidationMode, StoreGraph};
use rudof_rdf::rdf_core::term::Object;
use rudof_rdf::rdf_core::{RDFFormat, SHACLPath};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Everything a validation run needs, owned so it can run on another thread.
pub struct Job {
    pub store: Store,
    pub options: QueryOptions,
    pub shapes: String,
    /// The shapes files, where a result's shape is looked for when it is a blank node.
    pub shape_files: Vec<String>,
    /// ShEx schemas with their shape maps: (URI, ShExC, shape map).
    pub shex: Vec<(String, String, String)>,
    pub index: Arc<SourceIndex>,
    /// Validate the entailed graph rather than asserted triples only.
    pub inferred: bool,
    pub generation: u64,
    /// The project's current generation: a run that falls behind stops early.
    pub latest: Arc<AtomicU64>,
}

pub struct Outcome {
    pub generation: u64,
    pub diagnostics: HashMap<String, Vec<Diagnostic>>,
    pub report: Value,
}

impl Job {
    fn stale(&self) -> bool {
        self.latest.load(Ordering::SeqCst) != self.generation
    }

    pub fn run(self) -> Option<Outcome> {
        match self.validate() {
            Ok(Some(o)) => Some(o),
            Ok(None) => None,
            Err(e) => Some(Outcome {
                generation: self.generation,
                diagnostics: HashMap::new(),
                report: json!({"conforms": null, "error": e.to_string(), "results": []}),
            }),
        }
    }

    fn validate(&self) -> Result<Option<Outcome>> {
        let entailing = entailing(&self.options);
        let report = if self.shapes.trim().is_empty() {
            None
        } else {
            let Some(r) = report(
                &self.store,
                &self.options,
                &self.shapes,
                &RDFFormat::NTriples,
                self.inferred,
                &|| self.stale(),
            )?
            else {
                return Ok(None);
            };
            Some(r)
        };
        let mut diagnostics: HashMap<String, Vec<Diagnostic>> = HashMap::new();
        let mut results = Vec::new();
        let owners = owners(&self.shapes);
        let mut conforms = true;
        for r in report.iter().flat_map(|r| r.results()) {
            conforms = false;
            let focus = object_iri(r.focus_node());
            let path = r.path().and_then(path_predicate);
            let shape = shape_of(r.source(), &owners);
            let severity = severity(&r.severity().to_string());
            let component = local(&r.constraint_component().to_string());
            let message = r
                .message()
                .iter()
                .next()
                .map(|(_, m)| m.clone())
                .unwrap_or_else(|| {
                    format!(
                        "{component} violated{}",
                        path.as_ref()
                            .map(|p| format!(" on <{p}>"))
                            .unwrap_or_default()
                    )
                });
            let focus_text = r.focus_node().to_string();
            let at = focus
                .as_ref()
                .and_then(|f| self.index.triple_location(f, path.as_deref()));
            // A named shape is found where it is defined; a blank property shape where its
            // path is mentioned in a shapes file.
            let shape_at = shape
                .as_ref()
                .and_then(|s| self.index.definitions(s).into_iter().next())
                .or_else(|| {
                    let p = path.as_ref()?;
                    self.index
                        .references(p)
                        .into_iter()
                        .find(|l| self.shape_files.contains(&l.uri))
                });
            results.push(json!({
                "focus": focus_text,
                "path": path,
                "value": r.value().map(ToString::to_string),
                "severity": severity_name(severity),
                "component": component,
                "message": message,
                "shape": shape,
                "location": at.as_ref().map(location_json),
                "shapeLocation": shape_at.as_ref().map(location_json),
            }));
            // Report on the data line; without one, on the shape (naming the focus node).
            let (target, text) = match (&at, &shape_at) {
                (Some(l), _) => (l.clone(), message.clone()),
                (None, Some(l)) => (l.clone(), format!("{focus_text}: {message}")),
                (None, None) => continue,
            };
            let related = shape_at.as_ref().filter(|_| at.is_some()).and_then(|l| {
                Some(vec![DiagnosticRelatedInformation {
                    location: Location::new(Uri::from_str(&l.uri).ok()?, range(l.start, l.end)),
                    message: match &shape {
                        Some(s) => format!("shape <{s}>"),
                        None => "the property shape".into(),
                    },
                }])
            });
            diagnostics
                .entry(target.uri.clone())
                .or_default()
                .push(Diagnostic {
                    range: range(target.start, target.end),
                    severity: Some(severity),
                    code: Some(lsp_types::NumberOrString::String(component.clone())),
                    source: Some("shacl".into()),
                    message: text,
                    related_information: related,
                    ..Diagnostic::default()
                });
        }
        // ShEx: each nonconformant node of a shape map is a result on its statement.
        for (uri, schema, map) in &self.shex {
            if self.stale() {
                return Ok(None);
            }
            let graph = data_graph(&self.store, &self.options, self.inferred)?;
            let compiled = oxilite_validate::shex_schema(schema, uri)?;
            let shapes = oxilite_validate::validate_shex_graph(&graph, &compiled, map)?;
            for (node, label, status) in shapes.iter() {
                if status.is_conformant() {
                    continue;
                }
                conforms = false;
                let focus = node
                    .to_string()
                    .trim_matches(|c| c == '<' || c == '>')
                    .to_string();
                let shape = label
                    .to_string()
                    .trim_matches(|c| c == '<' || c == '>')
                    .to_string();
                let message = format!("does not conform to ShEx shape <{shape}>: {status:?}")
                    .chars()
                    .take(600)
                    .collect::<String>();
                let at = self.index.triple_location(&focus, None);
                let shape_at = self
                    .index
                    .definitions(&shape)
                    .into_iter()
                    .find(|l| l.uri == *uri);
                results.push(json!({
                    "focus": focus, "path": null, "value": null, "severity": "violation",
                    "component": "shex", "message": message, "shape": shape,
                    "location": at.as_ref().map(location_json),
                    "shapeLocation": shape_at.as_ref().map(location_json),
                }));
                let Some(target) = at.or(shape_at.clone()) else {
                    continue;
                };
                diagnostics
                    .entry(target.uri.clone())
                    .or_default()
                    .push(Diagnostic {
                        range: range(target.start, target.end),
                        severity: Some(DiagnosticSeverity::ERROR),
                        code: Some(lsp_types::NumberOrString::String("shex".into())),
                        source: Some("shex".into()),
                        message: message.clone(),
                        related_information: shape_at.and_then(|l| {
                            Some(vec![DiagnosticRelatedInformation {
                                location: Location::new(
                                    Uri::from_str(&l.uri).ok()?,
                                    range(l.start, l.end),
                                ),
                                message: format!("shape <{shape}>"),
                            }])
                        }),
                        ..Diagnostic::default()
                    });
            }
        }
        Ok(Some(Outcome {
            generation: self.generation,
            diagnostics,
            report: json!({
                "conforms": conforms,
                "inferred": self.inferred && entailing,
                "results": results,
            }),
        }))
    }
}

fn entailing(options: &QueryOptions) -> bool {
    options.reasoning != oxilite::sparql::Reasoning::None || options.include_inferred
}

/// The graph validators read: the store's union of graphs, or with reasoning on and
/// `inferred`, a copy of the entailed graph.
fn data_graph(
    store: &Store,
    options: &QueryOptions,
    inferred: bool,
) -> Result<StoreGraph<oxilite::rusqlite::RusqliteBackend>> {
    if inferred && entailing(options) {
        let copy = Store::new()?;
        let q = SparqlParser::new().parse_query("CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }")?;
        if let QueryOutput::Graph(triples) = store.query_output(q, options)? {
            copy.extend(
                triples
                    .into_iter()
                    .map(|t| t.in_graph(GraphName::DefaultGraph)),
            )?;
        }
        Ok(StoreGraph::new(copy))
    } else {
        Ok(StoreGraph::new(store.clone()).with_union(true))
    }
}

/// Validates a store (all graphs) against shapes. With reasoning on and `inferred`, a copy of
/// the entailed graph is validated, so a `sh:class` shape sees types that follow from the
/// ontology. `None` when `stale` says the result is no longer wanted.
pub fn report(
    store: &Store,
    options: &QueryOptions,
    shapes: &str,
    format: &RDFFormat,
    inferred: bool,
    stale: &dyn Fn() -> bool,
) -> Result<Option<oxilite_validate::ValidationReport>> {
    let schema = shacl_schema(shapes, format, None)?;
    if stale() {
        return Ok(None);
    }
    let graph = data_graph(store, options, inferred)?;
    if stale() {
        return Ok(None);
    }
    let report = validate_shacl_graph(graph, &schema, &ShaclValidationMode::Native)?;
    Ok((!stale()).then_some(report))
}

/// Blank property shapes (by label) to the named shape that owns them through `sh:property`
/// (or `sh:node`, transitively), read from shapes in N-Triples, whose labels rudof keeps.
pub fn owners(shapes_nt: &str) -> HashMap<String, String> {
    use oxilite::model::{NamedOrBlankNode, Term};
    let mut parent: HashMap<String, NamedOrBlankNode> = HashMap::new();
    for q in oxilite::io::RdfParser::from_format(oxilite::io::RdfFormat::NTriples)
        .for_slice(shapes_nt.as_bytes())
        .flatten()
    {
        let p = q.predicate.as_str();
        if p == "http://www.w3.org/ns/shacl#property" || p == "http://www.w3.org/ns/shacl#node" {
            if let Term::BlankNode(b) = &q.object {
                parent.insert(b.as_str().to_string(), q.subject.clone());
            }
        }
    }
    let mut out = HashMap::new();
    for label in parent.keys() {
        let mut at = label.clone();
        for _ in 0..16 {
            match parent.get(&at) {
                Some(NamedOrBlankNode::NamedNode(n)) => {
                    out.insert(label.clone(), n.as_str().to_string());
                    break;
                }
                Some(NamedOrBlankNode::BlankNode(b)) => at = b.as_str().to_string(),
                None => break,
            }
        }
    }
    out
}

/// The named shape a result is about: its source, or the named shape owning a blank one.
pub fn shape_of(source: Option<&Object>, owners: &HashMap<String, String>) -> Option<String> {
    match source? {
        Object::Iri(i) => Some(i.as_str().to_string()),
        Object::BlankNode(b) => owners.get(b.as_str()).cloned(),
        _ => None,
    }
}

/// The named shapes a report has results for.
pub fn failing_shapes(
    report: &oxilite_validate::ValidationReport,
    owners: &HashMap<String, String>,
) -> Vec<String> {
    let mut v: Vec<String> = report
        .results()
        .iter()
        .filter_map(|r| shape_of(r.source(), owners))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Shapes text in any RDF format as N-Triples, so blank node labels stay stable.
pub fn to_ntriples(text: &str, format: oxilite::io::RdfFormat) -> Result<String> {
    let mut out = String::new();
    for q in oxilite::io::RdfParser::from_format(format).for_slice(text.as_bytes()) {
        let q = q?;
        out.push_str(&format!("{} {} {} .\n", q.subject, q.predicate, q.object));
    }
    Ok(out)
}

fn range(start: Pos, end: Pos) -> Range {
    Range::new(
        Position::new(start.line, start.col),
        Position::new(end.line, end.col),
    )
}

fn location_json(l: &super::index::Location) -> Value {
    json!({
        "uri": l.uri,
        "range": {"start": {"line": l.start.line, "character": l.start.col}, "end": {"line": l.end.line, "character": l.end.col}},
    })
}

fn object_iri(o: &Object) -> Option<String> {
    match o {
        Object::Iri(i) => Some(i.as_str().to_string()),
        _ => None,
    }
}

fn path_predicate(p: &SHACLPath) -> Option<String> {
    match p {
        SHACLPath::Predicate { pred } => Some(pred.as_str().to_string()),
        _ => None,
    }
}

fn severity(s: &str) -> DiagnosticSeverity {
    let s = s.to_ascii_lowercase();
    if s.contains("warning") {
        DiagnosticSeverity::WARNING
    } else if s.contains("info") {
        DiagnosticSeverity::INFORMATION
    } else {
        DiagnosticSeverity::ERROR
    }
}

fn severity_name(s: DiagnosticSeverity) -> &'static str {
    match s {
        DiagnosticSeverity::WARNING => "warning",
        DiagnosticSeverity::INFORMATION => "info",
        _ => "violation",
    }
}

/// `sh:MinCountConstraintComponent` → `sh:minCount`.
fn local(component: &str) -> String {
    let name = component
        .trim_matches(|c| c == '<' || c == '>')
        .rsplit(['#', '/', ':'])
        .next()
        .unwrap_or(component);
    let name = name.strip_suffix("ConstraintComponent").unwrap_or(name);
    let mut chars = name.chars();
    match chars.next() {
        Some(c) => format!("sh:{}{}", c.to_ascii_lowercase(), chars.as_str()),
        None => component.to_string(),
    }
}
