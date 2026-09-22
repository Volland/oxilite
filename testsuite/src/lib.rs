//! oxilite ↔ Oxigraph compatibility harness.
//!
//! Oxigraph's W3C test runner (`manifest`, `evaluator`, `parser_evaluator`,
//! `sparql_evaluator`, `report`, `files`, `vocab`, `canonicalization_evaluator`), ported from
//! Oxigraph 0.5.11 `testsuite/` (MIT OR Apache-2.0, see `OXIGRAPH-LICENSE-*`), with every
//! evaluation also run on each oxilite variant ([`engine`]). Accepted divergences are listed
//! in `allowlist.toml` ([`allowlist`]); the [`differential`] corpus compares oxilite and
//! Oxigraph on generated data.
//!
// @lat: [[test-plan#Oxigraph compatibility harness]]

pub mod allowlist;
pub mod canonicalization_evaluator;
pub mod differential;
pub mod engine;
pub mod evaluator;
pub mod files;
pub mod manifest;
pub mod parser_evaluator;
pub mod report;
pub mod sparql_evaluator;
mod vocab;

use crate::canonicalization_evaluator::register_canonicalization_tests;
use crate::evaluator::TestEvaluator;
use crate::manifest::TestManifest;
use crate::parser_evaluator::register_parser_tests;
use crate::sparql_evaluator::register_sparql_tests;
use anyhow::Result;

/// Runs a W3C manifest. A test fails when it errors and is neither in Oxigraph's own
/// `upstream_ignored` list (the baseline copied from Oxigraph's `testsuite/tests`) nor in
/// `allowlist.toml`.
#[expect(clippy::panic_in_result_fn)]
pub fn check_testsuite(manifest_url: &str, upstream_ignored: &[&str]) -> Result<()> {
    let mut evaluator = TestEvaluator::default();
    register_parser_tests(&mut evaluator);
    register_canonicalization_tests(&mut evaluator);
    register_sparql_tests(&mut evaluator);

    let allow = allowlist::load()?;
    let manifest = TestManifest::new([manifest_url]);
    let results = evaluator.evaluate(manifest)?;

    let mut errors = Vec::default();
    let (mut passed, mut failed_upstream, mut allowed) = (0, 0, 0);
    for result in &results {
        match &result.outcome {
            Ok(()) => {
                passed += 1;
                if let Some(entry) = allow.get(result.test.as_str()) {
                    eprintln!(
                        "allow-list entry is stale (test now passes), removable: {} ({})",
                        result.test, entry.reason
                    );
                }
            }
            Err(error) => {
                if upstream_ignored.contains(&result.test.as_str()) {
                    failed_upstream += 1;
                } else if allow.contains_key(result.test.as_str()) {
                    allowed += 1;
                } else {
                    errors.push(format!("{}: failed with error {error:?}", result.test))
                }
            }
        }
    }
    allowlist::record(
        manifest_url,
        results.len(),
        passed,
        failed_upstream,
        allowed,
        errors.len(),
    );
    let (compiled, reasons) = engine::take_coverage();
    let fallback: usize = reasons.values().sum();
    if compiled + fallback > 0 {
        eprintln!(
            "{manifest_url}: {compiled} oxilite evaluations fully in SQL, {fallback} via the fallback"
        );
        let mut r: Vec<_> = reasons.into_iter().collect();
        r.sort_by(|a, b| b.1.cmp(&a.1));
        for (reason, n) in r {
            eprintln!("  {n:4} × {reason}");
        }
        allowlist::record_coverage(manifest_url, compiled, fallback);
    }
    assert!(
        errors.is_empty(),
        "{} failing tests:\n{}\n",
        errors.len(),
        errors.join("\n")
    );
    Ok(())
}
