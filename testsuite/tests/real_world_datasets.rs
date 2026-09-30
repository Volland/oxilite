//! Reliability tests against real published RDF (not W3C conformance micro-fixtures): can
//! oxilite load data from several independent publishers and formats, keep it intact, and
//! answer generic SPARQL queries the same way Oxigraph does? Snapshots and provenance are in
//! `testsuite/real-world-data/README.md`.
#![cfg(test)]

use anyhow::{bail, Context, Result};
use oxigraph::io::{JsonLdProfileSet, RdfFormat, RdfParser};
use oxigraph::model::graph::CanonicalizationAlgorithm;
use oxigraph::model::Dataset;
use oxilite_compat::differential::{check, CorpusQuery};
use oxilite_compat::engine::{variants, OxiliteEngine};
use std::fs::File;
use std::path::Path;

/// One vendored real-world snapshot.
struct RealWorldDataset {
    /// Short id used in test failure messages and corpus query ids.
    name: &'static str,
    /// Path under `testsuite/real-world-data/`.
    file: &'static str,
    format: RdfFormat,
}

const DATASETS: &[RealWorldDataset] = &[
    RealWorldDataset {
        name: "foaf",
        file: "foaf.rdf",
        format: RdfFormat::RdfXml,
    },
    RealWorldDataset {
        name: "skos",
        file: "skos.rdf",
        format: RdfFormat::RdfXml,
    },
    RealWorldDataset {
        name: "doap",
        file: "doap.rdf",
        format: RdfFormat::RdfXml,
    },
    RealWorldDataset {
        name: "dcat",
        file: "dcat.ttl",
        format: RdfFormat::Turtle,
    },
    RealWorldDataset {
        name: "prov-o",
        file: "prov-o.ttl",
        format: RdfFormat::Turtle,
    },
    RealWorldDataset {
        name: "wikidata-q42",
        file: "wikidata-q42.ttl",
        format: RdfFormat::Turtle,
    },
    RealWorldDataset {
        name: "dbpedia-douglas-adams-ttl",
        file: "dbpedia-douglas-adams.ttl",
        format: RdfFormat::Turtle,
    },
    RealWorldDataset {
        name: "dbpedia-douglas-adams-jsonld",
        file: "dbpedia-douglas-adams.jsonld",
        format: RdfFormat::JsonLd {
            profile: JsonLdProfileSet::empty(),
        },
    },
];

fn load(d: &RealWorldDataset) -> Result<Dataset> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("real-world-data")
        .join(d.file);
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let mut dataset = Dataset::new();
    for quad in RdfParser::from_format(d.format).for_reader(file) {
        dataset.insert(&quad.with_context(|| format!("parsing {}", path.display()))?);
    }
    // A parse that silently returns nothing usually means the vendored file is stale (an HTML
    // error page, a redirect target) rather than that the dataset is genuinely tiny.
    if dataset.len() < 20 {
        bail!(
            "{}: only {} quads parsed from {} — is the vendored snapshot still real RDF?",
            d.name,
            dataset.len(),
            path.display()
        );
    }
    Ok(dataset)
}

/// Generic queries that assume nothing about a dataset's vocabulary, run against every
/// snapshot: aggregation, GROUP BY/ORDER BY, OPTIONAL, and a language-tagged literal filter.
fn generic_queries(name: &str) -> Vec<CorpusQuery> {
    let q = |suffix: &str, query: &str, ordered: bool| CorpusQuery {
        id: format!("real-world:{name}:{suffix}"),
        query: query.to_string(),
        ordered,
    };
    vec![
        q(
            "count-all",
            "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
            false,
        ),
        q(
            "count-distinct-subjects",
            "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s ?p ?o }",
            false,
        ),
        q(
            "predicates-by-frequency",
            "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) ?p",
            true,
        ),
        q(
            "typed-nodes-with-optional-label",
            "SELECT ?s ?c ?l WHERE { ?s a ?c OPTIONAL { \
             ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l } }",
            false,
        ),
        q(
            "english-literals",
            "SELECT (COUNT(*) AS ?n) WHERE { \
             ?s ?p ?o FILTER(isLiteral(?o) && lang(?o) = \"en\") }",
            false,
        ),
    ]
}

// @lat: [[tests#Oxigraph compatibility#Real-world datasets round-trip through oxilite]]
#[test]
fn real_world_datasets_round_trip_through_oxilite() -> Result<()> {
    let mut failures = Vec::new();
    for d in DATASETS {
        let dataset = match load(d) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{}: {e:#}", d.name));
                continue;
            }
        };
        let mut expected = dataset.clone();
        expected.canonicalize(CanonicalizationAlgorithm::Unstable);
        for variant in variants() {
            let outcome = OxiliteEngine::new(variant, &dataset).and_then(|engine| {
                let mut stored = engine.dataset()?;
                stored.canonicalize(CanonicalizationAlgorithm::Unstable);
                if stored == expected {
                    Ok(())
                } else {
                    bail!(
                        "round-trip changed the dataset ({} quads in, {} out)",
                        expected.len(),
                        stored.len()
                    )
                }
            });
            if let Err(e) = outcome {
                failures.push(format!("{}[{variant}]: {e:#}", d.name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

// @lat: [[tests#Oxigraph compatibility#Real-world datasets answer generic queries like Oxigraph]]
#[test]
fn real_world_datasets_answer_generic_queries_like_oxigraph() -> Result<()> {
    let mut failures = Vec::new();
    for d in DATASETS {
        let dataset = match load(d) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{}: {e:#}", d.name));
                continue;
            }
        };
        for cq in generic_queries(d.name) {
            if let Err(e) = check(&dataset, &cq) {
                failures.push(format!("{e:#}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    Ok(())
}
