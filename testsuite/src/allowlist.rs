//! Accepted divergences from Oxigraph / the W3C expectations, and the compatibility report.
//!
// @lat: [[test-plan#Oxigraph compatibility harness#Allow-list and report]]

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// One accepted divergence.
#[derive(Debug, Clone)]
pub struct Entry {
    pub reason: String,
    pub decision: String,
}

/// Parses `allowlist.toml`: `[[divergence]]` tables with `id`, `reason`, `decision`.
///
/// A tiny line-based parser keeps the harness dependency-free.
pub fn parse(text: &str) -> Result<HashMap<String, Entry>> {
    let mut out = HashMap::new();
    let mut cur: HashMap<String, String> = HashMap::new();
    let flush =
        |cur: &mut HashMap<String, String>, out: &mut HashMap<String, Entry>| -> Result<()> {
            if cur.is_empty() {
                return Ok(());
            }
            let id = cur.remove("id").context("allow-list entry without id")?;
            let reason = cur
                .remove("reason")
                .with_context(|| format!("allow-list entry {id} without reason"))?;
            let decision = cur
                .remove("decision")
                .with_context(|| format!("allow-list entry {id} without decision"))?;
            out.insert(id, Entry { reason, decision });
            cur.clear();
            Ok(())
        };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[divergence]]" {
            flush(&mut cur, &mut out)?;
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .with_context(|| format!("bad allow-list line: {line}"))?;
        let v = v.trim().trim_matches('"').to_string();
        cur.insert(k.trim().to_string(), v);
    }
    flush(&mut cur, &mut out)?;
    Ok(out)
}

/// Loads `testsuite/allowlist.toml`.
pub fn load() -> Result<HashMap<String, Entry>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("allowlist.toml");
    parse(&std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?)
}

/// Appends a line to `target/compat-report.tsv` (aggregated into COMPATIBILITY.md).
pub fn record(
    suite: &str,
    total: usize,
    passed: usize,
    upstream: usize,
    allowed: usize,
    failed: usize,
) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("compat-report.tsv"))
    {
        let _ = writeln!(
            f,
            "{suite}\t{total}\t{passed}\t{upstream}\t{allowed}\t{failed}"
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses() {
        let a = super::parse(
            "# c\n[[divergence]]\nid = \"a\"\nreason = \"r\"\ndecision = \"d\"\n\n[[divergence]]\nid = \"b\"\nreason = \"r2\"\ndecision = \"d2\"\n",
        )
        .unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a["b"].reason, "r2");
    }
}
