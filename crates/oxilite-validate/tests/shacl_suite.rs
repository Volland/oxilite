//! rudof over an oxilite store gives the same SHACL reports as rudof over its in-memory graph,
//! on every test of the W3C SHACL core test suite (`testsuite/data-shapes`), in native and
//! SPARQL validation modes, on the bundled SQLite and on the system `libsqlite3`.

use oxilite::model::{GraphName, Quad};
use oxilite::store::Store;
use oxilite_core::SyncBackend;
use oxilite_validate::{shacl_schema, validate_shacl_graph, ShaclValidationMode, StoreGraph};
use rudof_rdf::rdf_core::{NeighsRDF, RDFFormat};
use rudof_rdf::rdf_impl::{OxigraphInMemory, ReaderMode};
use std::path::{Path, PathBuf};

fn suite_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testsuite/data-shapes/data-shapes-test-suite/tests/core");
    let mut files = Vec::new();
    for dir in std::fs::read_dir(&root).expect("git submodule update --init testsuite/data-shapes")
    {
        let dir = dir.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(&dir).unwrap() {
            let f = f.unwrap().path();
            if f.extension().is_some_and(|e| e == "ttl")
                && f.file_name().is_some_and(|n| n != "manifest.ttl")
            {
                files.push(f);
            }
        }
    }
    files.sort();
    files
}

fn dylib() -> Store<oxilite::dylib::DylibBackend> {
    let lib = std::env::var("OXILITE_SQLITE_LIBRARY").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/usr/lib/libsqlite3.dylib".into()
        } else {
            "libsqlite3.so.0".into()
        }
    });
    Store::open_with_library(lib, ":memory:").unwrap()
}

/// (reports compared, reports with violations, failures)
fn run_on<B: SyncBackend + Send + Sync + 'static>(
    store: Store<B>,
    name: &str,
) -> (usize, usize, Vec<String>) {
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut violations = 0;
    for file in suite_files() {
        let text = std::fs::read_to_string(&file).unwrap();
        let base = format!("file:/{}", file.canonicalize().unwrap().display());
        let Ok(schema) = shacl_schema(&text, &RDFFormat::Turtle, Some(&base)) else {
            continue; // shapes rudof itself cannot compile
        };
        let mut memory =
            OxigraphInMemory::from_str(&text, &RDFFormat::Turtle, Some(&base), &ReaderMode::Strict)
                .unwrap();
        memory.ensure_store().unwrap();
        // The same triples (same blank nodes) in the store.
        store.clear().unwrap();
        store
            .extend(
                memory
                    .triples()
                    .unwrap()
                    .map(|t| Quad::new(t.subject, t.predicate, t.object, GraphName::DefaultGraph)),
            )
            .unwrap();
        for mode in [ShaclValidationMode::Native, ShaclValidationMode::Sparql] {
            let expected = validate_shacl_graph(memory.clone(), &schema, &mode);
            let actual = validate_shacl_graph(StoreGraph::new(store.clone()), &schema, &mode);
            compared += 1;
            if matches!(&actual, Ok(r) if !r.results().is_empty()) {
                violations += 1;
            }
            let same = match (&expected, &actual) {
                (Ok(a), Ok(b)) => a == b,
                (Err(a), Err(b)) => a.to_string() == b.to_string(),
                _ => false,
            };
            if !same {
                failures.push(format!(
                    "{name} {mode:?} {}:\n  in memory: {:?}\n  oxilite:   {:?}",
                    file.display(),
                    expected.map(|r| r.results().len()),
                    actual.map(|r| r.results().len())
                ));
            }
        }
    }
    (compared, violations, failures)
}

// @lat: [[tests#Validation#SHACL suite matches rudof in memory]]
#[test]
fn shacl_core_suite_matches_rudof_in_memory() {
    let (n, v, mut failures) = run_on(Store::new().unwrap(), "rusqlite");
    let (m, _, f) = run_on(dylib(), "dylib");
    failures.extend(f);
    eprintln!("{n} reports compared per backend, {v} with violations");
    assert!(
        n > 200 && m == n && v > 100,
        "only {n}/{m} validations compared, {v} with violations"
    );
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        n + m,
        failures.join("\n")
    );
}
