//! The project manifest (`oxilite.toml`) and the conventions used without one: which graph each
//! file loads into and what role it plays (data, ontology, shapes, rules).
//!
// @lat: [[architecture#Studio server#Project manifest]]

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::path::Path;

pub const MANIFEST: &str = "oxilite.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    #[default]
    None,
    Rdfs,
    Owlql,
    Owl2rl,
}

impl Profile {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "none" => Self::None,
            "rdfs" => Self::Rdfs,
            "owlql" | "owl-ql" => Self::Owlql,
            "owl2rl" | "owl-rl" | "owl2-rl" => Self::Owl2rl,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Rdfs => "rdfs",
            Self::Owlql => "owlql",
            Self::Owl2rl => "owl2rl",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    Data,
    Ontology,
    Shapes,
    Rules,
    /// A ShEx schema (ShExC).
    Shex,
    /// A ShEx shape map: which nodes to check against which shapes.
    ShapeMap,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Ontology => "ontology",
            Self::Shapes => "shapes",
            Self::Rules => "rules",
            Self::Shex => "shex",
            Self::ShapeMap => "shapemap",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphSpec {
    pub iri: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub role: Role,
    /// For an ontology graph: the graphs it applies to (every graph when empty).
    #[serde(default)]
    pub applies_to: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FilesSpec {
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValidationSpec {
    /// Validate asserted plus inferred triples (default) or asserted triples only.
    #[serde(default = "yes")]
    pub inferred: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl Default for ValidationSpec {
    fn default() -> Self {
        Self {
            inferred: true,
            enabled: true,
        }
    }
}

fn yes() -> bool {
    true
}

/// A knowledge-graph test (see `oxilite check`).
#[derive(Debug, Clone, Deserialize)]
pub struct TestSpec {
    pub name: String,
    /// A SPARQL (`.rq`), Datalog (`.dl`) or Cypher (`.cypher`) file whose results are compared.
    pub query: Option<String>,
    /// Expected results: SPARQL JSON/XML/CSV/TSV results, or an RDF file for graph results.
    pub expect: Option<String>,
    /// Fixture data loaded into an overlay for this test only.
    pub data: Option<String>,
    /// Shapes to validate against (default: the project's shapes).
    pub shapes: Option<String>,
    /// Shapes (IRIs, prefixed with `<>` or as-is) that must report violations; empty means
    /// the data must conform.
    pub expect_violations: Option<Vec<String>>,
    /// Rules to materialize for this test (default: the project's rules).
    pub rules: Option<String>,
    /// Triples that must be entailed, and triples that must not be.
    pub entails: Option<String>,
    pub not_entails: Option<String>,
    /// Reasoning profile for this test (default: the project's).
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub reasoning: Option<Profile>,
    #[serde(default, rename = "graph")]
    pub graphs: Vec<GraphSpec>,
    #[serde(default)]
    pub shapes: FilesSpec,
    #[serde(default)]
    pub rules: FilesSpec,
    #[serde(default)]
    pub validation: ValidationSpec,
    #[serde(default, rename = "test")]
    pub tests: Vec<TestSpec>,
    /// Build the full-text index over string literals (for `oxl:textMatch`).
    #[serde(default)]
    pub text_index: bool,
    #[serde(default)]
    pub cypher: CypherSpec,
    #[serde(default)]
    pub shex: Vec<ShexSpec>,
}

/// A ShEx schema and the shape map saying which nodes to validate against which shapes.
#[derive(Debug, Clone, Deserialize)]
pub struct ShexSpec {
    pub schema: String,
    /// A shape map file…
    pub shape_map: Option<String>,
    /// …or an inline one, e.g. `{FOCUS a <http://ex.org/Person>}@<http://ex.org/PersonShape>`.
    pub map: Option<String>,
}

/// How Cypher names map to IRIs: unprefixed labels, types and keys are appended to `base`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CypherSpec {
    pub base: Option<String>,
}

/// Where a file goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub role: Role,
    /// The graph IRI data and ontology files load into.
    pub graph: String,
}

/// The manifest compiled to glob matchers.
pub struct Layout {
    pub manifest: Option<Manifest>,
    graphs: Vec<(GlobSet, GraphSpec)>,
    shapes: GlobSet,
    rules: GlobSet,
}

fn globs(patterns: &[String]) -> Result<GlobSet, globset::Error> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(Glob::new(p)?);
    }
    b.build()
}

impl Layout {
    pub fn convention() -> Self {
        Self {
            manifest: None,
            graphs: Vec::new(),
            shapes: GlobSet::empty(),
            rules: GlobSet::empty(),
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        let manifest: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
        let graphs = manifest
            .graphs
            .iter()
            .map(|g| Ok((globs(&g.files).map_err(|e| e.to_string())?, g.clone())))
            .collect::<Result<_, String>>()?;
        Ok(Self {
            shapes: globs(&manifest.shapes.files).map_err(|e| e.to_string())?,
            rules: globs(&manifest.rules.files).map_err(|e| e.to_string())?,
            graphs,
            manifest: Some(manifest),
        })
    }

    /// The role and graph of a file (`relative` to the root), or `None` when it is not loaded.
    /// Without a manifest, the extension and the content decide.
    pub fn assign(&self, relative: &Path, file_iri: &str, content: &str) -> Option<Assignment> {
        if self.manifest.is_none() {
            return convention(relative, file_iri, content);
        }
        if self.shapes.is_match(relative) {
            return Some(Assignment {
                role: Role::Shapes,
                graph: file_iri.into(),
            });
        }
        if self.rules.is_match(relative) {
            return Some(Assignment {
                role: Role::Rules,
                graph: file_iri.into(),
            });
        }
        let m = self.manifest.as_ref().expect("a manifest");
        for x in &m.shex {
            if Path::new(&x.schema) == relative {
                return Some(Assignment {
                    role: Role::Shex,
                    graph: file_iri.into(),
                });
            }
            if x.shape_map
                .as_deref()
                .is_some_and(|f| Path::new(f) == relative)
            {
                return Some(Assignment {
                    role: Role::ShapeMap,
                    graph: file_iri.into(),
                });
            }
        }
        self.graphs
            .iter()
            .find(|(g, _)| g.is_match(relative))
            .map(|(_, spec)| Assignment {
                role: spec.role,
                graph: spec.iri.clone(),
            })
    }

    pub fn profile(&self) -> Option<Profile> {
        self.manifest.as_ref().and_then(|m| m.reasoning)
    }
}

const SH: &str = "http://www.w3.org/ns/shacl#";

fn convention(relative: &Path, file_iri: &str, content: &str) -> Option<Assignment> {
    let ext = relative.extension()?.to_str()?.to_ascii_lowercase();
    let role = match ext.as_str() {
        "dl" => Role::Rules,
        "shex" => Role::Shex,
        "sm" | "shapemap" => Role::ShapeMap,
        "ttl" | "nt" | "nq" | "trig" | "n3" | "rdf" | "owl" => {
            let shacl = (content.contains(SH) || content.contains("sh:"))
                && (content.contains("NodeShape") || content.contains("PropertyShape"));
            if shacl {
                Role::Shapes
            } else if content.contains("owl:Ontology")
                || content.contains("http://www.w3.org/2002/07/owl#Ontology")
                || ext == "owl"
            {
                Role::Ontology
            } else {
                Role::Data
            }
        }
        _ => return None,
    };
    Some(Assignment {
        role,
        graph: file_iri.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[tests#Studio server#Manifest assigns graphs and roles]]
    #[test]
    fn manifest_assigns_graphs_and_roles() {
        let l = Layout::from_toml(
            r#"
reasoning = "owl2rl"
[[graph]]
iri = "https://ex.org/g/people"
files = ["data/**/*.ttl"]
[[graph]]
iri = "https://ex.org/g/onto"
files = ["onto/*.ttl"]
role = "ontology"
[shapes]
files = ["shapes/*.ttl"]
[rules]
files = ["rules/*.dl"]
"#,
        )
        .unwrap();
        assert_eq!(l.profile(), Some(Profile::Owl2rl));
        let a = |p: &str| l.assign(Path::new(p), "file:///x", "");
        assert_eq!(a("data/a/b.ttl").unwrap().graph, "https://ex.org/g/people");
        assert_eq!(a("onto/o.ttl").unwrap().role, Role::Ontology);
        assert_eq!(a("shapes/s.ttl").unwrap().role, Role::Shapes);
        assert_eq!(a("rules/r.dl").unwrap().role, Role::Rules);
        assert_eq!(a("other.ttl"), None, "the manifest is the whole truth");
        assert!(Layout::from_toml("[[graph]]\nfiles = 3").is_err());
    }

    // @lat: [[tests#Studio server#Conventions sniff roles]]
    #[test]
    fn conventions_sniff_roles() {
        let l = Layout::convention();
        let a = |p: &str, c: &str| l.assign(Path::new(p), "file:///x", c).map(|a| a.role);
        assert_eq!(
            a(
                "s.ttl",
                "@prefix sh: <http://www.w3.org/ns/shacl#> . ex:S a sh:NodeShape ."
            ),
            Some(Role::Shapes)
        );
        assert_eq!(a("o.ttl", "<o> a owl:Ontology ."), Some(Role::Ontology));
        assert_eq!(a("d.ttl", "<a> <b> <c> ."), Some(Role::Data));
        assert_eq!(a("r.dl", ""), Some(Role::Rules));
        assert_eq!(a("x.txt", ""), None);
    }
}
