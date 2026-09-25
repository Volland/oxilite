//! D1 write cost: rows written (D1's billing unit, index entries included) per inserted triple,
//! for each schema configuration, measured on a local D1 database (the Miniflare sidecar).
//!
//! `cargo run -p oxilite-bench --bin write-cost [sidecar-url] [triples]` writes
//! `bench/results/write-cost.json` and prints a Markdown table.

use oxilite::model::{Literal, NamedNode, Quad};
use oxilite_core::{ops, version, Capabilities, Job, Request, Step, StoreOptions, Versioning};
use serde_json::{json, Value};

fn post(url: &str, path: &str, body: Value) -> Value {
    ureq::post(&format!("{url}{path}"))
        .send_json(body)
        .expect("D1 sidecar request")
        .into_json()
        .expect("JSON response")
}

/// Runs a request on the sidecar; returns the result sets and the rows written.
fn execute(url: &str, request: &Request) -> (Value, u64) {
    let v = post(url, "/execute", serde_json::to_value(request).unwrap());
    let written = v
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| r["rows_written"].as_u64().unwrap_or(0))
                .sum()
        })
        .unwrap_or(0);
    (v, written)
}

/// Synthetic product data: types, labels (text), decimals, links to shared nodes.
fn data(triples: usize) -> Vec<Quad> {
    let ex = |s: String| NamedNode::new_unchecked(format!("http://example.com/{s}"));
    let mut out = Vec::new();
    for i in 0..triples.div_ceil(5) {
        let p = ex(format!("product{i}"));
        out.push(Quad::new(
            p.clone(),
            oxrdf::vocab::rdf::TYPE,
            ex("Product".into()),
            oxrdf::GraphName::DefaultGraph,
        ));
        out.push(Quad::new(
            p.clone(),
            ex("label".into()),
            Literal::new_simple_literal(format!("product {i} with a fairly long label")),
            oxrdf::GraphName::DefaultGraph,
        ));
        out.push(Quad::new(
            p.clone(),
            ex("price".into()),
            Literal::new_typed_literal(format!("{}.5", i % 997), oxrdf::vocab::xsd::DECIMAL),
            oxrdf::GraphName::DefaultGraph,
        ));
        out.push(Quad::new(
            p.clone(),
            ex("producer".into()),
            ex(format!("producer{}", i % 10)),
            oxrdf::GraphName::DefaultGraph,
        ));
        out.push(Quad::new(
            p,
            ex("feature".into()),
            ex(format!("feature{}", i % 50)),
            oxrdf::GraphName::DefaultGraph,
        ));
    }
    out.truncate(triples);
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url = args
        .get(1)
        .cloned()
        .or_else(|| std::env::var("OXILITE_D1_URL").ok())
        .expect("sidecar URL (argument or OXILITE_D1_URL)");
    let triples: usize = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(5_000);
    let caps = Capabilities::d1();
    let quads = data(triples);
    let base = |graph_index, text_index| StoreOptions {
        graph_index,
        text_index,
        ..Default::default()
    };
    let versioned = |versioning, as_of_index| StoreOptions {
        versioning,
        as_of_index,
        ..Default::default()
    };
    let configs = [
        ("graph index (default)", base(true, false)),
        ("no graph index", base(false, false)),
        ("graph index + text index", base(true, true)),
        ("versioning: stamped", versioned(Versioning::Stamped, false)),
        ("versioning: log", versioned(Versioning::Log, false)),
        (
            "versioning: log + as-of index",
            versioned(Versioning::Log, true),
        ),
    ];
    let mut report = Vec::new();
    println!("| schema | rows written per triple | statements per 1000 triples |\n|---|---|---|");
    for (name, options) in configs {
        post(&url, "/reset", json!({}));
        let mut open = ops::open_job(&options, &caps);
        let mut response = None;
        while let Step::Execute(r) = open.step(response.take()).unwrap() {
            let (v, _) = execute(&url, &r);
            response = Some(serde_json::from_value(v).unwrap());
        }
        // A versioned store stamps its inserts and opens one tick per batch.
        let caps = version::effective_caps(&caps, options.versioning);
        let (mut written, mut statements) = (0u64, 0usize);
        for chunk in quads.chunks(500) {
            let request = ops::insert_request(chunk.iter().map(Quad::as_ref), &caps);
            let request = version::prepare(&request, options.versioning, &Default::default())
                .unwrap_or(request);
            statements += request.statements.len();
            written += execute(&url, &request).1;
        }
        let per_triple = written as f64 / quads.len() as f64;
        println!(
            "| {name} | {per_triple:.2} | {:.1} |",
            statements as f64 * 1000.0 / quads.len() as f64
        );
        report.push(json!({
            "schema": name,
            "graph_index": options.graph_index,
            "text_index": options.text_index,
            "versioning": options.versioning.as_str(),
            "as_of_index": options.as_of_index,
            "triples": quads.len(),
            "rows_written": written,
            "rows_written_per_triple": per_triple,
            "statements": statements,
        }));
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("results/write-cost.json");
    std::fs::write(&out, serde_json::to_string_pretty(&json!(report)).unwrap()).unwrap();
    eprintln!("wrote {}", out.display());
}
