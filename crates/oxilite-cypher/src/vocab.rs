//! The vocabulary mapping property-graph names (labels, relationship types, property keys)
//! to IRIs, and back.
//!
// @lat: [[architecture#Property graph frontend#Mapping]]

use oxrdf::{GraphName, NamedNode};
use std::collections::HashMap;

/// How names map to IRIs.
///
/// A name resolves, in order: through an explicit override; as itself when it is an absolute
/// IRI (backtick-quoted in Cypher, e.g. `` :`http://schema.org/Person` ``); through a
/// registered prefix (`` :`schema:Person` ``); otherwise by appending it to `base`. IRIs map
/// back the same way, so names round-trip.
#[derive(Debug, Clone)]
pub struct Vocabulary {
    pub base: String,
    prefixes: Vec<(String, String)>,
    overrides: HashMap<String, String>,
    reverse: HashMap<String, String>,
    /// Prefix of the IRIs minted for nodes created by Cypher.
    pub node_prefix: String,
    /// Prefix of the IRIs minted for reifiers of created relationships.
    pub relationship_prefix: String,
}

impl Default for Vocabulary {
    fn default() -> Self {
        Self::new("urn:oxilite:pg:")
    }
}

impl Vocabulary {
    pub fn new(base: impl Into<String>) -> Self {
        let mut v = Self {
            base: base.into(),
            prefixes: Vec::new(),
            overrides: HashMap::new(),
            reverse: HashMap::new(),
            node_prefix: "urn:oxilite:node:".into(),
            relationship_prefix: "urn:oxilite:rel:".into(),
        };
        for (p, ns) in [
            ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
            ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
            ("owl", "http://www.w3.org/2002/07/owl#"),
            ("xsd", "http://www.w3.org/2001/XMLSchema#"),
            ("sh", "http://www.w3.org/ns/shacl#"),
        ] {
            v.prefixes.push((p.into(), ns.into()));
        }
        v
    }

    /// Registers a prefix usable in names (`prefix:local`).
    pub fn with_prefix(mut self, prefix: impl Into<String>, namespace: impl Into<String>) -> Self {
        let prefix = prefix.into();
        self.prefixes.retain(|(p, _)| *p != prefix);
        self.prefixes.push((prefix, namespace.into()));
        self
    }

    /// Maps one name to an explicit IRI (both directions).
    pub fn with_name(mut self, name: impl Into<String>, iri: impl Into<String>) -> Self {
        let (name, iri) = (name.into(), iri.into());
        self.reverse.insert(iri.clone(), name.clone());
        self.overrides.insert(name, iri);
        self
    }

    pub fn iri(&self, name: &str) -> NamedNode {
        if let Some(iri) = self.overrides.get(name) {
            return NamedNode::new_unchecked(iri.clone());
        }
        if is_absolute(name) {
            return NamedNode::new_unchecked(name);
        }
        if let Some((p, local)) = name.split_once(':') {
            if let Some((_, ns)) = self.prefixes.iter().find(|(x, _)| x == p) {
                return NamedNode::new_unchecked(format!("{ns}{local}"));
            }
        }
        NamedNode::new_unchecked(format!("{}{}", self.base, encode_local(name)))
    }

    pub fn name(&self, iri: &str) -> String {
        if let Some(n) = self.reverse.get(iri) {
            return n.clone();
        }
        if let Some(local) = iri.strip_prefix(self.base.as_str()) {
            if !local.is_empty() {
                return decode_local(local);
            }
        }
        // The longest matching namespace wins.
        let mut best: Option<(&str, &str)> = None;
        for (p, ns) in &self.prefixes {
            if let Some(local) = iri.strip_prefix(ns.as_str()) {
                if best.is_none_or(|(_, b)| ns.len() > b.len()) && !local.is_empty() {
                    best = Some((p, ns));
                }
            }
        }
        match best {
            Some((p, ns)) => format!("{p}:{}", &iri[ns.len()..]),
            None => iri.to_string(),
        }
    }

    pub(crate) fn graph(&self) -> GraphName {
        GraphName::DefaultGraph
    }
}

fn is_absolute(name: &str) -> bool {
    name.contains("://") || name.starts_with("urn:")
}

/// Names may contain characters that are not allowed in IRIs (spaces, quotes…).
fn encode_local(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '~') {
            out.push(c);
        } else {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

fn decode_local(local: &str) -> String {
    if !local.contains('%') {
        return local.to_string();
    }
    let bytes = local.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = local
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| local.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let v = Vocabulary::new("http://ex/").with_prefix("schema", "http://schema.org/");
        for n in [
            "Person",
            "has space",
            "schema:Person",
            "http://other/X",
            "rdf:type",
        ] {
            assert_eq!(v.name(v.iri(n).as_str()), n, "{n}");
        }
        assert_eq!(v.iri("Person").as_str(), "http://ex/Person");
    }
}
