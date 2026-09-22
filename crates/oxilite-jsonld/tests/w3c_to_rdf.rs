//! The W3C JSON-LD 1.1 `toRdf` suite (`testsuite/json-ld-api`), run through `put_document`
//! with the default-graph strategy. Remote contexts of the suite are served from the local
//! checkout by a context fetcher: no network access.
//!
//! Accepted failures are listed with a reason in `tests/to-rdf-allowlist.txt`; the test
//! fails on any unlisted failure and on listed tests that now pass.

use oxilite::jsonld::{GraphStrategy, JsonLdOptions, KeyStrategy, ProcessingMode, RdfDirection};
use oxilite::model::dataset::CanonicalizationAlgorithm;
use oxilite::model::Dataset;
use oxilite::store::Store;
use oxrdfio::{RdfFormat, RdfParser};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const BASE: &str = "https://w3c.github.io/json-ld-api/tests/";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testsuite/json-ld-api/tests")
}

fn allowlist() -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/to-rdf-allowlist.txt"),
    )
    .unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (id, reason) = l.split_once(char::is_whitespace)?;
            Some((id.to_owned(), reason.trim().to_owned()))
        })
        .collect()
}

fn canonical(mut d: Dataset) -> Dataset {
    d.canonicalize(CanonicalizationAlgorithm::Unstable);
    d
}

enum Outcome {
    Pass,
    Fail(String),
    Skip(&'static str),
}

fn run(test: &serde_json::Value) -> Outcome {
    let types: Vec<&str> = test["@type"]
        .as_array()
        .map(|a| a.iter().filter_map(|t| t.as_str()).collect())
        .unwrap_or_default();
    let option = &test["option"];
    if option["specVersion"] == "json-ld-1.0" {
        return Outcome::Skip("JSON-LD 1.0 only");
    }
    if option["produceGeneralizedRdf"] == true {
        return Outcome::Skip("generalized RDF is not stored");
    }
    if option.get("expandContext").is_some() {
        return Outcome::Skip("expandContext is not an option of document storage");
    }
    let input = test["input"].as_str().unwrap();
    let json = std::fs::read_to_string(root().join(input)).unwrap();
    let dir = root();
    let fetcher: oxilite::jsonld::ContextFetcher = Arc::new(move |iri: &str| {
        let path = iri
            .strip_prefix(BASE)
            .ok_or_else(|| oxilite::jsonld::JsonLdError::ContextNotFound(iri.to_owned()))?;
        std::fs::read_to_string(dir.join(path))
            .map_err(|_| oxilite::jsonld::JsonLdError::ContextNotFound(iri.to_owned()))
    });
    let base = option["base"]
        .as_str()
        .map_or_else(|| format!("{BASE}{input}"), str::to_owned);
    let options = JsonLdOptions {
        key: KeyStrategy::Explicit,
        graph: GraphStrategy::DefaultGraph,
        base_iri: Some(base),
        rdf_direction: match option["rdfDirection"].as_str() {
            Some("i18n-datatype") => Some(RdfDirection::I18nDatatype),
            Some("compound-literal") => Some(RdfDirection::CompoundLiteral),
            _ => None,
        },
        processing_mode: if option["processingMode"] == "json-ld-1.0" {
            ProcessingMode::JsonLd10
        } else {
            ProcessingMode::JsonLd11
        },
        fetcher: Some(fetcher),
        ..Default::default()
    };
    let store = Store::new().unwrap();
    let result = store
        .jsonld_with(options)
        .and_then(|d| d.put_document_with_key("urn:test", &json));
    if types.contains(&"jld:NegativeEvaluationTest") {
        return match result {
            Err(_) => Outcome::Pass,
            Ok(_) => Outcome::Fail(format!(
                "expected error {}",
                test["expectErrorCode"].as_str().unwrap_or("?")
            )),
        };
    }
    if let Err(e) = result {
        return Outcome::Fail(format!("error: {e}"));
    }
    let Some(expect) = test["expect"].as_str() else {
        return Outcome::Pass; // syntax test
    };
    let expected: Dataset = match RdfParser::from_format(RdfFormat::NQuads)
        .lenient()
        .for_slice(&std::fs::read(root().join(expect)).unwrap())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(q) => q.into_iter().collect(),
        Err(e) => return Outcome::Fail(format!("unparsable expectation: {e}")),
    };
    let actual: Dataset = store.iter().map(Result::unwrap).collect();
    if canonical(actual.clone()) == canonical(expected.clone()) {
        Outcome::Pass
    } else {
        Outcome::Fail(format!(
            "{} quads, expected {}",
            actual.len(),
            expected.len()
        ))
    }
}

// @lat: [[tests#JSON-LD documents#W3C toRdf suite]]
#[test]
fn w3c_to_rdf_suite() {
    let manifest_path = root().join("toRdf-manifest.jsonld");
    let Ok(text) = std::fs::read_to_string(&manifest_path) else {
        eprintln!("skipped: testsuite/json-ld-api is not checked out");
        return;
    };
    let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
    let allow = allowlist();
    let (mut pass, mut skip) = (0, 0);
    let mut unexpected = Vec::new();
    let mut fixed = Vec::new();
    let mut allowed = 0;
    let mut skipped = BTreeMap::new();
    for test in manifest["sequence"].as_array().unwrap() {
        let id = test["@id"]
            .as_str()
            .unwrap()
            .trim_start_matches('#')
            .to_owned();
        match run(test) {
            Outcome::Pass => {
                pass += 1;
                if allow.contains_key(&id) {
                    fixed.push(id);
                }
            }
            Outcome::Skip(reason) => {
                skip += 1;
                *skipped.entry(reason).or_insert(0) += 1;
            }
            Outcome::Fail(why) => {
                if allow.contains_key(&id) {
                    allowed += 1;
                } else {
                    unexpected.push(format!(
                        "{id} ({}): {why}",
                        test["name"].as_str().unwrap_or("")
                    ));
                }
            }
        }
    }
    eprintln!("toRdf: {pass} passed, {allowed} allowed failures, {skip} skipped {skipped:?}");
    assert!(
        unexpected.is_empty(),
        "unexpected failures:\n{}",
        unexpected.join("\n")
    );
    assert!(
        fixed.is_empty(),
        "allowlisted tests now pass, remove them: {fixed:?}"
    );
}
