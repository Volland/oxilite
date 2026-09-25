//! JSON forms of Cypher options and parameters, shared by the JavaScript bindings.
//!
//! Options: `{"base": "http://ex/", "prefixes": {"schema": "http://schema.org/"},
//! "names": {"knows": "http://xmlns.com/foaf/0.1/knows"}, "multiValue": "list",
//! "varLengthCap": 10, "shortestPathCap": 32, "shapes": true, "nodeMarker": true,
//! "reasoning": "none" | "rdfs" | "owl-ql", "useDefaultGraphAsUnion": false}`.
//! Parameters: a JSON object whose values become Cypher values (numbers without a fraction
//! are integers).
//!
// @lat: [[architecture#Bindings]]

use crate::error::{CypherError, Result};
use crate::value::{Params, Value};
use crate::vocab::Vocabulary;
use crate::{CypherOptions, MultiValue};
use oxilite_core::reason::Reasoning;

fn bad(msg: impl Into<String>) -> CypherError {
    CypherError::semantic(msg)
}

/// Parses Cypher options from JSON (see the module documentation).
pub fn options_from_json(json: &str) -> Result<CypherOptions> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| bad(format!("invalid options JSON: {e}")))?;
    let o = v
        .as_object()
        .ok_or_else(|| bad("options must be a JSON object"))?;
    let mut out = CypherOptions::default();
    let s = |k: &str| o.get(k).and_then(|x| x.as_str());
    let n = |k: &str| {
        o.get(k)
            .and_then(serde_json::Value::as_u64)
            .map(|x| x as usize)
    };
    let b = |k: &str| o.get(k).and_then(serde_json::Value::as_bool);
    let mut vocab = match s("base") {
        Some(base) => Vocabulary::new(base),
        None => Vocabulary::default(),
    };
    if let Some(p) = o.get("prefixes").and_then(|x| x.as_object()) {
        for (k, ns) in p {
            let ns = ns
                .as_str()
                .ok_or_else(|| bad("prefix namespaces must be strings"))?;
            vocab = vocab.with_prefix(k.clone(), ns);
        }
    }
    if let Some(p) = o.get("names").and_then(|x| x.as_object()) {
        for (k, iri) in p {
            let iri = iri
                .as_str()
                .ok_or_else(|| bad("name IRIs must be strings"))?;
            vocab = vocab.with_name(k.clone(), iri);
        }
    }
    if let Some(p) = s("nodePrefix") {
        vocab.node_prefix = p.into();
    }
    if let Some(p) = s("relationshipPrefix") {
        vocab.relationship_prefix = p.into();
    }
    out.vocabulary = vocab;
    if let Some(m) = s("multiValue") {
        out.multi_value = match m {
            "list" => MultiValue::List,
            "first" => MultiValue::First,
            "error" => MultiValue::Error,
            other => return Err(bad(format!("unknown multiValue '{other}'"))),
        };
    }
    if let Some(x) = n("varLengthCap") {
        out.var_length_cap = x;
    }
    if let Some(x) = n("shortestPathCap") {
        out.shortest_path_cap = x;
    }
    if let Some(x) = n("maxBranches") {
        out.max_branches = x;
    }
    if let Some(x) = b("shapes") {
        out.shapes = x;
    }
    if let Some(x) = b("nodeMarker") {
        out.node_marker = x;
    }
    if let Some(x) = b("reifierUniqueness") {
        out.reifier_uniqueness = x;
    }
    if let Some(r) = s("reasoning") {
        out.query.reasoning = match r {
            "none" => Reasoning::None,
            "rdfs" => Reasoning::Rdfs,
            "owl-ql" | "owlql" => Reasoning::OwlQl,
            other => return Err(bad(format!("unknown reasoning '{other}'"))),
        };
    }
    // The store as it was at this version (`HEAD~1`, `#42`, `@…`); `asOfTick` is a resolved one.
    if let Some(v) = s("asOf") {
        out.query.as_of = Some(v.to_owned());
    }
    if let Some(t) = v.get("asOfTick").and_then(serde_json::Value::as_i64) {
        out.query.as_of_tick = Some(t);
    }
    if let Some(x) = b("useDefaultGraphAsUnion") {
        out.query.union_default_graph = x;
    }
    Ok(out)
}

/// Parses parameters from a JSON object.
pub fn params_from_json(json: &str) -> Result<Params> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| bad(format!("invalid parameters JSON: {e}")))?;
    let o = v
        .as_object()
        .ok_or_else(|| bad("parameters must be a JSON object"))?;
    Ok(o.iter()
        .map(|(k, v)| (k.clone(), Value::from_json(v)))
        .collect())
}
