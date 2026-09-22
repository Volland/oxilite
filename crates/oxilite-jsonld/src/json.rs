//! JSON forms of options, filters and stored documents for the JavaScript bindings
//! (`@oxilite/node`, `@oxilite/d1`).
//!
//! Options: `{"key": "id" | "contentHash" | "explicit" | {"pointer": "/a/b"},
//! "onMissingKey": "reject" | "contentHash", "graph": "key" | "default" | {"template": "…{key}"}
//! | {"fixed": "iri"}, "baseIri", "rdfDirection": "i18n-datatype" | "compound-literal",
//! "processingMode": "json-ld-1.0" | "json-ld-1.1", "contexts": {iri: object | string},
//! "cacheFetched", "indexes": {"issuer", "subject", "validUntil"}}`.
//!
// @lat: [[architecture#Bindings]]

use crate::error::{JsonLdError, Result};
use crate::jobs::{
    DocumentFilter, DocumentInput, DocumentMeta, Drift, StoredDocument, WriteOutcome,
};
use crate::options::{
    GraphStrategy, JsonLdOptions, KeyStrategy, MetadataIndexes, MissingKey, ProcessingMode,
    RdfDirection,
};
use oxrdf::{GraphName, NamedNode};
use serde_json::{json, Value};

fn bad(msg: impl Into<String>) -> JsonLdError {
    JsonLdError::Invalid(msg.into())
}

/// Options from their JSON form (missing fields keep `base`'s values).
pub fn options_from_json(v: &Value, base: JsonLdOptions) -> Result<JsonLdOptions> {
    let mut o = base;
    if v.is_null() {
        return Ok(o);
    }
    let Some(obj) = v.as_object() else {
        return Err(bad("options must be an object"));
    };
    for (k, v) in obj {
        match k.as_str() {
            "key" => {
                o.key = match v {
                    Value::String(s) if s == "id" => KeyStrategy::Id,
                    Value::String(s) if s == "contentHash" => KeyStrategy::ContentHash,
                    Value::String(s) if s == "explicit" => KeyStrategy::Explicit,
                    Value::Object(p) if p.get("pointer").is_some_and(Value::is_string) => {
                        KeyStrategy::Pointer(p["pointer"].as_str().unwrap_or_default().into())
                    }
                    _ => return Err(bad(format!("invalid key strategy {v}"))),
                }
            }
            "onMissingKey" => {
                o.on_missing_key = match v.as_str() {
                    Some("reject") => MissingKey::Reject,
                    Some("contentHash") => MissingKey::ContentHash,
                    _ => return Err(bad(format!("invalid onMissingKey {v}"))),
                }
            }
            "graph" => {
                o.graph = match v {
                    Value::String(s) if s == "key" => GraphStrategy::Key,
                    Value::String(s) if s == "default" => GraphStrategy::DefaultGraph,
                    Value::Object(t) if t.get("template").is_some_and(Value::is_string) => {
                        GraphStrategy::Template(t["template"].as_str().unwrap_or_default().into())
                    }
                    Value::Object(t) if t.get("fixed").is_some_and(Value::is_string) => {
                        let iri = t["fixed"].as_str().unwrap_or_default();
                        GraphStrategy::Fixed(
                            NamedNode::new(iri).map_err(|e| {
                                JsonLdError::InvalidGraphName(format!("{iri}: {e}"))
                            })?,
                        )
                    }
                    _ => return Err(bad(format!("invalid graph strategy {v}"))),
                }
            }
            "baseIri" => o.base_iri = v.as_str().map(str::to_owned),
            "rdfDirection" => {
                o.rdf_direction = match v.as_str() {
                    None => None,
                    Some("i18n-datatype") => Some(RdfDirection::I18nDatatype),
                    Some("compound-literal") => Some(RdfDirection::CompoundLiteral),
                    Some(d) => return Err(bad(format!("invalid rdfDirection {d}"))),
                }
            }
            "processingMode" => {
                o.processing_mode = match v.as_str() {
                    Some("json-ld-1.0") => ProcessingMode::JsonLd10,
                    Some("json-ld-1.1") | None => ProcessingMode::JsonLd11,
                    Some(m) => return Err(bad(format!("invalid processingMode {m}"))),
                }
            }
            "contexts" => {
                for (iri, ctx) in v.as_object().into_iter().flatten() {
                    let text = match ctx {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    o.contexts.insert(iri.clone(), text);
                }
            }
            "cacheFetched" => o.cache_fetched = v.as_bool().unwrap_or(false),
            "indexes" => {
                let flag = |name: &str, d: bool| v.get(name).and_then(Value::as_bool).unwrap_or(d);
                o.indexes = MetadataIndexes {
                    issuer: flag("issuer", true),
                    subject: flag("subject", true),
                    valid_until: flag("validUntil", true),
                };
            }
            // Read by the bindings themselves.
            "network" | "embedCredentials" | "credentials" => {}
            other => return Err(bad(format!("unknown option {other}"))),
        }
    }
    Ok(o)
}

/// A metadata filter from `{"issuer", "subject", "type", "validAt" (epoch seconds),
/// "profile", "after", "limit"}`.
pub fn filter_from_json(v: &Value) -> Result<DocumentFilter> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Ok(DocumentFilter {
        issuer: s("issuer"),
        subject: s("subject"),
        type_: s("type"),
        valid_at: v.get("validAt").and_then(Value::as_f64),
        profile: s("profile"),
        after: s("after"),
        limit: v
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(100, |l| l as usize),
    })
}

/// Documents to write from `[{"json": "…", "key"?: "…"}]`.
pub fn inputs_from_json(v: &Value) -> Result<Vec<DocumentInput>> {
    v.as_array()
        .ok_or_else(|| bad("documents must be an array"))?
        .iter()
        .map(|d| {
            Ok(DocumentInput {
                json: d
                    .get("json")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad("each document needs a `json` string"))?
                    .to_owned(),
                key: d.get("key").and_then(Value::as_str).map(str::to_owned),
                meta: DocumentMeta::default(),
            })
        })
        .collect()
}

fn graph_json(g: &GraphName) -> Value {
    match g {
        GraphName::DefaultGraph => json!({"termType": "DefaultGraph", "value": ""}),
        GraphName::NamedNode(n) => json!({"termType": "NamedNode", "value": n.as_str()}),
        GraphName::BlankNode(b) => json!({"termType": "BlankNode", "value": b.as_str()}),
    }
}

/// A stored document as JSON (`graph` is an RDF/JS term; times are epoch seconds).
pub fn document_to_json(d: &StoredDocument) -> Value {
    json!({
        "key": d.key,
        "graph": graph_json(&d.graph),
        "json": d.json,
        "sha256": d.sha256,
        "profile": d.meta.profile,
        "issuer": d.meta.issuer,
        "subject": d.meta.subject,
        "types": d.meta.types,
        "validFrom": d.meta.valid_from,
        "validUntil": d.meta.valid_until,
        "refs": d.meta.refs,
        "storedAt": d.stored_at,
    })
}

/// A list of stored documents as JSON.
pub fn documents_to_json(docs: &[StoredDocument]) -> Value {
    Value::Array(docs.iter().map(document_to_json).collect())
}

/// Graph names as RDF/JS terms.
pub fn graphs_to_json(graphs: &[GraphName]) -> Value {
    Value::Array(graphs.iter().map(graph_json).collect())
}

/// A write outcome as JSON.
pub fn outcome_to_json(o: &WriteOutcome) -> Value {
    json!({"keys": o.keys, "removed": o.removed})
}

/// A drift report as JSON.
pub fn drifts_to_json(d: &[Drift]) -> Value {
    Value::Array(
        d.iter()
            .map(|d| json!({"key": d.key, "missing": d.missing, "extra": d.extra}))
            .collect(),
    )
}

/// An error as `{"code", "message"}` JSON: `code` is the JSON-LD error code or one of
/// `json`, `missing-key`, `invalid-graph-name`, `graph-owned`, `document-too-large`,
/// `invalid`, `store`.
pub fn error_to_json(e: &JsonLdError) -> Value {
    let code = match e {
        JsonLdError::Json(_) => "json",
        JsonLdError::JsonLd { code, .. } => code,
        JsonLdError::ContextNotFound(_) => "loading remote context failed",
        JsonLdError::MissingKey(_) => "missing-key",
        JsonLdError::InvalidGraphName(_) => "invalid-graph-name",
        JsonLdError::GraphOwned(_) => "graph-owned",
        JsonLdError::DocumentTooLarge { .. } => "document-too-large",
        JsonLdError::Invalid(_) => "invalid",
        JsonLdError::Store(_) => "store",
    };
    json!({"code": code, "message": e.to_string()})
}
