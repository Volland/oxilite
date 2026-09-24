//! Knowledge-graph tests declared in `oxilite.toml`: query results against an expected file,
//! SHACL conformance or expected violations, and entailments. Run by `oxilite check` and by
//! the server for the editor's Test Explorer.
//!
// @lat: [[architecture#Studio server#Knowledge-graph tests]]

use super::manifest::{Profile, Role, TestSpec};
use super::project::{format_of, Project};
use super::validate;
use oxilite::io::{RdfFormat, RdfParser};
use oxilite::model::{NamedNode, Term, Triple, Variable};
use oxilite::sparql::results::{
    QueryResultsFormat, QueryResultsParser, SliceQueryResultsParserOutput,
};
use oxilite::sparql::{QueryOptions, Reasoning, SparqlParser};
use oxilite::store::Store;
use oxilite_core::QueryOutput;
use rudof_rdf::rdf_core::RDFFormat;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct Outcome {
    pub name: String,
    pub passed: bool,
    pub message: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub millis: f64,
}

impl Outcome {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "passed": self.passed,
            "message": self.message,
            "expected": self.expected,
            "actual": self.actual,
            "millis": self.millis,
        })
    }
}

/// What a test runs against: the project store, or an overlay built for it.
struct Env {
    store: Store,
    options: QueryOptions,
}

pub struct Runner<'a> {
    pub project: &'a Project,
}

impl Runner<'_> {
    fn root(&self) -> PathBuf {
        self.project
            .root()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    fn path(&self, p: &str) -> PathBuf {
        self.root().join(p)
    }

    pub fn tests(&self) -> Vec<TestSpec> {
        self.project
            .manifest()
            .map(|m| m.tests.clone())
            .unwrap_or_default()
    }

    pub fn run_all(&self) -> Vec<Outcome> {
        self.tests().iter().map(|t| self.run(t)).collect()
    }

    pub fn run(&self, spec: &TestSpec) -> Outcome {
        let start = Instant::now();
        let mut out = match self.run_inner(spec) {
            Ok(o) => o,
            Err(e) => Outcome {
                name: spec.name.clone(),
                passed: false,
                message: format!("error: {e}"),
                expected: None,
                actual: None,
                millis: 0.0,
            },
        };
        out.name = spec.name.clone();
        out.millis = start.elapsed().as_secs_f64() * 1000.0;
        out
    }

    fn outcome(passed: bool, message: impl Into<String>) -> Outcome {
        Outcome {
            name: String::new(),
            passed,
            message: message.into(),
            expected: None,
            actual: None,
            millis: 0.0,
        }
    }

    fn profile(&self, spec: &TestSpec) -> Result<Profile> {
        Ok(match &spec.reasoning {
            Some(p) => Profile::parse(p).ok_or_else(|| format!("unknown reasoning profile {p}"))?,
            None => self.project.profile(),
        })
    }

    /// The project store as it is, or an overlay: the fixture with the project's ontologies
    /// (when the test names `data`), or a copy of the project (when it changes reasoning or rules).
    fn env(&self, spec: &TestSpec) -> Result<Env> {
        let profile = self.profile(spec)?;
        if spec.data.is_none() && spec.rules.is_none() && profile == self.project.profile() {
            return Ok(Env {
                store: self.project.store().clone(),
                options: self.project.options(),
            });
        }
        let store = Store::new()?;
        match &spec.data {
            Some(fixture) => {
                load_file(&store, &self.path(fixture), None)?;
                for f in self
                    .project
                    .files()
                    .iter()
                    .filter(|f| f.role == Role::Ontology)
                {
                    load_file(&store, &f.path, Some(&f.graph))?;
                }
            }
            None => {
                let quads: Vec<_> = self
                    .project
                    .store()
                    .iter()
                    .collect::<std::result::Result<_, _>>()?;
                store.extend(quads)?;
            }
        }
        let mut materialized = false;
        if profile == Profile::Owl2rl {
            store.materialize()?;
            materialized = true;
        }
        let rules: Vec<(String, String)> = match &spec.rules {
            Some(r) => vec![(r.clone(), std::fs::read_to_string(self.path(r))?)],
            None => self
                .project
                .rules()
                .iter()
                .map(|(uri, text)| (self.project.producer_name(uri), text.clone()))
                .collect(),
        };
        for (producer, text) in &rules {
            let options = oxilite::datalog::Options {
                union_default_graph: true,
                include_inferred: true,
                producer: producer.clone(),
                ..Default::default()
            };
            store.datalog_materialize_with(text, &options)?;
            materialized = true;
        }
        if profile == Profile::Owl2rl && !rules.is_empty() {
            store.materialize()?;
        }
        Ok(Env {
            store,
            options: QueryOptions {
                union_default_graph: true,
                reasoning: match profile {
                    Profile::Rdfs => Reasoning::Rdfs,
                    Profile::Owlql => Reasoning::OwlQl,
                    _ => Reasoning::None,
                },
                include_inferred: materialized,
                ..QueryOptions::default()
            },
        })
    }

    fn run_inner(&self, spec: &TestSpec) -> Result<Outcome> {
        let env = self.env(spec)?;
        if let Some(query) = &spec.query {
            return self.query_test(spec, query, &env);
        }
        if spec.entails.is_some() || spec.not_entails.is_some() {
            return self.entailment_test(spec, &env);
        }
        self.shacl_test(spec, &env)
    }

    fn query_test(&self, spec: &TestSpec, query: &str, env: &Env) -> Result<Outcome> {
        let text = std::fs::read_to_string(self.path(query))?;
        let actual = execute(query, &text, env)?;
        let Some(expect) = &spec.expect else {
            return Ok(Self::outcome(
                false,
                "no `expect` file: run Update Snapshot to create it",
            ));
        };
        let expect_path = self.path(expect);
        if !expect_path.exists() {
            return Ok(Outcome {
                actual: Some(actual.render()),
                ..Self::outcome(
                    false,
                    format!("{expect} does not exist: run Update Snapshot to create it"),
                )
            });
        }
        let expected = Answer::read(&expect_path)?;
        let ordered = text.to_ascii_uppercase().contains("ORDER BY");
        let diff = actual.diff(&expected, ordered);
        Ok(Outcome {
            passed: diff.is_none(),
            message: diff.unwrap_or_else(|| "results match".into()),
            expected: Some(expected.render()),
            actual: Some(actual.render()),
            ..Self::outcome(false, "")
        })
    }

    fn entailment_test(&self, spec: &TestSpec, env: &Env) -> Result<Outcome> {
        let mut missing = Vec::new();
        let mut unwanted = Vec::new();
        let ask = |t: &Triple| -> Result<bool> {
            let q = format!("ASK {{ {} {} {} }}", t.subject, t.predicate, t.object);
            Ok(matches!(
                env.store
                    .query_output(SparqlParser::new().parse_query(&q)?, &env.options)?,
                QueryOutput::Boolean(true)
            ))
        };
        if let Some(f) = &spec.entails {
            for t in read_triples(&self.path(f))? {
                if !ask(&t)? {
                    missing.push(t.to_string());
                }
            }
        }
        if let Some(f) = &spec.not_entails {
            for t in read_triples(&self.path(f))? {
                if ask(&t)? {
                    unwanted.push(t.to_string());
                }
            }
        }
        let passed = missing.is_empty() && unwanted.is_empty();
        let mut message = Vec::new();
        if !missing.is_empty() {
            message.push(format!("not entailed: {}", missing.join("; ")));
        }
        if !unwanted.is_empty() {
            message.push(format!(
                "entailed but should not be: {}",
                unwanted.join("; ")
            ));
        }
        Ok(Self::outcome(
            passed,
            if passed {
                "entailments hold".into()
            } else {
                message.join("\n")
            },
        ))
    }

    fn shacl_test(&self, spec: &TestSpec, env: &Env) -> Result<Outcome> {
        let shapes = match &spec.shapes {
            Some(f) => {
                let path = self.path(f);
                validate::to_ntriples(
                    &std::fs::read_to_string(&path)?,
                    format_of(&path).unwrap_or(RdfFormat::Turtle),
                )?
            }
            None => self.project.shapes_ntriples(),
        };
        let format = RDFFormat::NTriples;
        if shapes.trim().is_empty() {
            return Ok(Self::outcome(
                false,
                "no shapes: name a `shapes` file or add shapes to the project",
            ));
        }
        let inferred = self.project.validate_inferred();
        let report = validate::report(
            &env.store,
            &env.options,
            &shapes,
            &format,
            inferred,
            &|| false,
        )?
        .ok_or("validation was cancelled")?;
        let actual = validate::failing_shapes(&report, &validate::owners(&shapes));
        let mut expected: Vec<String> = spec
            .expect_violations
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|s| self.expand(s))
            .collect();
        expected.sort();
        let passed = actual == expected;
        let describe = |v: &[String]| {
            if v.is_empty() {
                "conforms".to_string()
            } else {
                v.join(", ")
            }
        };
        Ok(Outcome {
            passed,
            message: if passed {
                format!("{} ({} results)", describe(&actual), report.results().len())
            } else {
                format!(
                    "expected {}, got {}",
                    describe(&expected),
                    describe(&actual)
                )
            },
            expected: Some(describe(&expected)),
            actual: Some(describe(&actual)),
            ..Self::outcome(false, "")
        })
    }

    /// `ex:Shape` with a workspace prefix, `<iri>`, or an IRI as is.
    fn expand(&self, s: &str) -> String {
        let s = s.trim();
        if let Some(iri) = s.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
            return iri.to_string();
        }
        if let Some((p, local)) = s.split_once(':') {
            if let Some(ns) = self.project.index().prefixes().get(p) {
                return format!("{ns}{local}");
            }
            if let Some((_, ns)) = super::lang::WELL_KNOWN.iter().find(|(w, _)| *w == p) {
                return format!("{ns}{local}");
            }
        }
        s.to_string()
    }

    /// Writes the current results of a query test into its `expect` file.
    pub fn update_snapshot(&self, spec: &TestSpec) -> Result<String> {
        let query = spec
            .query
            .as_ref()
            .ok_or("only query tests have snapshots")?;
        let expect = spec
            .expect
            .as_ref()
            .ok_or("the test has no `expect` file")?;
        let env = self.env(spec)?;
        let text = std::fs::read_to_string(self.path(query))?;
        let answer = execute(query, &text, &env)?;
        let path = self.path(expect);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, answer.serialize(&path)?)?;
        Ok(path.display().to_string())
    }
}

fn load_file(store: &Store, path: &Path, graph: Option<&str>) -> Result<()> {
    let format =
        format_of(path).ok_or_else(|| format!("unknown RDF format of {}", path.display()))?;
    let mut parser = RdfParser::from_format(format);
    if let Some(g) = graph {
        parser = parser.with_default_graph(NamedNode::new(g)?);
    }
    store.load_from_slice(parser, &std::fs::read(path)?)?;
    Ok(())
}

fn read_triples(path: &Path) -> Result<Vec<Triple>> {
    let format = format_of(path).unwrap_or(RdfFormat::Turtle);
    let mut out = Vec::new();
    for q in RdfParser::from_format(format).for_slice(&std::fs::read(path)?) {
        let q = q?;
        out.push(Triple::new(q.subject, q.predicate, q.object));
    }
    Ok(out)
}

/// A comparable answer: solutions (rows of variable → N-Triples term), a boolean, a graph
/// (N-Triples lines), or Cypher rows (JSON).
#[derive(Debug, Clone, PartialEq)]
enum Answer {
    Rows(Vec<String>, Vec<BTreeMap<String, String>>),
    Boolean(bool),
    Graph(Vec<String>),
    Json(Vec<String>, Vec<Value>),
}

fn execute(file: &str, text: &str, env: &Env) -> Result<Answer> {
    let ext = Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("rq");
    match ext {
        "dl" => {
            let options = oxilite::datalog::Options {
                union_default_graph: env.options.union_default_graph,
                include_inferred: env.options.include_inferred,
                ..Default::default()
            };
            let r = env.store.datalog_with(text, &options)?;
            Ok(Answer::from_rows(&r.variables, &r.rows))
        }
        "cypher" | "cyp" => {
            let options = oxilite::cypher::CypherOptions {
                query: env.options.clone(),
                ..Default::default()
            };
            let r = env
                .store
                .cypher_with(text, &oxilite::cypher::Params::new(), &options)?;
            let v = r.to_json();
            Ok(Answer::Json(
                r.columns.clone(),
                v["rows"].as_array().cloned().unwrap_or_default(),
            ))
        }
        _ => {
            let q = SparqlParser::new().parse_query(text)?;
            Ok(match env.store.query_output(q, &env.options)? {
                QueryOutput::Boolean(b) => Answer::Boolean(b),
                QueryOutput::Graph(triples) => {
                    let mut lines: Vec<String> = triples.iter().map(|t| format!("{t} .")).collect();
                    lines.sort();
                    Answer::Graph(lines)
                }
                QueryOutput::Solutions { variables, rows } => {
                    let names: Vec<String> =
                        variables.iter().map(|v| v.as_str().to_string()).collect();
                    Answer::from_rows(&names, &rows)
                }
            })
        }
    }
}

impl Answer {
    fn from_rows(variables: &[String], rows: &[Vec<Option<Term>>]) -> Self {
        Answer::Rows(
            variables.to_vec(),
            rows.iter()
                .map(|r| {
                    variables
                        .iter()
                        .zip(r)
                        .filter_map(|(v, t)| Some((v.clone(), t.as_ref()?.to_string())))
                        .collect()
                })
                .collect(),
        )
    }

    fn read(path: &Path) -> Result<Self> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let data = std::fs::read(path)?;
        if let Some(format) = format_of(path) {
            let mut lines: Vec<String> = RdfParser::from_format(format)
                .for_slice(&data)
                .map(|q| q.map(|q| format!("{} .", Triple::new(q.subject, q.predicate, q.object))))
                .collect::<std::result::Result<_, _>>()?;
            lines.sort();
            return Ok(Answer::Graph(lines));
        }
        if ext == "json" {
            let v: Value = serde_json::from_slice(&data)?;
            if let (Some(cols), Some(rows)) = (v["columns"].as_array(), v["rows"].as_array()) {
                return Ok(Answer::Json(
                    cols.iter()
                        .filter_map(|c| c.as_str().map(String::from))
                        .collect(),
                    rows.clone(),
                ));
            }
        }
        let format = match ext {
            "srx" | "xml" => QueryResultsFormat::Xml,
            "csv" => QueryResultsFormat::Csv,
            "tsv" => QueryResultsFormat::Tsv,
            _ => QueryResultsFormat::Json,
        };
        Ok(
            match QueryResultsParser::from_format(format).for_slice(&data)? {
                SliceQueryResultsParserOutput::Boolean(b) => Answer::Boolean(b),
                SliceQueryResultsParserOutput::Solutions(solutions) => {
                    let variables: Vec<String> = solutions
                        .variables()
                        .iter()
                        .map(|v| v.as_str().to_string())
                        .collect();
                    let mut rows = Vec::new();
                    for s in solutions {
                        let s = s?;
                        rows.push(
                            s.iter()
                                .map(|(v, t)| (v.as_str().to_string(), t.to_string()))
                                .collect(),
                        );
                    }
                    Answer::Rows(variables, rows)
                }
            },
        )
    }

    /// `None` when equal; otherwise what differs. Rows compare as multisets unless ordered.
    fn diff(&self, expected: &Self, ordered: bool) -> Option<String> {
        match (self, expected) {
            (Answer::Rows(_, a), Answer::Rows(_, e)) => {
                if ordered {
                    return (a != e).then(|| {
                        format!(
                            "expected {} rows in order, got {} (or a different order)",
                            e.len(),
                            a.len()
                        )
                    });
                }
                let key = |r: &BTreeMap<String, String>| format!("{r:?}");
                let mut a: Vec<String> = a.iter().map(key).collect();
                let mut e: Vec<String> = e.iter().map(key).collect();
                a.sort();
                e.sort();
                if a == e {
                    return None;
                }
                let missing: Vec<&String> = e.iter().filter(|r| !a.contains(r)).take(5).collect();
                let extra: Vec<&String> = a.iter().filter(|r| !e.contains(r)).take(5).collect();
                Some(format!(
                    "expected {} rows, got {}\nmissing: {missing:?}\nunexpected: {extra:?}",
                    e.len(),
                    a.len()
                ))
            }
            (Answer::Graph(a), Answer::Graph(e)) => (a != e).then(|| {
                let missing: Vec<&String> = e.iter().filter(|t| !a.contains(t)).take(5).collect();
                let extra: Vec<&String> = a.iter().filter(|t| !e.contains(t)).take(5).collect();
                format!(
                    "expected {} triples, got {}\nmissing: {missing:?}\nunexpected: {extra:?}",
                    e.len(),
                    a.len()
                )
            }),
            (a, e) => (a != e).then(|| format!("expected {}, got {}", e.render(), a.render())),
        }
    }

    fn render(&self) -> String {
        match self {
            Answer::Boolean(b) => b.to_string(),
            Answer::Graph(lines) => lines.join("\n"),
            Answer::Rows(vars, rows) => {
                let mut out = vars.join("\t");
                for r in rows {
                    out.push('\n');
                    out.push_str(
                        &vars
                            .iter()
                            .map(|v| r.get(v).cloned().unwrap_or_default())
                            .collect::<Vec<_>>()
                            .join("\t"),
                    );
                }
                out
            }
            Answer::Json(cols, rows) => {
                serde_json::to_string_pretty(&json!({"columns": cols, "rows": rows}))
                    .unwrap_or_default()
            }
        }
    }

    /// The answer in the expected file's format.
    fn serialize(&self, path: &Path) -> Result<String> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("srj");
        Ok(match self {
            Answer::Json(..) => self.render(),
            Answer::Graph(lines) => lines.join("\n") + "\n",
            Answer::Boolean(b) => oxilite_core::json::output_to_format(
                &QueryOutput::Boolean(*b),
                results_format(ext),
            )?,
            Answer::Rows(vars, rows) => {
                let variables: Vec<Variable> = vars.iter().map(Variable::new_unchecked).collect();
                let parsed: Vec<Vec<Option<Term>>> = rows
                    .iter()
                    .map(|r| {
                        vars.iter()
                            .map(|v| r.get(v).and_then(|t| parse_term(t)))
                            .collect()
                    })
                    .collect();
                oxilite_core::json::output_to_format(
                    &QueryOutput::Solutions {
                        variables,
                        rows: parsed,
                    },
                    results_format(ext),
                )?
            }
        })
    }
}

fn results_format(ext: &str) -> &'static str {
    match ext {
        "srx" | "xml" => "xml",
        "csv" => "csv",
        "tsv" => "tsv",
        _ => "json",
    }
}

/// A term back from its N-Triples form.
fn parse_term(nt: &str) -> Option<Term> {
    let line = format!("<urn:s> <urn:p> {nt} .");
    RdfParser::from_format(RdfFormat::NTriples)
        .for_slice(line.as_bytes())
        .next()?
        .ok()
        .map(|q| q.object)
}
