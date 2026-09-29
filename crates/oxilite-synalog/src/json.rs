//! JSON forms of Synalog options and results, for the JavaScript and Python bindings.
//!
// @lat: [[architecture#Synalog frontend#Execution]]

use crate::error::{Result, SynalogError};
use crate::exec::SynalogResult;
use crate::tables::Table;
use crate::Options;
use oxilite_core::sql::SqlValue;
use serde_json::{json, Map, Value};

/// Reads `{ "useDefaultGraphAsUnion": bool, "includeInferred": bool, "limit": n, "offset": n,
/// "asOf": string, "tables": [{ "name", "predicate" } | { "name", "class" }] }`.
///
/// The scope names match the Datalog and Cypher options, so the dialects read the same.
pub fn options_from_json(v: &Value) -> Result<Options> {
    let mut out = Options::default();
    let Some(map) = v.as_object() else {
        return if v.is_null() {
            Ok(out)
        } else {
            Err(SynalogError::unsupported(
                "synalog options must be an object",
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
    out.limit = map.get("limit").and_then(Value::as_u64);
    out.offset = map.get("offset").and_then(Value::as_u64);
    if let Some(v) = map.get("asOf").and_then(Value::as_str) {
        out.as_of = Some(v.to_owned());
    }
    if let Some(t) = map.get("asOfTick").and_then(Value::as_i64) {
        out.as_of_tick = Some(t);
    }
    for t in map
        .get("tables")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |k: &str| t.get(k).and_then(Value::as_str).map(str::to_owned);
        let name = field("name")
            .ok_or_else(|| SynalogError::unsupported("a declared table needs a \"name\""))?;
        out.tables.push(match (field("predicate"), field("class")) {
            (Some(iri), None) => Table::Predicate { name, iri },
            (None, Some(iri)) => Table::Class { name, iri },
            _ => {
                return Err(SynalogError::unsupported(format!(
                    "table `{name}` needs exactly one of \"predicate\" or \"class\""
                )))
            }
        });
    }
    Ok(out)
}

/// `{ "kind": "synalog", "columns": [...], "rows": [[null | number | string, …], …] }`.
pub fn result_to_json(r: &SynalogResult) -> Value {
    json!({
        "kind": "synalog",
        "columns": r.columns,
        "rows": r
            .rows
            .iter()
            .map(|row| row.iter().map(value_to_json).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
    })
}

fn value_to_json(v: &SqlValue) -> Value {
    match v {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(i) => json!(i),
        SqlValue::Real(r) => serde_json::Number::from_f64(*r).map_or(Value::Null, Value::Number),
        SqlValue::Text(s) => json!(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_results_round_trip() {
        let o = options_from_json(&json!({
            "useDefaultGraphAsUnion": true, "limit": 10, "asOf": "HEAD~1",
            "tables": [{"name": "parent", "predicate": "http://ex.org/parent"},
                       {"name": "person", "class": "http://ex.org/Person"}]
        }))
        .unwrap();
        assert!(o.union_default_graph && !o.include_inferred);
        assert_eq!(
            (o.limit, o.as_of.as_deref(), o.tables.len()),
            (Some(10), Some("HEAD~1"), 2)
        );
        assert!(options_from_json(&json!({"tables": [{"name": "x"}]})).is_err());
        let r = SynalogResult {
            columns: vec!["a".into(), "b".into()],
            rows: vec![
                vec![SqlValue::Integer(3), SqlValue::Null],
                vec![SqlValue::Real(1.5), SqlValue::Text("x".into())],
            ],
        };
        assert_eq!(
            result_to_json(&r),
            json!({"kind": "synalog", "columns": ["a", "b"], "rows": [[3, null], [1.5, "x"]]})
        );
    }
}
