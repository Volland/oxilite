//! The schema registry of a connection, for the studio's Schema Registry view: what oxilite's
//! reader sees in `<oxilite:schema>`, the graphs it maps, and edits through the registry API.
//!
// @lat: [[architecture#Studio server#Schema registry view]]

use super::conn::{NeedsConfirmation, Target};
use oxilite::model::{GraphName, NamedNode, Term};
use oxilite::schema::{vocab, RegisteredGraph, Registration, SchemaRole};
use oxilite::sparql::{QueryOptions, Reasoning, SparqlParser};
use oxilite_core::registry::{SCHEMA_GRAPH, SYSTEM_GRAPHS};
use oxilite_core::QueryOutput;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::str::FromStr;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const OWL_IMPORTS: &str = "http://www.w3.org/2002/07/owl#imports";

/// A graph name as the registry writes it: its IRI, or `oxl:DefaultGraph`.
pub fn graph_iri(g: &GraphName) -> String {
    match g {
        GraphName::NamedNode(n) => n.as_str().to_string(),
        GraphName::DefaultGraph => vocab::DEFAULT_GRAPH.to_string(),
        GraphName::BlankNode(b) => format!("_:{}", b.as_str()),
    }
}

fn graph_name(s: &str) -> Result<GraphName> {
    Ok(if s == vocab::DEFAULT_GRAPH {
        GraphName::DefaultGraph
    } else {
        NamedNode::new(s)?.into()
    })
}

/// Reads the stored triples plainly: no reasoning, no inferences, schema graphs visible.
fn plain(target: &Target) -> QueryOptions {
    QueryOptions {
        reasoning: Reasoning::None,
        include_inferred: false,
        union_default_graph: false,
        include_schema_graphs: true,
        ..target.options.clone()
    }
}

fn rows(target: &Target, text: &str) -> Result<Vec<Vec<Option<Term>>>> {
    match target
        .store
        .query_output(SparqlParser::new().parse_query(text)?, &plain(target))?
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

fn number(t: &Option<Term>) -> u64 {
    match t {
        Some(Term::Literal(l)) => l.value().parse().unwrap_or(0),
        _ => 0,
    }
}

fn entry_json(e: &RegisteredGraph) -> Value {
    let mut v = oxilite::core::json::schema_graph_to_json(&e.to_entry());
    v["graph"] = json!(graph_iri(&e.graph));
    v
}

/// Every graph of the store with its size; sizes are `null` on remote D1, where counting is a
/// billed scan. The default graph is listed when it holds triples.
fn graphs(target: &Target) -> Result<Vec<Value>> {
    let metered = target.store.meter().is_some();
    let mut out = Vec::new();
    if metered {
        for r in rows(
            target,
            "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } } ORDER BY ?g",
        )? {
            if let Some(g) = iri(&r[0]) {
                out.push(json!({"graph": g, "triples": null}));
            }
        }
        if !rows(target, "SELECT ?s WHERE { ?s ?p ?o } LIMIT 1")?.is_empty() {
            out.push(json!({"graph": vocab::DEFAULT_GRAPH, "triples": null}));
        }
        return Ok(out);
    }
    for r in rows(
        target,
        "SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g ORDER BY ?g",
    )? {
        if let Some(g) = iri(&r[0]) {
            out.push(json!({"graph": g, "triples": number(&r[1])}));
        }
    }
    let default = rows(target, "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")?
        .first()
        .map_or(0, |r| number(&r[0]));
    if default > 0 {
        out.push(json!({"graph": vocab::DEFAULT_GRAPH, "triples": default}));
    }
    Ok(out)
}

/// `owl:imports` asserted inside registered ontology graphs, which oxilite follows too.
fn own_imports(target: &Target) -> Result<Vec<Value>> {
    let text = format!(
        "SELECT DISTINCT ?g ?t WHERE {{ GRAPH <{SCHEMA_GRAPH}> {{ ?g a <{}> }} GRAPH ?g {{ ?o <{OWL_IMPORTS}> ?t }} }} ORDER BY ?g ?t",
        vocab::ONTOLOGY_GRAPH
    );
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in rows(target, &text)? {
        if let (Some(g), Some(t)) = (iri(&r[0]), iri(&r[1])) {
            by.entry(g).or_default().push(t);
        }
    }
    Ok(by
        .into_iter()
        .map(|(graph, imports)| json!({"graph": graph, "imports": imports}))
        .collect())
}

/// The payload of `oxilite/registry`.
pub fn read(target: &Target) -> Result<Value> {
    let entries = target.store.schema_graphs()?;
    let graphs = graphs(target)?;
    let present: Vec<&str> = SYSTEM_GRAPHS
        .iter()
        .copied()
        .filter(|s| graphs.iter().any(|g| g["graph"] == *s))
        .collect();
    Ok(json!({
        "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
        "graphs": graphs,
        "ownImports": own_imports(target)?,
        "problems": target.store.registry_problems()?,
        "systemGraphs": {
            "present": present,
            "current": target.store.system_graphs_installed()?,
        },
        "ephemeral": target.ephemeral,
        "readOnly": target.read_only,
    }))
}

fn targets(p: &Value) -> Result<Vec<GraphName>> {
    p["appliesTo"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .filter(|s| *s != vocab::ALL_GRAPHS)
        .map(graph_name)
        .collect()
}

/// Applies one `oxilite/registryEdit`. Like an update, an edit on a persistent store needs
/// `confirmed`, and a read-only connection refuses it.
pub fn edit(target: &Target, p: &Value) -> Result<Value> {
    let op = p["op"]
        .as_str()
        .ok_or("oxilite/registryEdit needs an `op`")?;
    if target.read_only {
        return Err("this connection is read-only".into());
    }
    if !target.ephemeral && !p["confirmed"].as_bool().unwrap_or(false) {
        return Err(Box::new(NeedsConfirmation { estimate: None }));
    }
    let store = &target.store;
    let graph = || -> Result<GraphName> {
        graph_name(
            p["graph"]
                .as_str()
                .ok_or_else(|| format!("registryEdit {op} needs a `graph`"))?,
        )
    };
    let role = || -> Result<SchemaRole> {
        Ok(SchemaRole::from_str(
            p["role"].as_str().unwrap_or("ontology"),
        )?)
    };
    let mut out = json!({"ephemeral": target.ephemeral});
    let changed = match op {
        "register" => {
            let registration = Registration::new().applies_to(targets(p)?);
            store.register_schema_graph(graph()?.as_ref(), role()?, &registration)?;
            true
        }
        // `register` replaces the whole description; a second role is one more type triple.
        "addRole" => {
            let g = graph()?;
            let role = role()?;
            if store.schema_graphs()?.iter().any(|e| e.graph == g) {
                let node = graph_iri(&g);
                let update = format!(
                    "INSERT DATA {{ GRAPH <{SCHEMA_GRAPH}> {{ <{node}> a <{}> }} }}",
                    role.class()
                );
                store.update(SparqlParser::new().parse_update(&update)?)?;
            } else {
                let registration = Registration::new().applies_to(targets(p)?);
                store.register_schema_graph(g.as_ref(), role, &registration)?;
            }
            true
        }
        "map" => store.set_schema_graph_targets(graph()?.as_ref(), &targets(p)?)?,
        "activate" => store.set_schema_graph_active(graph()?.as_ref(), true)?,
        "deactivate" => store.set_schema_graph_active(graph()?.as_ref(), false)?,
        "unregister" => store.unregister_schema_graph(graph()?.as_ref())?,
        "drop" => {
            let n = store.drop_schema_graph(graph()?.as_ref())?;
            out["dropped"] = json!(n);
            true
        }
        "installSystemGraphs" => store.install_system_graphs()?,
        other => return Err(format!("unknown registry edit {other}").into()),
    };
    out["changed"] = json!(changed);
    Ok(out)
}

/// For the Store Explorer: each registered graph's roles (`system` for the system graphs) and a
/// short description of its targets.
pub fn roles(
    target: &Target,
    short: impl Fn(&str) -> String,
) -> BTreeMap<String, (String, String)> {
    let mut out: BTreeMap<String, (Vec<&'static str>, String)> = BTreeMap::new();
    for e in target.store.schema_graphs().unwrap_or_default() {
        let to = if e.registration.applies_to.is_empty() {
            "all graphs".to_string()
        } else {
            e.registration
                .applies_to
                .iter()
                .map(|g| match g {
                    GraphName::DefaultGraph => "default graph".to_string(),
                    g => short(&graph_iri(g)),
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let to = if e.registration.active {
            format!("→ {to}")
        } else {
            format!("→ {to} (inactive)")
        };
        let slot = out.entry(graph_iri(&e.graph)).or_insert((Vec::new(), to));
        slot.0.push(match e.role {
            SchemaRole::Ontology => "ontology",
            SchemaRole::Shacl => "shapes",
            SchemaRole::Shex => "shex",
        });
    }
    let mut out: BTreeMap<String, (String, String)> = out
        .into_iter()
        .map(|(g, (r, to))| (g, (r.join(" + "), to)))
        .collect();
    for s in SYSTEM_GRAPHS {
        out.insert(s.to_string(), ("system".into(), String::new()));
    }
    out
}
