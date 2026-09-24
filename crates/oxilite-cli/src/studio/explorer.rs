//! The Store Explorer tree: graphs, the class hierarchy with asserted and inferred instance
//! counts, properties by use, the project's files and prefixes.
//!
// @lat: [[architecture#Studio server#Store Explorer]]

use super::conn::{Target, Vocab};
use super::project::Project;
use oxilite::model::Term;
use oxilite::sparql::{QueryOptions, Reasoning, SparqlParser};
use oxilite_core::QueryOutput;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn rows(t: &Target, text: &str, options: &QueryOptions) -> Result<Vec<Vec<Option<Term>>>> {
    match t
        .store
        .query_output(SparqlParser::new().parse_query(text)?, options)?
    {
        QueryOutput::Solutions { rows, .. } => Ok(rows),
        _ => Ok(Vec::new()),
    }
}

fn iri(t: &Option<Term>) -> Option<String> {
    match t {
        Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
        _ => None,
    }
}

fn count(t: &Option<Term>) -> u64 {
    match t {
        Some(Term::Literal(l)) => l.value().parse().unwrap_or(0),
        _ => 0,
    }
}

fn node(id: String, label: String, description: String, kind: &str, collapsible: bool) -> Value {
    json!({"id": id, "label": label, "description": description, "kind": kind, "collapsible": collapsible})
}

/// The children of an explorer node (`root` for the top level).
pub fn children(
    target: &Target,
    vocab: &Vocab,
    project: Option<&Project>,
    id: &str,
    short: impl Fn(&str) -> String,
) -> Result<Value> {
    let plain = QueryOptions {
        reasoning: Reasoning::None,
        include_inferred: false,
        ..target.options.clone()
    };
    Ok(match id {
        "root" => {
            let mut v = vec![
                node(
                    "graphs".into(),
                    "Graphs".into(),
                    String::new(),
                    "folder",
                    true,
                ),
                node(
                    "classes".into(),
                    "Classes".into(),
                    format!("{}", vocab.classes.len()),
                    "folder",
                    true,
                ),
                node(
                    "properties".into(),
                    "Properties".into(),
                    format!("{}", vocab.predicates.len()),
                    "folder",
                    true,
                ),
            ];
            if let Some(p) = project {
                v.push(node(
                    "files".into(),
                    "Files".into(),
                    format!("{}", p.files().len()),
                    "folder",
                    true,
                ));
                v.push(node(
                    "prefixes".into(),
                    "Prefixes".into(),
                    format!("{}", p.index().prefixes().len()),
                    "folder",
                    true,
                ));
            }
            json!(v)
        }
        "graphs" => {
            let r = rows(
                target,
                "SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g ORDER BY ?g",
                &plain,
            )?;
            json!(r
                .iter()
                .filter_map(|row| {
                    let g = iri(&row[0])?;
                    let mut n = node(
                        format!("graph:{g}"),
                        short(&g),
                        format!("{} triples", count(&row[1])),
                        "graph",
                        false,
                    );
                    n["iri"] = json!(g);
                    Some(n)
                })
                .collect::<Vec<_>>())
        }
        "classes" | "class" => json!(classes(target, vocab, &plain, None, &short)?),
        c if c.starts_with("class:") => {
            json!(classes(target, vocab, &plain, Some(&c[6..]), &short)?)
        }
        "properties" => json!(vocab
            .predicates
            .iter()
            .map(|(p, n)| {
                let mut v = node(
                    format!("property:{p}"),
                    short(p),
                    format!("{n} uses"),
                    "property",
                    false,
                );
                v["iri"] = json!(p);
                v
            })
            .collect::<Vec<_>>()),
        "files" => json!(project
            .map(|p| p
                .files()
                .iter()
                .map(|f| {
                    let name = f.uri.rsplit('/').next().unwrap_or(&f.uri).to_string();
                    let mut d = format!("{} · {} triples", f.role.name(), f.triples);
                    if f.graph != f.uri {
                        d.push_str(&format!(" · {}", short(&f.graph)));
                    }
                    if f.error.is_some() || p.reasoning_errors().contains_key(&f.uri) {
                        d.push_str(" · error");
                    }
                    let mut v = node(format!("file:{}", f.uri), name, d, f.role.name(), false);
                    v["uri"] = json!(f.uri);
                    v
                })
                .collect::<Vec<_>>())
            .unwrap_or_default()),
        "prefixes" => json!(project
            .map(|p| p
                .index()
                .prefixes()
                .iter()
                .map(|(k, ns)| node(
                    format!("prefix:{k}"),
                    format!("{k}:"),
                    ns.clone(),
                    "prefix",
                    false
                ))
                .collect::<Vec<_>>())
            .unwrap_or_default()),
        _ => json!([]),
    })
}

/// Classes under `parent` (roots when `None`) in the asserted hierarchy, with instance counts:
/// asserted, plus how many more the reasoning adds.
fn classes(
    target: &Target,
    vocab: &Vocab,
    plain: &QueryOptions,
    parent: Option<&str>,
    short: &impl Fn(&str) -> String,
) -> Result<Vec<Value>> {
    let pairs = rows(
        target,
        "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
         SELECT DISTINCT ?c ?sup WHERE { ?c rdfs:subClassOf ?sup FILTER(isIRI(?c) && isIRI(?sup) && ?c != ?sup) } LIMIT 50000",
        plain,
    )?;
    let mut supers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut subs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in &pairs {
        if let (Some(c), Some(s)) = (iri(&r[0]), iri(&r[1])) {
            supers.entry(c.clone()).or_default().insert(s.clone());
            subs.entry(s).or_default().insert(c);
        }
    }
    let asserted: BTreeMap<String, u64> = rows(
        target,
        "SELECT ?c (COUNT(?s) AS ?n) WHERE { ?s a ?c } GROUP BY ?c",
        plain,
    )?
    .iter()
    .filter_map(|r| Some((iri(&r[0])?, count(&r[1]))))
    .collect();
    let all: BTreeSet<String> = vocab
        .classes
        .iter()
        .map(|(c, _)| c.clone())
        .chain(supers.keys().cloned())
        .chain(subs.keys().cloned())
        .collect();
    let chosen: Vec<&String> = match parent {
        None => all.iter().filter(|c| !supers.contains_key(*c)).collect(),
        Some(p) => subs.get(p).map(|s| s.iter().collect()).unwrap_or_default(),
    };
    Ok(chosen
        .into_iter()
        .map(|c| {
            let total = vocab.class_count(c).unwrap_or(0);
            let own = asserted.get(c).copied().unwrap_or(0);
            let description = if total > own {
                format!("{own} (+{} inferred)", total - own)
            } else {
                format!("{own}")
            };
            let mut v = node(
                format!("class:{c}"),
                short(c),
                description,
                "class",
                subs.contains_key(c),
            );
            v["iri"] = json!(c);
            v
        })
        .collect())
}

/// The ontology as a diagram: classes with instance counts and datatype properties, subclass
/// links, and object properties from their domain to their range.
pub fn ontology(target: &Target) -> Result<Value> {
    let plain = QueryOptions {
        reasoning: Reasoning::None,
        include_inferred: false,
        ..target.options.clone()
    };
    let rdfs = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> PREFIX owl: <http://www.w3.org/2002/07/owl#> ";
    let classes: BTreeMap<String, u64> = rows(
        target,
        &format!("{rdfs}SELECT ?c (COUNT(?x) AS ?n) WHERE {{ {{ ?c a owl:Class }} UNION {{ ?c a rdfs:Class }} UNION {{ ?c rdfs:subClassOf ?y }} UNION {{ ?y rdfs:subClassOf ?c }} UNION {{ ?z a ?c }} OPTIONAL {{ ?x a ?c }} FILTER(isIRI(?c)) }} GROUP BY ?c LIMIT 2000"),
        &plain,
    )?
    .iter()
    .filter_map(|r| Some((iri(&r[0])?, count(&r[1]))))
    // Vocabulary terms (owl:Class, owl:ObjectProperty…) are typed things too, not the ontology.
    .filter(|(c, _)| !is_meta(c))
    .collect();
    let subclass: Vec<(String, String)> = rows(
        target,
        &format!("{rdfs}SELECT DISTINCT ?c ?s WHERE {{ ?c rdfs:subClassOf ?s FILTER(isIRI(?c) && isIRI(?s) && ?c != ?s) }} LIMIT 5000"),
        &plain,
    )?
    .iter()
    .filter_map(|r| Some((iri(&r[0])?, iri(&r[1])?)))
    .collect();
    let properties: Vec<Value> = rows(
        target,
        &format!("{rdfs}SELECT DISTINCT ?p ?d ?r ?k WHERE {{ {{ ?p rdfs:domain ?d }} UNION {{ ?p rdfs:range ?r }} OPTIONAL {{ ?p rdfs:domain ?d }} OPTIONAL {{ ?p rdfs:range ?r }} OPTIONAL {{ ?p a ?k FILTER(?k IN (owl:ObjectProperty, owl:DatatypeProperty)) }} }} LIMIT 5000"),
        &plain,
    )?
    .iter()
    .filter_map(|r| {
        let range = iri(&r[2]);
        let datatype = iri(&r[3]).is_some_and(|k| k.ends_with("DatatypeProperty"))
            || range.as_deref().is_some_and(|x| x.starts_with("http://www.w3.org/2001/XMLSchema#") || x.ends_with("#Literal") || x.ends_with("langString"));
        Some(json!({"iri": iri(&r[0])?, "domain": iri(&r[1]), "range": range, "datatype": datatype}))
    })
    .collect();
    let subclass: Vec<(String, String)> = subclass
        .into_iter()
        .filter(|(c, s)| !is_meta(c) && !is_meta(s))
        .collect();
    Ok(json!({
        "classes": classes.iter().map(|(c, n)| json!({"iri": c, "instances": n})).collect::<Vec<_>>(),
        "subclass": subclass.iter().map(|(c, s)| json!([c, s])).collect::<Vec<_>>(),
        "properties": properties,
    }))
}

/// A term of the RDF, RDFS, OWL, XSD or SHACL vocabularies.
fn is_meta(iri: &str) -> bool {
    [
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
        "http://www.w3.org/2000/01/rdf-schema#",
        "http://www.w3.org/2002/07/owl#",
        "http://www.w3.org/2001/XMLSchema#",
        "http://www.w3.org/ns/shacl#",
    ]
    .iter()
    .any(|ns| iri.starts_with(ns))
}
