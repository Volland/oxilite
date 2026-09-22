//! Turns `bench/bsbm.sh` results (BSBM testdriver XML and load JSON) into
//! `bench/results/summary.json` and the benchmark table of the README.
//!
//! `cargo run -p oxilite-bench --bin bsbm-report [results-dir] [README.md]`

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

const BEGIN: &str = "<!-- bsbm-results:begin -->";
const END: &str = "<!-- bsbm-results:end -->";

/// Text of the first `<tag>…</tag>` after `from`.
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{name}>"))? + start;
    Some(xml[start..end].trim())
}

fn num(xml: &str, name: &str) -> Option<f64> {
    tag(xml, name)?.parse().ok()
}

fn parse_mix(xml: &str) -> Value {
    let mut queries = BTreeMap::new();
    for part in xml.split("<query nr=\"").skip(1) {
        let nr: u32 = part
            .split('"')
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        queries.insert(
            nr,
            json!({
                "aqet": num(part, "aqet"),
                "qps": num(part, "qps"),
                "avg_results": num(part, "avgresults"),
                "timeouts": num(part, "timeoutcount"),
            }),
        );
    }
    json!({
        "qmph": num(xml, "qmph"),
        "runs": num(xml, "querymixruns"),
        "queries": queries,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = args.get(1).map_or_else(|| root.join("results"), Into::into);
    let readme = args
        .get(2)
        .map_or_else(|| root.join("../README.md"), Into::into);

    // results[products][engine] = {load…, explore, businessIntelligence}
    let mut results: BTreeMap<u64, BTreeMap<String, Value>> = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("results directory") {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let parts: Vec<&str> = name.split('.').collect();
        match parts.as_slice() {
            ["load", engine, products, "json"] => {
                let v: Value = serde_json::from_str(&text).unwrap_or_default();
                let e = results
                    .entry(products.parse().unwrap_or(0))
                    .or_default()
                    .entry((*engine).to_string())
                    .or_insert_with(|| json!({}));
                e["load"] = v;
            }
            ["bsbm", mix, engine, products, "xml"] => {
                let e = results
                    .entry(products.parse().unwrap_or(0))
                    .or_default()
                    .entry((*engine).to_string())
                    .or_insert_with(|| json!({}));
                e[*mix] = parse_mix(&text);
            }
            _ => {}
        }
    }
    let summary = json!(results);
    std::fs::write(
        dir.join("summary.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();

    let fmt = |v: Option<f64>, digits: usize| {
        v.map_or_else(|| "–".to_string(), |x| format!("{x:.digits$}"))
    };
    let mut md = String::new();
    for (products, engines) in &results {
        let triples = engines
            .values()
            .find_map(|e| e["load"]["triples"].as_u64())
            .unwrap_or(0);
        md.push_str(&format!(
            "\n**{products} products ({triples} triples)**\n\n| engine | load (s) | size (MB) | explore QMpH | business intelligence QMpH |\n|---|---|---|---|---|\n"
        ));
        for (engine, e) in engines {
            let size = e["load"]["size_bytes"]
                .as_f64()
                .filter(|s| *s > 0.0)
                .map(|s| s / 1e6);
            md.push_str(&format!(
                "| {engine} | {} | {} | {} | {} |\n",
                fmt(e["load"]["load_seconds"].as_f64(), 2),
                fmt(size, 1),
                fmt(e["explore"]["qmph"].as_f64(), 0),
                fmt(e["businessIntelligence"]["qmph"].as_f64(), 0),
            ));
        }
        // Per-query explore times, to spot regressions.
        md.push_str("\n| explore query (avg ms) |");
        let names: Vec<&String> = engines.keys().collect();
        for n in &names {
            md.push_str(&format!(" {n} |"));
        }
        md.push_str("\n|---|");
        md.push_str(&"---|".repeat(names.len()));
        md.push('\n');
        for q in 1..=12 {
            // Queries outside the mix (BSBM 3.1 dropped explore Q6) have no timings.
            if engines.values().all(|e| {
                e["explore"]["queries"][q.to_string()]["aqet"]
                    .as_f64()
                    .unwrap_or(0.0)
                    == 0.0
            }) {
                continue;
            }
            md.push_str(&format!("| Q{q} |"));
            for n in &names {
                let aqet = engines[*n]["explore"]["queries"][q.to_string()]["aqet"].as_f64();
                md.push_str(&format!(" {} |", fmt(aqet.map(|s| s * 1000.0), 2)));
            }
            md.push('\n');
        }
    }
    print!("{md}");
    if let Ok(text) = std::fs::read_to_string(&readme) {
        if let (Some(b), Some(e)) = (text.find(BEGIN), text.find(END)) {
            let updated = format!("{}{BEGIN}\n{md}\n{}", &text[..b], &text[e..]);
            std::fs::write(&readme, updated).unwrap();
            eprintln!("updated {}", readme.display());
        }
    }
}
