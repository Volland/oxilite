//! Vector index requests: list, create, drop and search the indexes of a connection, and list
//! the host functions its store can call.
//!
// @lat: [[architecture#Studio server#Vector indexes]]

use super::conn::Target;
use oxilite::model::NamedNode;
use oxilite::vector::{ElementType, Metric, QueryVector, VectorIndex, VectorIndexInfo};
use serde_json::{json, Value};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn info_json(i: &VectorIndexInfo) -> Value {
    json!({
        "name": i.index.name,
        "iri": i.index.iri().as_str(),
        "property": i.index.property.as_str(),
        "dimensions": i.index.dimensions,
        "metric": i.index.metric.name(),
        "elementType": i.index.element_type.name(),
        "class": i.index.class.as_ref().map(NamedNode::as_str),
        "rows": i.rows,
        "built": i.built,
    })
}

/// `oxilite/vectorIndexes`: `{ supported, indexes }`.
pub fn list(target: &Target) -> Result<Value> {
    if !target.store.supports_vectors() {
        return Ok(json!({ "supported": false, "indexes": [] }));
    }
    let indexes: Vec<Value> = target
        .store
        .vector_indexes()?
        .iter()
        .map(info_json)
        .collect();
    Ok(json!({ "supported": true, "indexes": indexes }))
}

fn string<'a>(p: &'a Value, key: &str, request: &str) -> Result<&'a str> {
    p[key]
        .as_str()
        .ok_or_else(|| format!("{request} needs a `{key}` string").into())
}

/// `oxilite/vectorIndexCreate`: the created index, as listed.
pub fn create(target: &Target, p: &Value) -> Result<Value> {
    const R: &str = "oxilite/vectorIndexCreate";
    if target.read_only {
        return Err("the connection is read-only".into());
    }
    let name = string(p, "name", R)?;
    let property = NamedNode::new(string(p, "property", R)?)?;
    let dimensions = p["dimensions"]
        .as_u64()
        .and_then(|d| u32::try_from(d).ok())
        .ok_or("oxilite/vectorIndexCreate needs `dimensions`, a positive integer")?;
    let mut index = VectorIndex::new(name, property, dimensions);
    if let Some(m) = p["metric"].as_str() {
        index.metric = Metric::parse(m).ok_or_else(|| format!("unknown metric {m}"))?;
        if index.metric == Metric::Jaccard {
            index.element_type = ElementType::SparseFloat32;
        }
    }
    if let Some(t) = p["elementType"].as_str() {
        index.element_type =
            ElementType::parse(t).ok_or_else(|| format!("unknown element type {t}"))?;
    }
    if let Some(c) = p["class"].as_str() {
        index.class = Some(NamedNode::new(c)?);
    }
    target.store.create_vector_index(&index)?;
    let created = target
        .store
        .vector_indexes()?
        .into_iter()
        .find(|i| i.index.name == index.name)
        .ok_or("the index was not created")?;
    Ok(info_json(&created))
}

/// `oxilite/vectorIndexDrop`: `{ dropped }`.
pub fn drop(target: &Target, p: &Value) -> Result<Value> {
    if target.read_only {
        return Err("the connection is read-only".into());
    }
    let name = string(p, "name", "oxilite/vectorIndexDrop")?;
    Ok(json!({ "dropped": target.store.drop_vector_index(name)? }))
}

/// `oxilite/vectorSearch`: `{ hits: [{ node, distance, score }], elapsedMs }`.
pub fn search(target: &Target, p: &Value) -> Result<Value> {
    let index = string(p, "index", "oxilite/vectorSearch")?;
    let query = match (&p["vector"], p["node"].as_str()) {
        (Value::Array(items), _) => QueryVector::vector(
            &items
                .iter()
                .map(|v| v.as_f64().ok_or("`vector` must be numbers"))
                .collect::<std::result::Result<Vec<f64>, _>>()?,
        ),
        (Value::String(text), _) => QueryVector::Vector(text.clone()),
        (_, Some(node)) => QueryVector::node(NamedNode::new(node)?),
        _ => return Err("oxilite/vectorSearch needs a `vector` or a `node`".into()),
    };
    let k = p["k"].as_u64().unwrap_or(oxilite::core::vector::DEFAULT_K);
    let start = Instant::now();
    let hits = target.store.vector_search(index, &query, k)?;
    Ok(json!({
        "hits": hits.iter().map(|h| json!({
            "node": oxilite_core::json::term_to_json(&h.node),
            "distance": h.distance,
            "score": h.score,
        })).collect::<Vec<_>>(),
        "elapsedMs": start.elapsed().as_secs_f64() * 1000.0,
    }))
}

/// `oxilite/functions`: the host functions the connection's store can call.
pub fn functions(target: &Target) -> Result<Value> {
    Ok(json!({
        "functions": target.store.functions().iter().map(|f| json!({
            "iri": f.iri(),
            "cypherName": f.cypher(),
            "minArity": f.min_arity(),
            "maxArity": f.max_arity(),
            "description": f.description_text(),
        })).collect::<Vec<_>>()
    }))
}
