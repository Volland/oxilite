//! SHACL shapes as the property-graph schema, and the schema procedures.
//!
//! When enabled, the shapes stored in the dataset are read once per writing statement from
//! the compiled shape index (`oxilite_core::shapes`), which costs one request and no SPARQL
//! evaluation; the registered shapes graphs decide which graphs it covers. Property shapes with `sh:maxCount 1` make a property scalar when read; every
//! node a statement creates or changes is checked against the datatype, cardinality, `sh:in`
//! and `sh:pattern` constraints of the shapes targeting its labels, before the write batch is
//! sent — a violation aborts the statement. Complete SHACL and ShEx validation stays with
//! rudof (`oxilite-validate`).
//!
// @lat: [[architecture#Property graph frontend#OWL and SHACL awareness]]

use crate::ast::Expr;
use crate::error::{CypherError, Result};
use crate::lower::Lowerer;
use crate::vocab::Vocabulary;
use oxilite_core::query::QueryOutput;
use oxilite_core::shapes::ShapeIndex;
use oxilite_core::{Capabilities, Request};
use oxrdf::{NamedNode, NamedOrBlankNode, Term};
use spargebra::algebra::GraphPattern;
use spargebra::SparqlParser;
use std::collections::BTreeMap;

const SH: &str = "http://www.w3.org/ns/shacl#";

#[derive(Debug, Clone, Default, PartialEq)]
struct PropertyShape {
    datatype: Option<NamedNode>,
    min: Option<i64>,
    max: Option<i64>,
    values_in: Vec<Term>,
    pattern: Option<String>,
    /// Relationship-valued (`sh:class` / `sh:node`): not checked by the guards.
    relationship: bool,
}

/// Property shapes by target class and path.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shapes {
    by_class: BTreeMap<NamedNode, BTreeMap<NamedNode, PropertyShape>>,
}

/// The request reading the compiled shape index (see [`Shapes::from_index`]).
pub fn schema_request(caps: &Capabilities) -> Request {
    ShapeIndex::load_request(caps)
}

/// The SPARQL query that reads the same shapes by evaluation instead of from the index. Kept
/// as the reference the index is checked against (see the agreement test).
pub fn schema_query() -> spargebra::Query {
    let body = format!(
        "?shape <{SH}targetClass> ?target ; <{SH}property> ?ps . ?ps <{SH}path> ?path . \
         OPTIONAL {{ ?ps <{SH}datatype> ?datatype }} OPTIONAL {{ ?ps <{SH}minCount> ?min }} \
         OPTIONAL {{ ?ps <{SH}maxCount> ?max }} OPTIONAL {{ ?ps <{SH}pattern> ?pattern }} \
         OPTIONAL {{ ?ps <{SH}class> ?class }} OPTIONAL {{ ?ps <{SH}node> ?node }} \
         OPTIONAL {{ ?ps <{SH}in>/<http://www.w3.org/1999/02/22-rdf-syntax-ns#rest>*/<http://www.w3.org/1999/02/22-rdf-syntax-ns#first> ?in }}"
    );
    SparqlParser::new()
        .parse_query(&format!(
            "SELECT ?target ?path ?datatype ?min ?max ?pattern ?class ?node ?in WHERE {{ {{ {body} }} UNION {{ GRAPH ?g {{ {body} }} }} }}"
        ))
        .expect("valid shapes query")
}

impl Shapes {
    /// Builds the summary from the compiled shape index.
    pub fn from_index(index: ShapeIndex) -> Self {
        let mut me = Self::default();
        for (target, paths) in index.by_class {
            let entry = me.by_class.entry(target).or_default();
            for (path, shape) in paths {
                entry.insert(
                    path,
                    PropertyShape {
                        datatype: shape.datatype,
                        min: shape.min,
                        max: shape.max,
                        values_in: shape.values_in,
                        pattern: shape.pattern,
                        relationship: shape.relationship,
                    },
                );
            }
        }
        me.sort_values();
        me
    }

    /// `sh:in` is a set: its order carries no meaning and must not depend on where the shapes
    /// were read from (the index or the query).
    fn sort_values(&mut self) {
        for paths in self.by_class.values_mut() {
            for shape in paths.values_mut() {
                shape.values_in.sort_by_key(ToString::to_string);
            }
        }
    }

    /// Builds the summary from the output of [`schema_query`].
    pub fn from_output(out: QueryOutput) -> Result<Self> {
        let QueryOutput::Solutions { variables, rows } = out else {
            return Err(CypherError::runtime("unexpected shapes query output"));
        };
        let idx = |n: &str| variables.iter().position(|v| v.as_str() == n);
        let get = |row: &Vec<Option<Term>>, n: &str| idx(n).and_then(|i| row[i].clone());
        let int = |t: Option<Term>| match t {
            Some(Term::Literal(l)) => l.value().parse::<i64>().ok(),
            _ => None,
        };
        let mut me = Self::default();
        for row in &rows {
            let (Some(Term::NamedNode(target)), Some(Term::NamedNode(path))) =
                (get(row, "target"), get(row, "path"))
            else {
                continue;
            };
            let ps = me
                .by_class
                .entry(target)
                .or_default()
                .entry(path)
                .or_default();
            if let Some(Term::NamedNode(d)) = get(row, "datatype") {
                ps.datatype = Some(d);
            }
            if let Some(m) = int(get(row, "min")) {
                ps.min = Some(m);
            }
            if let Some(m) = int(get(row, "max")) {
                ps.max = Some(m);
            }
            if let Some(Term::Literal(p)) = get(row, "pattern") {
                ps.pattern = Some(p.value().to_string());
            }
            if get(row, "class").is_some() || get(row, "node").is_some() {
                ps.relationship = true;
            }
            if let Some(v) = get(row, "in") {
                if !ps.values_in.contains(&v) {
                    ps.values_in.push(v);
                }
            }
        }
        me.sort_values();
        Ok(me)
    }

    /// Whether a property of a node with these labels is declared mandatory.
    pub fn is_required(&self, labels: &[NamedNode], path: &NamedNode) -> bool {
        labels.iter().any(|l| {
            self.by_class
                .get(l)
                .and_then(|m| m.get(path))
                .is_some_and(|ps| ps.min.is_some_and(|m| m >= 1) && !ps.relationship)
        })
    }

    /// The value type a shape's `sh:datatype` declares for a property of a node with these
    /// labels.
    pub fn value_type(
        &self,
        labels: &[NamedNode],
        path: &NamedNode,
    ) -> Option<oxilite_core::ValueType> {
        use oxilite_core::ValueType;
        labels.iter().find_map(|l| {
            let dt = self.by_class.get(l)?.get(path)?.datatype.as_ref()?;
            let local = dt
                .as_str()
                .strip_prefix("http://www.w3.org/2001/XMLSchema#")?;
            Some(match local {
                "integer" | "decimal" | "double" | "float" | "int" | "long" | "short" | "byte"
                | "nonNegativeInteger" | "positiveInteger" | "nonPositiveInteger"
                | "negativeInteger" | "unsignedLong" | "unsignedInt" | "unsignedShort"
                | "unsignedByte" => ValueType::Numeric,
                "string" => ValueType::String,
                "boolean" => ValueType::Boolean,
                _ => return None,
            })
        })
    }

    /// Whether the dataset declares no shapes.
    pub fn is_empty(&self) -> bool {
        self.by_class.is_empty()
    }

    /// Whether a property of a node with these labels is declared single-valued.
    pub(crate) fn is_scalar(&self, labels: &[NamedNode], path: &NamedNode) -> bool {
        labels.iter().any(|l| {
            self.by_class
                .get(l)
                .and_then(|m| m.get(path))
                .is_some_and(|ps| ps.max == Some(1))
        })
    }

    /// Checks the literal-valued constraints of every shape targeting the node's labels.
    pub(crate) fn check(
        &self,
        node: &NamedOrBlankNode,
        labels: &[NamedNode],
        props: &[(NamedNode, Term)],
        vocab: &Vocabulary,
    ) -> Result<()> {
        for l in labels {
            let Some(shapes) = self.by_class.get(l) else {
                continue;
            };
            for (path, ps) in shapes {
                if ps.relationship {
                    continue;
                }
                let values: Vec<&Term> = props
                    .iter()
                    .filter(|(p, _)| p == path)
                    .map(|(_, o)| o)
                    .collect();
                let what = || {
                    format!(
                        "node <{}> ({}) property `{}`",
                        crate::value::subject_str(node),
                        vocab.name(l.as_str()),
                        vocab.name(path.as_str())
                    )
                };
                if let Some(dt) = &ps.datatype {
                    for v in &values {
                        let ok = matches!(v, Term::Literal(x) if x.datatype() == dt.as_ref());
                        if !ok {
                            return Err(CypherError::ShapeViolation(format!(
                                "{} must have datatype {}, got {v}",
                                what(),
                                vocab.name(dt.as_str())
                            )));
                        }
                    }
                    if let Some(min) = ps.min {
                        if (values.len() as i64) < min {
                            return Err(CypherError::ShapeViolation(format!(
                                "{} needs at least {min} value(s)",
                                what()
                            )));
                        }
                    }
                }
                if let Some(max) = ps.max {
                    if values.len() as i64 > max {
                        return Err(CypherError::ShapeViolation(format!(
                            "{} allows at most {max} value(s), got {}",
                            what(),
                            values.len()
                        )));
                    }
                }
                if !ps.values_in.is_empty() {
                    for v in &values {
                        if !ps.values_in.contains(v) {
                            return Err(CypherError::ShapeViolation(format!(
                                "{} value {v} is not one of the allowed values",
                                what()
                            )));
                        }
                    }
                }
                if let Some(p) = &ps.pattern {
                    let re = regex_lite::Regex::new(p)
                        .map_err(|e| CypherError::runtime(format!("invalid sh:pattern: {e}")))?;
                    for v in &values {
                        if let Term::Literal(x) = v {
                            if !re.is_match(x.value()) {
                                return Err(CypherError::ShapeViolation(format!(
                                    "{} value \"{}\" does not match /{p}/",
                                    what(),
                                    x.value()
                                )));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn pattern_of(q: &str) -> GraphPattern {
    match SparqlParser::new()
        .parse_query(q)
        .expect("valid procedure query")
    {
        spargebra::Query::Select { pattern, .. } => pattern,
        _ => unreachable!("SELECT"),
    }
}

/// The SPARQL pattern and columns of a schema procedure. Result variables are named
/// `proc_<column>`; IRIs in them are returned as property-graph names.
pub(crate) fn procedure_pattern(
    name: &str,
    args: &[Expr],
    lw: &Lowerer<'_>,
) -> Result<(GraphPattern, Vec<String>)> {
    let _ = lw;
    if !args.is_empty() {
        return Err(CypherError::semantic(format!(
            "{name}() takes no arguments"
        )));
    }
    let not_schema = "FILTER(!STRSTARTS(STR(?p), \"http://www.w3.org/\"))";
    Ok(match name {
        "db.labels" => (
            pattern_of(&format!(
                "SELECT DISTINCT ?proc_label WHERE {{ {{ ?x a ?proc_label }} UNION {{ ?s <{SH}targetClass> ?proc_label }} UNION {{ GRAPH ?g {{ ?s <{SH}targetClass> ?proc_label }} }} FILTER(isIRI(?proc_label) && ?proc_label != <http://www.w3.org/2000/01/rdf-schema#Resource>) }} ORDER BY ?proc_label"
            )),
            vec!["label".into()],
        ),
        "db.relationshiptypes" => (
            pattern_of(&format!(
                "SELECT DISTINCT ?proc_relationshipType WHERE {{ ?s ?p ?o FILTER(isIRI(?o) || isBLANK(?o)) FILTER(?p != <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> && ?p != <{}>) {not_schema} BIND(?p AS ?proc_relationshipType) }} ORDER BY ?proc_relationshipType",
                crate::value::REIFIES
            )),
            vec!["relationshipType".into()],
        ),
        "db.propertykeys" => (
            pattern_of(&format!(
                "SELECT DISTINCT ?proc_propertyKey WHERE {{ ?s ?p ?o FILTER(isLITERAL(?o)) {not_schema} BIND(?p AS ?proc_propertyKey) }} ORDER BY ?proc_propertyKey"
            )),
            vec!["propertyKey".into()],
        ),
        "db.schema" | "db.schema.nodetypeproperties" => {
            let body = format!(
                "?shape <{SH}targetClass> ?proc_nodeType ; <{SH}property> ?ps . ?ps <{SH}path> ?proc_propertyName . \
                 OPTIONAL {{ ?ps <{SH}datatype> ?proc_propertyType }} OPTIONAL {{ ?ps <{SH}minCount> ?mc }} OPTIONAL {{ ?ps <{SH}maxCount> ?xc }} \
                 BIND(COALESCE(?mc >= 1, false) AS ?proc_mandatory) BIND(COALESCE(?xc = 1, false) AS ?proc_single)"
            );
            (
                pattern_of(&format!(
                    "SELECT ?proc_nodeType ?proc_propertyName ?proc_propertyType ?proc_mandatory ?proc_single WHERE {{ {{ {body} }} UNION {{ GRAPH ?g {{ {body} }} }} }} ORDER BY ?proc_nodeType ?proc_propertyName"
                )),
                vec![
                    "nodeType".into(),
                    "propertyName".into(),
                    "propertyType".into(),
                    "mandatory".into(),
                    "single".into(),
                ],
            )
        }
        other => {
            return Err(CypherError::unsupported(format!(
                "procedure {other} (available: db.labels, db.relationshipTypes, db.propertyKeys, db.schema.nodeTypeProperties)"
            )))
        }
    })
}
