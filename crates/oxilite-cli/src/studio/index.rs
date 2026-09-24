//! The source-position index: where each IRI occurs in the workspace's Turtle-family files, and
//! in which role. It drives go-to-definition, references, hover locations and the placement of
//! SHACL diagnostics on the triple that caused them.
//!
// @lat: [[architecture#Studio server#Source index]]

use super::scanner::{scan, Kind, Pos, Role};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy)]
struct Occ {
    file: u32,
    start: Pos,
    end: Pos,
    role: Role,
}

/// An occurrence handed out to callers.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub uri: String,
    pub start: Pos,
    pub end: Pos,
    pub role: Role,
}

#[derive(Default, Clone)]
pub struct SourceIndex {
    files: Vec<String>,
    file_ids: HashMap<String, u32>,
    by_iri: HashMap<String, Vec<Occ>>,
    /// Workspace prefixes: prefix to namespace, first declaration wins.
    prefixes: BTreeMap<String, String>,
    /// Per file, its prefixes (so removing a file can rebuild the workspace table).
    file_prefixes: HashMap<u32, BTreeMap<String, String>>,
}

impl SourceIndex {
    /// Indexes (or re-indexes) one file's text.
    pub fn set_file(&mut self, uri: &str, text: &str) {
        self.remove_file(uri);
        let id = match self.file_ids.get(uri) {
            Some(id) => *id,
            None => {
                let id = self.files.len() as u32;
                self.files.push(uri.to_string());
                self.file_ids.insert(uri.to_string(), id);
                id
            }
        };
        let s = scan(text, Some(uri));
        for t in &s.tokens {
            if !matches!(t.kind, Kind::Iri(_) | Kind::PName { .. }) {
                continue;
            }
            if let Some(iri) = &t.iri {
                self.by_iri.entry(iri.clone()).or_default().push(Occ {
                    file: id,
                    start: t.start,
                    end: t.end,
                    role: t.role,
                });
            }
        }
        let prefixes: BTreeMap<String, String> =
            s.prefixes.into_iter().map(|(p, (ns, _))| (p, ns)).collect();
        for (p, ns) in &prefixes {
            self.prefixes.entry(p.clone()).or_insert_with(|| ns.clone());
        }
        self.file_prefixes.insert(id, prefixes);
    }

    pub fn remove_file(&mut self, uri: &str) {
        let Some(&id) = self.file_ids.get(uri) else {
            return;
        };
        self.by_iri.retain(|_, occs| {
            occs.retain(|o| o.file != id);
            !occs.is_empty()
        });
        if self.file_prefixes.remove(&id).is_some() {
            self.prefixes.clear();
            let mut ids: Vec<_> = self.file_prefixes.keys().copied().collect();
            ids.sort_unstable();
            for i in ids {
                for (p, ns) in &self.file_prefixes[&i] {
                    self.prefixes.entry(p.clone()).or_insert_with(|| ns.clone());
                }
            }
        }
    }

    fn location(&self, o: &Occ) -> Location {
        Location {
            uri: self.files[o.file as usize].clone(),
            start: o.start,
            end: o.end,
            role: o.role,
        }
    }

    /// Where the IRI is described: its occurrences as a subject, else all occurrences.
    pub fn definitions(&self, iri: &str) -> Vec<Location> {
        let Some(occs) = self.by_iri.get(iri) else {
            return Vec::new();
        };
        let subjects: Vec<_> = occs
            .iter()
            .filter(|o| o.role == Role::Subject)
            .map(|o| self.location(o))
            .collect();
        if subjects.is_empty() {
            occs.iter().map(|o| self.location(o)).collect()
        } else {
            subjects
        }
    }

    pub fn references(&self, iri: &str) -> Vec<Location> {
        self.by_iri
            .get(iri)
            .map(|occs| occs.iter().map(|o| self.location(o)).collect())
            .unwrap_or_default()
    }

    /// The best place to report something about `subject`, preferring a line that also mentions
    /// `predicate` as a predicate after it in the same file.
    pub fn triple_location(&self, subject: &str, predicate: Option<&str>) -> Option<Location> {
        let subject_at = self.definitions(subject);
        let first = subject_at.first()?.clone();
        if let Some(p) = predicate {
            for s in &subject_at {
                let next = self
                    .by_iri
                    .get(p)
                    .into_iter()
                    .flatten()
                    .filter(|o| {
                        o.role == Role::Predicate
                            && self.files[o.file as usize] == s.uri
                            && o.start >= s.start
                    })
                    .min_by_key(|o| o.start);
                if let Some(o) = next {
                    return Some(self.location(o));
                }
            }
        }
        Some(first)
    }

    pub fn prefixes(&self) -> &BTreeMap<String, String> {
        &self.prefixes
    }

    /// IRIs seen in the workspace that start with `namespace` (and occur in `role`, if given),
    /// with how often.
    pub fn iris_in(&self, namespace: &str, role: Option<Role>) -> Vec<(String, usize)> {
        let mut v: Vec<_> = self
            .by_iri
            .iter()
            .filter(|(iri, occs)| {
                iri.starts_with(namespace) && role.is_none_or(|r| occs.iter().any(|o| o.role == r))
            })
            .map(|(iri, occs)| (iri.clone(), occs.len()))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[tests#Studio server#Index finds definitions]]
    #[test]
    fn definitions_prefer_subjects() {
        let mut ix = SourceIndex::default();
        ix.set_file(
            "file:///a.ttl",
            "@prefix ex: <http://ex.org/> .\nex:alice ex:knows ex:bob .\n",
        );
        ix.set_file(
            "file:///b.ttl",
            "@prefix ex: <http://ex.org/> .\n\nex:bob ex:name \"Bob\" .\n",
        );
        let d = ix.definitions("http://ex.org/bob");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].uri, "file:///b.ttl");
        assert_eq!(d[0].start, Pos { line: 2, col: 0 });
        assert_eq!(ix.references("http://ex.org/bob").len(), 2);
        let at = ix
            .triple_location("http://ex.org/bob", Some("http://ex.org/name"))
            .unwrap();
        assert_eq!(at.start, Pos { line: 2, col: 7 });
        ix.remove_file("file:///b.ttl");
        assert_eq!(ix.definitions("http://ex.org/bob")[0].uri, "file:///a.ttl");
        assert_eq!(ix.prefixes()["ex"], "http://ex.org/");
    }
}
