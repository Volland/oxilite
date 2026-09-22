//! Differential corpus: oxilite vs Oxigraph on generated data.
use anyhow::Result;
use oxilite_compat::allowlist;
use oxilite_compat::differential::{check, dataset, queries};

// @lat: [[tests#Oxigraph compatibility#Differential corpus matches Oxigraph]]
#[test]
fn differential_corpus_matches_oxigraph() -> Result<()> {
    let allow = allowlist::load()?;
    let mut failures = Vec::new();
    let mut total = 0;
    for seed in [1_u64, 7] {
        let data = dataset(seed, 60);
        for cq in queries() {
            total += 1;
            if let Err(e) = check(&data, &cq) {
                if !allow.contains_key(cq.id.as_str()) {
                    failures.push(format!("seed {seed}: {e:#}"));
                }
            }
        }
    }
    eprintln!(
        "differential corpus: {total} query runs, {} unexpected divergences",
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    Ok(())
}

// @lat: [[tests#Oxigraph compatibility#Optimizer regression query]]
#[test]
fn optional_on_foreign_key_matches_oxigraph_and_is_planned_selectively() -> Result<()> {
    use oxilite_compat::differential::{orders_dataset, CorpusQuery, ORDERS_QUERY};
    let data = orders_dataset(20, 20);
    check(
        &data,
        &CorpusQuery {
            id: "corpus:optimizer-regression".into(),
            query: ORDERS_QUERY.into(),
            ordered: false,
        },
    )?;
    // The OPTIONAL's inner BGP must start with the foreign-key lookup on the bound ?c, not
    // with `?order a schema:Order` (which would scan every order for each person).
    let engine = oxilite_compat::engine::OxiliteEngine::new(
        oxilite_compat::engine::Variant::Rusqlite,
        &data,
    )?;
    let query = spargebra::SparqlParser::new().parse_query(ORDERS_QUERY)?;
    let explain = engine.explain(&query);
    let inner = explain
        .lines()
        .filter(|l| l.starts_with("-- join order"))
        .nth(1)
        .unwrap_or_default()
        .to_string();
    assert!(
        inner.contains(": ?order <http://schema.org/customer> ?c"),
        "OPTIONAL planned without the bound ?c:\n{explain}"
    );
    Ok(())
}
