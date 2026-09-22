//! rudof's ShEx validator gives the same results over an oxilite store as over its in-memory
//! graph on the shexTest validation suite (`testsuite/shexTest`).

use oxilite::io::{RdfFormat, RdfParser};
use oxilite::model::{GraphName, NamedNode, Quad, Term};
use oxilite::store::Store;
use oxilite_validate::{shex_schema, validate_shex_graph, ResultShapeMap, StoreGraph};
use rudof_rdf::rdf_core::{NeighsRDF, RDFFormat};
use rudof_rdf::rdf_impl::{OxigraphInMemory, ReaderMode};
use std::collections::BTreeMap;
use std::path::Path;

const SHT: &str = "http://www.w3.org/ns/shacl/test-suite#";

struct Case {
    name: String,
    schema: String,
    data: String,
    shape: String,
    focus: Term,
    pass: bool,
}

fn cases() -> Vec<Case> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testsuite/shexTest/validation");
    let manifest = dir.join("manifest.ttl");
    let base = format!(
        "file://{}",
        manifest
            .canonicalize()
            .expect("git submodule update --init testsuite/shexTest")
            .display()
    );
    let quads: Vec<Quad> = RdfParser::from_format(RdfFormat::Turtle)
        .with_base_iri(&base)
        .unwrap()
        .for_reader(std::fs::File::open(&manifest).unwrap())
        .collect::<Result<_, _>>()
        .unwrap();
    let get = |s: &Term, p: &str| -> Option<Term> {
        quads
            .iter()
            .find(|q| Term::from(q.subject.clone()) == *s && q.predicate.as_str() == p)
            .map(|q| q.object.clone())
    };
    let sht = |l: &str| format!("{SHT}{l}");
    let mut out = Vec::new();
    for q in &quads {
        if q.predicate.as_str() != "http://www.w3.org/1999/02/22-rdf-syntax-ns#type" {
            continue;
        }
        let pass = match &q.object {
            Term::NamedNode(n) if n.as_str() == sht("ValidationTest") => true,
            Term::NamedNode(n) if n.as_str() == sht("ValidationFailure") => false,
            _ => continue,
        };
        let test = Term::from(q.subject.clone());
        let Some(action) = get(
            &test,
            "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#action",
        ) else {
            continue;
        };
        let (
            Some(Term::NamedNode(schema)),
            Some(Term::NamedNode(data)),
            Some(Term::NamedNode(shape)),
            Some(focus),
        ) = (
            get(&action, &sht("schema")),
            get(&action, &sht("data")),
            get(&action, &sht("shape")),
            get(&action, &sht("focus")),
        )
        else {
            continue;
        };
        out.push(Case {
            name: test.to_string(),
            schema: schema.as_str().to_string(),
            data: data.as_str().to_string(),
            shape: shape.as_str().to_string(),
            focus,
            pass,
        });
    }
    out
}

fn statuses(r: &ResultShapeMap) -> BTreeMap<String, String> {
    r.iter()
        .map(|(n, l, s)| {
            let k = if s.is_conformant() {
                "conformant"
            } else if s.is_non_conformant() {
                "nonconformant"
            } else {
                "other"
            };
            (format!("{n}@{l}"), k.to_string())
        })
        .collect()
}

/// The manifest's `@base` is the suite's GitHub URL: read those files from the checkout.
fn read(iri: &str) -> Option<String> {
    let rel = iri.strip_prefix("https://raw.githubusercontent.com/shexSpec/shexTest/master/")?;
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsuite/shexTest")
            .join(rel),
    )
    .ok()
}

// @lat: [[tests#Validation#ShEx suite matches rudof in memory]]
#[test]
fn shex_suite_matches_rudof_in_memory() {
    let store = Store::new().unwrap();
    let (mut compared, mut expected_ok, mut failures) = (0, 0, Vec::new());
    let all = cases();
    let mut skipped = BTreeMap::<&str, usize>::new();
    for case in all.iter() {
        let (Some(schema_text), Some(data_text)) = (read(&case.schema), read(&case.data)) else {
            *skipped.entry("unreadable").or_default() += 1;
            continue;
        };
        let schema = match shex_schema(&schema_text, &case.schema) {
            Ok(s) => s,
            Err(e) => {
                if !skipped.contains_key("schema") {
                    eprintln!("first schema error: {e}");
                }
                *skipped.entry("schema").or_default() += 1;
                continue;
            }
        };
        let Ok(mut memory) = OxigraphInMemory::from_str(
            &data_text,
            &RDFFormat::Turtle,
            Some(&case.data),
            &ReaderMode::Strict,
        ) else {
            *skipped.entry("data").or_default() += 1;
            continue;
        };
        memory.ensure_store().unwrap();
        store.clear().unwrap();
        store
            .extend(
                memory
                    .triples()
                    .unwrap()
                    .map(|t| Quad::new(t.subject, t.predicate, t.object, GraphName::DefaultGraph)),
            )
            .unwrap();
        let map = format!("{}@{}", case.focus, NamedNode::new_unchecked(&case.shape));
        let expected = validate_shex_graph(&memory, &schema, &map);
        let actual = validate_shex_graph(&StoreGraph::new(store.clone()), &schema, &map);
        compared += 1;
        match (&expected, &actual) {
            (Ok(a), Ok(b)) if statuses(a) == statuses(b) => {
                let conformant = statuses(b).values().all(|s| s == "conformant");
                if conformant == case.pass {
                    expected_ok += 1;
                }
            }
            (Err(a), Err(b)) if a.to_string() == b.to_string() => {}
            _ => failures.push(format!(
                "{}: in memory {:?} / oxilite {:?}",
                case.name,
                expected.as_ref().map(statuses).map_err(ToString::to_string),
                actual.as_ref().map(statuses).map_err(ToString::to_string)
            )),
        }
    }
    eprintln!("{} cases, skipped {skipped:?}", all.len());
    eprintln!(
        "{compared} ShEx validations compared, {expected_ok} match the suite's expected outcome"
    );
    assert!(compared > 900, "only {compared} compared");
    assert!(
        failures.is_empty(),
        "{} of {compared} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
