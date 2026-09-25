//! JSON forms of Datalog options and results, for the JavaScript bindings.
//!
// @lat: [[architecture#Datalog frontend#Execution]]

use crate::error::{DatalogError, Result};
use crate::exec::DatalogResult;
use crate::sql::Options;
use oxilite_core::json::term_to_json;
use serde_json::{json, Map, Value};

/// Reads `{ "useDefaultGraphAsUnion": bool, "includeInferred": bool, "maxIterations": number,
/// "producer": string }`.
///
/// The names match `CypherOptions`, so the two dialects read the same from JavaScript.
pub fn options_from_json(v: &Value) -> Result<Options> {
    let mut out = Options::default();
    let Some(map) = v.as_object() else {
        return if v.is_null() {
            Ok(out)
        } else {
            Err(DatalogError::unsupported(
                "datalog options must be an object",
            ))
        };
    };
    let flag = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_bool);
    if let Some(b) = flag(map, "useDefaultGraphAsUnion").or_else(|| flag(map, "unionDefaultGraph"))
    {
        out.union_default_graph = b;
    }
    if let Some(b) = flag(map, "includeInferred") {
        out.include_inferred = b;
    }
    if let Some(n) = map.get("maxIterations").and_then(Value::as_u64) {
        out.max_iterations = n as usize;
    }
    if let Some(p) = map.get("producer").and_then(Value::as_str) {
        out.producer = p.to_owned();
    }
    if let Some(v) = map.get("asOf").and_then(Value::as_str) {
        out.as_of = Some(v.to_owned());
    }
    if let Some(t) = map.get("asOfTick").and_then(Value::as_i64) {
        out.as_of_tick = Some(t);
    }
    Ok(out)
}

/// `{ "kind": "datalog", "columns": [...], "rows": [[term | null, …], …], "rounds": [...] }`.
///
/// Terms use the same JSON shape as SPARQL results, so a caller reads a Datalog row exactly
/// as it reads a query solution.
pub fn result_to_json(r: &DatalogResult) -> Value {
    json!({
        "kind": "datalog",
        "columns": r.variables,
        "rows": r
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| match c {
                        Some(t) => term_to_json(t),
                        None => Value::Null,
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>(),
        "rounds": r.rounds,
    })
}

/// `{ "kind": "datalogMaterialize", "inferred": n, "relations": n }`.
pub fn stats_to_json(s: &crate::MaterializeStats) -> Value {
    json!({
        "kind": "datalogMaterialize",
        "inferred": s.inferred,
        "relations": s.relations,
    })
}
