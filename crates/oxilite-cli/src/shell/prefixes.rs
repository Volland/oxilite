//! The session's prefix table: predeclared, learnt from statements and loaded files, added to
//! statements that use a prefix without declaring it, and used to compact IRIs in results.

use crate::studio::lang::WELL_KNOWN;
use crate::studio::scanner::{scan, Kind};
use std::collections::{BTreeMap, BTreeSet};

pub struct Prefixes {
    map: BTreeMap<String, String>,
}

impl Default for Prefixes {
    fn default() -> Self {
        let mut map: BTreeMap<String, String> = WELL_KNOWN
            .iter()
            .map(|(p, ns)| ((*p).to_string(), (*ns).to_string()))
            .collect();
        map.insert("oxl".into(), "https://oxilite.dev/ns#".into());
        Self { map }
    }
}

impl Prefixes {
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.map.iter()
    }

    pub fn get(&self, prefix: &str) -> Option<&String> {
        self.map.get(prefix)
    }

    pub fn insert(&mut self, prefix: impl Into<String>, namespace: impl Into<String>) {
        self.map.insert(prefix.into(), namespace.into());
    }

    pub fn remove(&mut self, prefix: &str) -> bool {
        self.map.remove(prefix).is_some()
    }

    /// Remembers the prefixes `text` declares (SPARQL `PREFIX` or Turtle `@prefix`).
    pub fn learn(&mut self, text: &str) {
        for (p, (ns, _)) in scan(text, None).prefixes {
            if !ns.is_empty() {
                self.map.insert(p, ns);
            }
        }
    }

    /// `text` with a declaration for every prefix it uses without declaring, on its first line
    /// so that error positions keep their line numbers.
    pub fn declare_missing(&self, text: &str) -> String {
        self.declare_missing_as(text, |p, ns| format!("PREFIX {p}: <{ns}> "))
    }

    /// The same for Datalog, whose declarations are Turtle's `@prefix`.
    pub fn declare_missing_datalog(&self, text: &str) -> String {
        self.declare_missing_as(text, |p, ns| format!("@prefix {p}: <{ns}> . "))
    }

    fn declare_missing_as(&self, text: &str, declare: impl Fn(&str, &str) -> String) -> String {
        let s = scan(text, None);
        let used: BTreeSet<String> = s
            .tokens
            .into_iter()
            .filter_map(|t| match t.kind {
                Kind::PName { prefix, .. } if !s.prefixes.contains_key(&prefix) => Some(prefix),
                _ => None,
            })
            .collect();
        let mut out = String::new();
        for p in used {
            if let Some(ns) = self.map.get(&p) {
                out.push_str(&declare(&p, ns));
            }
        }
        out.push_str(text);
        out
    }

    /// Every prefix as a SPARQL prologue, one declaration per line.
    pub fn prologue(&self) -> String {
        self.map
            .iter()
            .map(|(p, ns)| format!("PREFIX {p}: <{ns}>\n"))
            .collect()
    }

    /// `iri` as a prefixed name, with the longest matching namespace.
    pub fn compact(&self, iri: &str) -> Option<String> {
        self.map
            .iter()
            .filter(|(_, ns)| iri.starts_with(ns.as_str()) && valid_local(&iri[ns.len()..]))
            .max_by_key(|(_, ns)| ns.len())
            .map(|(p, ns)| format!("{p}:{}", &iri[ns.len()..]))
    }
}

/// A local name that needs no escaping in a prefixed name.
fn valid_local(local: &str) -> bool {
    local.chars().enumerate().all(|(i, c)| {
        c.is_alphanumeric() || c == '_' || (i > 0 && (c == '-' || c == '.')) || (c as u32) > 0x7F
    }) && !local.ends_with('.')
}
