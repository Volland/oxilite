//! How documents are keyed and where their RDF goes.
//!
// @lat: [[architecture#JSON-LD documents#Keys and graphs]]

use crate::error::{JsonLdError, Result};
use json_syntax::Value;
use oxrdf::{GraphName, NamedNode};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::Arc;

/// Prefix of content-hash keys: `urn:oxilite:doc:sha256:<hex>`.
pub const CONTENT_HASH_PREFIX: &str = "urn:oxilite:doc:sha256:";

/// Where a document's key comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KeyStrategy {
    /// The top-level `@id` or `id` (the default; a credential's id).
    #[default]
    Id,
    /// A JSON Pointer (RFC 6901) to a string, e.g. `/credentialSubject/id`.
    Pointer(String),
    /// `urn:oxilite:doc:sha256:<hex>` of the document bytes.
    ContentHash,
    /// The caller passes the key with every write.
    Explicit,
}

/// What happens when the key strategy finds no key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingKey {
    /// Fail with [`JsonLdError::MissingKey`] (the generic default).
    #[default]
    Reject,
    /// Use the content-hash key (the credentials default).
    ContentHash,
}

/// Which graph a document's default-graph triples go to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GraphStrategy {
    /// The key itself, as a named graph IRI (the default).
    #[default]
    Key,
    /// An IRI template; `{key}` is replaced by the percent-encoded key.
    Template(String),
    /// One named graph shared by all documents.
    Fixed(NamedNode),
    /// The default graph.
    DefaultGraph,
}

impl GraphStrategy {
    /// Does each document get a graph of its own? (Then replace and remove clear it whole.)
    pub fn owns_target(&self) -> bool {
        matches!(self, Self::Key | Self::Template(_))
    }
}

/// How `@direction` is represented in RDF (JSON-LD `rdfDirection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdfDirection {
    /// `https://www.w3.org/ns/i18n#{lang}_{dir}` datatypes.
    I18nDatatype,
    /// `rdf:CompoundLiteral` nodes.
    CompoundLiteral,
}

/// JSON-LD processing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcessingMode {
    /// JSON-LD 1.0 (rejects 1.1 features).
    JsonLd10,
    /// JSON-LD 1.1 (the default).
    #[default]
    JsonLd11,
}

/// Fetches a remote context's JSON by IRI (network access, native only; see `http_fetcher`).
pub type ContextFetcher = Arc<dyn Fn(&str) -> Result<String> + Send + Sync>;

/// Which document metadata columns are indexed (the columns are always filled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataIndexes {
    pub issuer: bool,
    pub subject: bool,
    pub valid_until: bool,
}

impl Default for MetadataIndexes {
    fn default() -> Self {
        Self {
            issuer: true,
            subject: true,
            valid_until: true,
        }
    }
}

impl MetadataIndexes {
    /// No metadata index (cheapest writes on D1).
    pub fn none() -> Self {
        Self {
            issuer: false,
            subject: false,
            valid_until: false,
        }
    }
}

/// Options of a JSON-LD document handle.
#[derive(Clone, Default)]
pub struct JsonLdOptions {
    pub key: KeyStrategy,
    pub on_missing_key: MissingKey,
    pub graph: GraphStrategy,
    /// Base IRI for relative IRIs in documents.
    pub base_iri: Option<String>,
    pub rdf_direction: Option<RdfDirection>,
    pub processing_mode: ProcessingMode,
    /// Contexts available without loading: IRI → JSON text.
    pub contexts: BTreeMap<String, String>,
    /// Fetches unknown contexts (off by default: no network access).
    pub fetcher: Option<ContextFetcher>,
    /// Persist fetched contexts in `jsonld_contexts`, so later loads need no network.
    pub cache_fetched: bool,
    /// Metadata indexes created with the schema.
    pub indexes: MetadataIndexes,
}

impl std::fmt::Debug for JsonLdOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonLdOptions")
            .field("key", &self.key)
            .field("on_missing_key", &self.on_missing_key)
            .field("graph", &self.graph)
            .field("base_iri", &self.base_iri)
            .field("rdf_direction", &self.rdf_direction)
            .field("processing_mode", &self.processing_mode)
            .field("contexts", &self.contexts.keys().collect::<Vec<_>>())
            .field("fetcher", &self.fetcher.is_some())
            .field("cache_fetched", &self.cache_fetched)
            .field("indexes", &self.indexes)
            .finish()
    }
}

impl JsonLdOptions {
    /// Registers a context in memory (IRI → JSON text).
    pub fn with_context(mut self, iri: impl Into<String>, json: impl Into<String>) -> Self {
        self.contexts.insert(iri.into(), json.into());
        self
    }

    /// Resolves a document's key.
    pub fn key_for(&self, raw: &str, doc: &Value, explicit: Option<&str>) -> Result<String> {
        if let Some(k) = explicit {
            return Ok(k.to_owned());
        }
        let found = match &self.key {
            KeyStrategy::Id => top_level_id(doc),
            KeyStrategy::Pointer(p) => pointer(doc, p).and_then(|v| v.as_str().map(str::to_owned)),
            KeyStrategy::ContentHash => return Ok(content_hash_key(raw)),
            KeyStrategy::Explicit => None,
        };
        match found {
            Some(k) if !k.is_empty() => Ok(k),
            _ => match self.on_missing_key {
                MissingKey::ContentHash => Ok(content_hash_key(raw)),
                MissingKey::Reject => Err(JsonLdError::MissingKey(match &self.key {
                    KeyStrategy::Id => "no top-level @id or id".into(),
                    KeyStrategy::Pointer(p) => format!("no string at {p}"),
                    _ => "no key was passed".into(),
                })),
            },
        }
    }

    /// The graph a document with this key writes to.
    pub fn graph_for(&self, key: &str) -> Result<GraphName> {
        Ok(match &self.graph {
            GraphStrategy::Key => GraphName::NamedNode(parse_iri(key)?),
            GraphStrategy::Template(t) => {
                GraphName::NamedNode(parse_iri(&t.replace("{key}", &percent_encode(key)))?)
            }
            GraphStrategy::Fixed(g) => GraphName::NamedNode(g.clone()),
            GraphStrategy::DefaultGraph => GraphName::DefaultGraph,
        })
    }
}

fn parse_iri(s: &str) -> Result<NamedNode> {
    NamedNode::new(s).map_err(|e| JsonLdError::InvalidGraphName(format!("{s}: {e}")))
}

/// The top-level `@id`/`id` of a document (a string).
pub fn top_level_id(doc: &Value) -> Option<String> {
    let o = doc.as_object()?;
    for k in ["@id", "id"] {
        if let Some(v) = o.get_unique(k).ok().flatten() {
            return v.as_str().map(str::to_owned);
        }
    }
    None
}

/// Resolves a JSON Pointer (RFC 6901).
pub fn pointer<'a>(doc: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        return Some(doc);
    }
    let mut cur = doc;
    for token in pointer.strip_prefix('/')?.split('/') {
        let token = token.replace("~1", "/").replace("~0", "~");
        cur = match cur {
            Value::Object(o) => o.get_unique(token.as_str()).ok().flatten()?,
            Value::Array(a) => a.get(token.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// SHA-256 of the document bytes.
pub fn sha256(raw: &str) -> [u8; 32] {
    Sha256::digest(raw.as_bytes()).into()
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// `urn:oxilite:doc:sha256:<hex>` of the document bytes.
pub fn content_hash_key(raw: &str) -> String {
    format!("{CONTENT_HASH_PREFIX}{}", hex(&sha256(raw)))
}

/// Percent-encodes everything but RFC 3986 `pchar`s and `/`.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use json_syntax::Parse;

    fn parse(s: &str) -> Value {
        Value::parse_str(s).unwrap().0
    }

    #[test]
    fn keys() {
        let raw =
            r#"{"id": "https://example.org/docs/1", "credentialSubject": {"id": "did:ex:1"}}"#;
        let doc = parse(raw);
        let o = JsonLdOptions::default();
        assert_eq!(
            o.key_for(raw, &doc, None).unwrap(),
            "https://example.org/docs/1"
        );
        let o = JsonLdOptions {
            key: KeyStrategy::Pointer("/credentialSubject/id".into()),
            ..Default::default()
        };
        assert_eq!(o.key_for(raw, &doc, None).unwrap(), "did:ex:1");
        let o = JsonLdOptions {
            key: KeyStrategy::Pointer("/nope".into()),
            ..Default::default()
        };
        assert!(matches!(
            o.key_for(raw, &doc, None),
            Err(JsonLdError::MissingKey(_))
        ));
    }

    #[test]
    fn templates() {
        let o = JsonLdOptions {
            graph: GraphStrategy::Template("https://example.org/g/{key}".into()),
            ..Default::default()
        };
        assert_eq!(
            o.graph_for("a b").unwrap(),
            GraphName::NamedNode(NamedNode::new_unchecked("https://example.org/g/a%20b"))
        );
        assert!(matches!(
            JsonLdOptions::default().graph_for("not an iri"),
            Err(JsonLdError::InvalidGraphName(_))
        ));
    }
}
