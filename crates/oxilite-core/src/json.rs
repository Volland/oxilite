//! JSON forms of terms, quads and results shared by the JavaScript bindings (Node and D1).
//!
//! Terms follow RDF/JS: `{"termType": "NamedNode" | "BlankNode" | "Literal" | "Quad" |
//! "DefaultGraph", "value", "language", "direction", "datatype", …}`.
//!
// @lat: [[architecture#Bindings]]

use crate::error::{Error, Result};
use crate::query::QueryOutput;
use oxrdf::{
    BaseDirection, BlankNode, GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, Term, Triple,
};
use serde_json::{json, Map, Value};

pub fn term_to_json(t: &Term) -> Value {
    match t {
        Term::NamedNode(n) => json!({"termType": "NamedNode", "value": n.as_str()}),
        Term::BlankNode(b) => json!({"termType": "BlankNode", "value": b.as_str()}),
        Term::Literal(l) => {
            let mut m = Map::new();
            m.insert("termType".into(), "Literal".into());
            m.insert("value".into(), l.value().into());
            m.insert("language".into(), l.language().unwrap_or("").into());
            if let Some(d) = l.direction() {
                m.insert(
                    "direction".into(),
                    match d {
                        BaseDirection::Ltr => "ltr",
                        BaseDirection::Rtl => "rtl",
                    }
                    .into(),
                );
            }
            m.insert(
                "datatype".into(),
                json!({"termType": "NamedNode", "value": l.datatype().as_str()}),
            );
            Value::Object(m)
        }
        Term::Triple(t) => json!({
            "termType": "Quad",
            "value": "",
            "subject": term_to_json(&t.subject.clone().into()),
            "predicate": term_to_json(&t.predicate.clone().into()),
            "object": term_to_json(&t.object),
            "graph": {"termType": "DefaultGraph", "value": ""},
        }),
    }
}

fn graph_to_json(g: &GraphName) -> Value {
    match g {
        GraphName::DefaultGraph => json!({"termType": "DefaultGraph", "value": ""}),
        GraphName::NamedNode(n) => term_to_json(&n.clone().into()),
        GraphName::BlankNode(b) => term_to_json(&b.clone().into()),
    }
}

pub fn quad_to_json(q: &Quad) -> Value {
    json!({
        "termType": "Quad",
        "value": "",
        "subject": term_to_json(&q.subject.clone().into()),
        "predicate": term_to_json(&q.predicate.clone().into()),
        "object": term_to_json(&q.object),
        "graph": graph_to_json(&q.graph_name),
    })
}

fn field<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Other(format!("term JSON without \"{k}\": {v}")))
}

pub fn json_to_term(v: &Value) -> Result<Term> {
    Ok(match field(v, "termType")? {
        "NamedNode" => NamedNode::new(field(v, "value")?)
            .map_err(|e| Error::Other(e.to_string()))?
            .into(),
        "BlankNode" => BlankNode::new(field(v, "value")?)
            .map_err(|e| Error::Other(e.to_string()))?
            .into(),
        "Literal" => {
            let value = field(v, "value")?;
            let lang = v.get("language").and_then(Value::as_str).unwrap_or("");
            let dt = v
                .get("datatype")
                .and_then(|d| d.get("value"))
                .and_then(Value::as_str);
            if !lang.is_empty() {
                match v.get("direction").and_then(Value::as_str) {
                    Some(d @ ("ltr" | "rtl")) => Literal::new_directional_language_tagged_literal(
                        value,
                        lang,
                        if d == "rtl" {
                            BaseDirection::Rtl
                        } else {
                            BaseDirection::Ltr
                        },
                    ),
                    _ => Literal::new_language_tagged_literal(value, lang),
                }
                .map_err(|e| Error::Other(e.to_string()))?
                .into()
            } else if let Some(dt) = dt {
                Literal::new_typed_literal(
                    value,
                    NamedNode::new(dt).map_err(|e| Error::Other(e.to_string()))?,
                )
                .into()
            } else {
                Literal::new_simple_literal(value).into()
            }
        }
        "Quad" => {
            let s = json_to_term(v.get("subject").unwrap_or(&Value::Null))?;
            let p = json_to_term(v.get("predicate").unwrap_or(&Value::Null))?;
            let o = json_to_term(v.get("object").unwrap_or(&Value::Null))?;
            crate::encoding::make_triple(s, p, o)?.into()
        }
        other => return Err(Error::Other(format!("unsupported termType {other}"))),
    })
}

pub fn json_to_graph(v: &Value) -> Result<GraphName> {
    if v.is_null() || field(v, "termType")? == "DefaultGraph" {
        return Ok(GraphName::DefaultGraph);
    }
    match json_to_term(v)? {
        Term::NamedNode(n) => Ok(n.into()),
        Term::BlankNode(b) => Ok(b.into()),
        _ => Err(Error::Other("invalid graph name".into())),
    }
}

pub fn json_to_quad(v: &Value) -> Result<Quad> {
    let s = match json_to_term(v.get("subject").unwrap_or(&Value::Null))? {
        Term::NamedNode(n) => NamedOrBlankNode::from(n),
        Term::BlankNode(b) => NamedOrBlankNode::from(b),
        _ => return Err(Error::Other("invalid subject".into())),
    };
    let Term::NamedNode(p) = json_to_term(v.get("predicate").unwrap_or(&Value::Null))? else {
        return Err(Error::Other("invalid predicate".into()));
    };
    let o = json_to_term(v.get("object").unwrap_or(&Value::Null))?;
    let g = json_to_graph(v.get("graph").unwrap_or(&Value::Null))?;
    Ok(Quad::new(s, p, o, g))
}

/// `{"kind": "solutions", "variables": [...], "rows": [[term | null]]}`,
/// `{"kind": "boolean", "value": b}` or `{"kind": "quads", "quads": [...]}`.
pub fn output_to_json(out: &QueryOutput) -> Value {
    match out {
        QueryOutput::Solutions { variables, rows } => json!({
            "kind": "solutions",
            "variables": variables.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
            "rows": rows.iter().map(|r| r.iter().map(|t| t.as_ref().map_or(Value::Null, term_to_json)).collect::<Vec<_>>()).collect::<Vec<_>>(),
        }),
        QueryOutput::Boolean(b) => json!({"kind": "boolean", "value": b}),
        QueryOutput::Graph(triples) => json!({
            "kind": "quads",
            "quads": triples.iter().map(|t: &Triple| quad_to_json(&t.clone().in_graph(GraphName::DefaultGraph))).collect::<Vec<_>>(),
        }),
    }
}

/// SPARQL 1.1 JSON results serialization of an output (SELECT / ASK).
pub fn output_to_sparql_json(out: &QueryOutput) -> Result<String> {
    use sparesults::{QueryResultsFormat, QueryResultsSerializer};
    let serializer = QueryResultsSerializer::from_format(QueryResultsFormat::Json);
    match out {
        QueryOutput::Boolean(b) => Ok(String::from_utf8(
            serializer.serialize_boolean_to_writer(Vec::new(), *b)?,
        )
        .unwrap_or_default()),
        QueryOutput::Solutions { variables, rows } => {
            let mut w = serializer.serialize_solutions_to_writer(Vec::new(), variables.clone())?;
            for row in rows {
                w.serialize(
                    variables
                        .iter()
                        .zip(row)
                        .filter_map(|(v, t)| Some((v.as_ref(), t.as_ref()?.as_ref()))),
                )?;
            }
            Ok(String::from_utf8(w.finish()?).unwrap_or_default())
        }
        QueryOutput::Graph(_) => Err(Error::Other(
            "CONSTRUCT/DESCRIBE results are graphs, not SPARQL JSON results".into(),
        )),
    }
}

/// Query options with Oxigraph's JavaScript names; graphs are JSON terms.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct JsQueryOptions {
    pub base_iri: Option<String>,
    pub use_default_graph_as_union: bool,
    pub default_graph: Option<Value>,
    pub named_graphs: Option<Vec<Value>>,
    pub results_format: Option<String>,
    /// `"none"` (default), `"rdfs"` or `"owl-ql"`.
    pub reasoning: crate::reason::Reasoning,
    /// Also match materialized inferences.
    pub include_inferred: bool,
}

impl JsQueryOptions {
    /// Converts to core options (graph terms to ids).
    pub fn to_options(&self) -> Result<crate::QueryOptions> {
        let gid = |v: &Value| -> Result<i64> {
            Ok(match json_to_graph(v)? {
                GraphName::DefaultGraph => crate::encoding::DEFAULT_GRAPH_ID,
                g => crate::encoding::graph_id(g.as_ref()),
            })
        };
        Ok(crate::QueryOptions {
            union_default_graph: self.use_default_graph_as_union,
            default_graph: match &self.default_graph {
                None => None,
                Some(Value::Array(a)) => Some(a.iter().map(gid).collect::<Result<_>>()?),
                Some(v) => Some(vec![gid(v)?]),
            },
            named_graphs: self
                .named_graphs
                .as_ref()
                .map(|a| a.iter().map(gid).collect::<Result<_>>())
                .transpose()?,
            reasoning: self.reasoning,
            include_inferred: self.include_inferred,
            ..crate::QueryOptions::default()
        })
    }
}

/// Serializes an output in a results format (`json`, `xml`, `csv`, `tsv` or a media type) for
/// SELECT/ASK, or in an RDF format for CONSTRUCT/DESCRIBE.
pub fn output_to_format(out: &QueryOutput, format: &str) -> Result<String> {
    use sparesults::{QueryResultsFormat, QueryResultsSerializer};
    if let QueryOutput::Graph(triples) = out {
        let f = oxrdfio::RdfFormat::from_media_type(format)
            .or_else(|| oxrdfio::RdfFormat::from_extension(format))
            .ok_or_else(|| Error::Other(format!("unknown RDF format {format}")))?;
        let mut s = oxrdfio::RdfSerializer::from_format(f).for_writer(Vec::new());
        for t in triples {
            s.serialize_triple(t)?;
        }
        return Ok(String::from_utf8_lossy(&s.finish()?).into_owned());
    }
    let f = QueryResultsFormat::from_media_type(format)
        .or_else(|| QueryResultsFormat::from_extension(format))
        .ok_or_else(|| Error::Other(format!("unknown results format {format}")))?;
    let serializer = QueryResultsSerializer::from_format(f);
    let bytes = match out {
        QueryOutput::Boolean(b) => serializer.serialize_boolean_to_writer(Vec::new(), *b)?,
        QueryOutput::Solutions { variables, rows } => {
            let mut w = serializer.serialize_solutions_to_writer(Vec::new(), variables.clone())?;
            for row in rows {
                w.serialize(
                    variables
                        .iter()
                        .zip(row)
                        .filter_map(|(v, t)| Some((v.as_ref(), t.as_ref()?.as_ref()))),
                )?;
            }
            w.finish()?
        }
        QueryOutput::Graph(_) => unreachable!(),
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
