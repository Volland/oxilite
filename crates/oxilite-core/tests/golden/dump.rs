//! The SQL an unversioned store generates for a corpus of queries and updates, one hash per
//! file and backend. Shared by the golden test and the `golden_sql` example, which writes the
//! baseline from the code before versioning existed.

use oxilite_core::query::compile_query;
use oxilite_core::update::{explain_plan, plan_update};
use oxilite_core::writer::EncodedQuads;
use oxilite_core::{Capabilities, QueryOptions, Stats};
use spargebra::SparqlParser;
use std::path::Path;

fn hash(s: &str) -> String {
    format!(
        "{:016x}",
        xxhash_rust::xxh3::xxh3_64(normalize(s).as_bytes())
    )
}

/// Renames the parser's random blank node labels (`_:` and 16 or more hex digits, in the planner's
/// notes) to `_:b0`, `_:b1`… in order of appearance, so the rendering is deterministic.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut seen: Vec<&str> = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("_:") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 2..];
        let n = tail.bytes().take_while(u8::is_ascii_hexdigit).count();
        if n >= 16 {
            let label = &tail[..n];
            let k = seen.iter().position(|l| *l == label).unwrap_or_else(|| {
                seen.push(label);
                seen.len() - 1
            });
            out.push_str(&format!("_:b{k}"));
            rest = &tail[n..];
        } else {
            out.push_str("_:");
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if matches!(p.extension().and_then(|x| x.to_str()), Some("rq" | "ru")) {
            out.push(p);
        }
    }
}

/// `path \t backend \t hash` lines, sorted, for every `.rq` / `.ru` file under `root`, plus
/// the write statements of a fixed set of quads.
pub fn dump(root: &Path) -> Vec<String> {
    let backends = [
        ("native", Capabilities::native()),
        ("d1", Capabilities::d1()),
    ];
    let mut paths = Vec::new();
    for sub in ["sparql10", "sparql11", "sparql12"] {
        files(&root.join(sub), &mut paths);
    }
    paths.sort();
    let mut out = Vec::new();
    let options = QueryOptions::default();
    let stats = Stats::default();
    for p in &paths {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        // How `\u` escapes parse depends on spargebra's `standard-unicode-escaping` feature, which
        // workspace builds enable (for the W3C runner): leave those files out, so the rendering
        // depends on the compiler alone.
        if text.contains("\\u") || text.contains("\\U") {
            continue;
        }
        let rel = p.strip_prefix(root).unwrap().display().to_string();
        let parser = || {
            SparqlParser::new()
                .with_base_iri("http://example.com/base/")
                .unwrap()
        };
        let is_update = p.extension().and_then(|x| x.to_str()) == Some("ru");
        for (name, caps) in &backends {
            let render = || -> Option<String> {
                Some(if is_update {
                    match plan_update(&parser().parse_update(&text).ok()?, caps) {
                        Ok(plan) => explain_plan(&plan),
                        Err(e) => format!("ERR {e}"),
                    }
                } else {
                    match compile_query(&parser().parse_query(&text).ok()?, &stats, caps, &options)
                    {
                        Ok(c) => c.explain(),
                        Err(e) => format!("ERR {e}"),
                    }
                })
            };
            let Some(first) = render() else { continue };
            // Updates creating fresh blank nodes embed random ids: record that they vary.
            let h = if render().map(|r| hash(&r)) == Some(hash(&first)) {
                hash(&first)
            } else {
                "varies".to_owned()
            };
            out.push(format!("{rel}\t{name}\t{h}"));
        }
    }
    // The batch writer: inserts and deletes of a fixed set of quads.
    let ex = |s: String| oxrdf::NamedNode::new_unchecked(format!("http://example.com/{s}"));
    let quads: Vec<oxrdf::Quad> = (0..700)
        .map(|i| {
            oxrdf::Quad::new(
                ex(format!("s{i}")),
                ex(format!("p{}", i % 7)),
                oxrdf::Literal::new_simple_literal(format!("value {i}")),
                if i % 3 == 0 {
                    oxrdf::GraphName::DefaultGraph
                } else {
                    ex(format!("g{}", i % 5)).into()
                },
            )
        })
        .collect();
    let enc = EncodedQuads::new(quads.iter().map(oxrdf::Quad::as_ref));
    for (name, caps) in &backends {
        let ins: Vec<String> = enc
            .insert_statements(caps)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        let del: Vec<String> = enc
            .delete_statements(caps)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        out.push(format!("writer/insert\t{name}\t{}", hash(&ins.join("\n"))));
        out.push(format!("writer/delete\t{name}\t{}", hash(&del.join("\n"))));
    }
    out
}
