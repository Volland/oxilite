//! As-of query latency: the same queries on the current state and on past versions of a store
//! with a change log, with and without the as-of index, against an unversioned store.
//!
//! `cargo run --release -p oxilite-bench --bin as-of-latency [entities] [commits]` writes
//! `bench/results/as-of-latency.json` and prints a Markdown table. The data is synthetic and
//! deterministic: tickets with a type, an owner, a priority and a status; each commit moves
//! the status of a few tickets and adds a comment (bulk-loaded tickets, then small commits, the
//! shape of an application's history).

use oxilite::io::RdfFormat;
use oxilite::rusqlite::RusqliteBackend;
use oxilite::sparql::QueryResults;
use oxilite::store::Store;
use oxilite::version::Versioning;
use oxilite::{QueryOptions, StoreOptions};
use serde_json::json;
use std::time::Instant;

const EX: &str = "http://example.com/";
const STATUSES: [&str; 4] = ["open", "doing", "blocked", "done"];

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

struct Built {
    store: Store<RusqliteBackend>,
    /// Bulk load of the initial tickets, ms.
    load_ms: f64,
    /// Mean wall time of one commit (a SPARQL update), ms.
    commit_ms: f64,
    /// Database size (pages × page size), bytes.
    bytes: i64,
}

fn build(level: Versioning, as_of_index: bool, entities: usize, commits: usize) -> Built {
    let store = Store::with_backend_and_options(
        RusqliteBackend::memory().unwrap(),
        &StoreOptions {
            versioning: level,
            as_of_index,
            ..Default::default()
        },
    )
    .unwrap();
    let mut nt = String::new();
    for i in 0..entities {
        nt.push_str(&format!(
            "<{EX}t{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{EX}Ticket> .\n\
             <{EX}t{i}> <{EX}owner> <{EX}u{}> .\n\
             <{EX}t{i}> <{EX}priority> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n\
             <{EX}t{i}> <{EX}status> \"open\" .\n",
            i % 97,
            i % 5
        ));
    }
    let start = Instant::now();
    store
        .load_from_reader(RdfFormat::NTriples, nt.as_bytes())
        .unwrap();
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut rng = Rng(42);
    let start = Instant::now();
    for c in 0..commits {
        let tickets: Vec<String> = (0..20)
            .map(|_| format!("<{EX}t{}>", rng.below(entities as u64)))
            .collect();
        let status = STATUSES[rng.below(4) as usize];
        store
            .update(format!(
                "DELETE {{ ?t <{EX}status> ?s }} INSERT {{ ?t <{EX}status> \"{status}\" }} \
                 WHERE {{ VALUES ?t {{ {} }} ?t <{EX}status> ?s }} ; \
                 INSERT DATA {{ <{EX}c{c}> <{EX}about> {} ; <{EX}text> \"comment {c}\" }}",
                tickets.join(" "),
                tickets[0]
            ))
            .unwrap();
    }
    let commit_ms = start.elapsed().as_secs_f64() * 1000.0 / commits.max(1) as f64;
    store.optimize().unwrap();
    let bytes = store.backend().with_connection(|c| {
        c.query_row(
            "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
            [],
            |r| r.get(0),
        )
        .unwrap()
    });
    Built {
        store,
        load_ms,
        commit_ms,
        bytes,
    }
}

fn run(store: &Store<RusqliteBackend>, query: &str, as_of: Option<&str>) -> usize {
    let options = QueryOptions {
        as_of: as_of.map(str::to_owned),
        ..Default::default()
    };
    match store.query_opt(query, options).unwrap() {
        QueryResults::Solutions(s) => s.count(),
        QueryResults::Boolean(_) => 1,
        QueryResults::Graph(g) => g.count(),
    }
}

/// Median wall time in milliseconds of `runs` executions after one warm-up.
fn time(
    store: &Store<RusqliteBackend>,
    query: &str,
    as_of: Option<&str>,
    runs: usize,
) -> (f64, usize) {
    let rows = run(store, query, as_of);
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            run(store, query, as_of);
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    t.sort_by(f64::total_cmp);
    (t[runs / 2], rows)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let entities: usize = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(20_000);
    let commits: usize = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(500);
    let runs = 15;
    let queries = [
        (
            "subject lookup",
            format!("SELECT ?p ?o WHERE {{ <{EX}t123> ?p ?o }}"),
        ),
        (
            "value lookup",
            format!("SELECT ?t WHERE {{ ?t <{EX}status> \"blocked\" }}"),
        ),
        (
            "star join",
            format!("SELECT (COUNT(*) AS ?n) WHERE {{ ?t a <{EX}Ticket> ; <{EX}status> \"open\" ; <{EX}owner> <{EX}u7> ; <{EX}priority> ?p }}"),
        ),
        (
            "group by",
            format!("SELECT ?s (COUNT(*) AS ?n) WHERE {{ ?t <{EX}status> ?s }} GROUP BY ?s"),
        ),
    ];
    let stores = [
        ("off", build(Versioning::Off, false, entities, commits)),
        (
            "stamped",
            build(Versioning::Stamped, false, entities, commits),
        ),
        ("log", build(Versioning::Log, false, entities, commits)),
        (
            "log + as-of index",
            build(Versioning::Log, true, entities, commits),
        ),
    ];
    let log_rows: i64 = stores[2].1.store.backend().with_connection(|c| {
        c.query_row("SELECT count(*) FROM quad_log", [], |r| r.get(0))
            .unwrap()
    });
    println!(
        "{entities} tickets ({} triples), {commits} commits, {log_rows} log rows; median of {runs} runs, ms\n",
        entities * 4
    );

    // Writes and size.
    println!("| store | bulk load, ms | one commit, ms | database size, MB |\n|---|---|---|---|");
    let mut writes = Vec::new();
    for (name, b) in &stores {
        println!(
            "| {name} | {:.0} | {:.3} | {:.1} |",
            b.load_ms,
            b.commit_ms,
            b.bytes as f64 / 1_048_576.0
        );
        writes.push(json!({"store": name, "load_ms": b.load_ms, "commit_ms": b.commit_ms, "bytes": b.bytes}));
    }

    // Reads: the present and past versions.
    let middle = format!("HEAD~{}", commits / 2);
    let versions: [(&str, Option<&str>); 3] = [
        ("current", None),
        ("as of HEAD", Some("HEAD")),
        ("as of the middle", Some(middle.as_str())),
    ];
    println!(
        "\n| query | store | current | as of HEAD | as of HEAD~{} | ratio (middle / current) |",
        commits / 2
    );
    println!("|---|---|---|---|---|---|");
    let mut report = Vec::new();
    for (qname, q) in &queries {
        for (sname, b) in &stores {
            if *sname == "stamped" {
                continue;
            }
            let mut cells = Vec::new();
            for (vname, v) in &versions {
                if *sname == "off" && v.is_some() {
                    cells.push(None);
                    continue;
                }
                let (ms, rows) = time(&b.store, q, *v, runs);
                cells.push(Some(ms));
                report.push(json!({
                    "query": qname, "store": sname, "version": vname, "ms": ms, "rows": rows,
                }));
            }
            let fmt = |c: &Option<f64>| c.map_or("—".to_owned(), |v| format!("{v:.2}"));
            let ratio = match (cells[0], cells[2]) {
                (Some(cur), Some(mid)) => format!("{:.1}×", mid / cur.max(1e-3)),
                _ => "—".to_owned(),
            };
            println!(
                "| {qname} | {sname} | {} | {} | {} | {ratio} |",
                fmt(&cells[0]),
                fmt(&cells[1]),
                fmt(&cells[2])
            );
        }
    }

    // How far back: the same query at increasing depths.
    let depths = [0, commits / 10, commits / 2, commits * 9 / 10, commits - 1];
    println!(
        "\n| query | store | {} |",
        depths
            .iter()
            .map(|d| format!("HEAD~{d}"))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    println!("|---|---|{}", "---|".repeat(depths.len()));
    let mut sweep = Vec::new();
    for (qname, q) in queries.iter().filter(|(n, _)| *n != "subject lookup") {
        for (sname, b) in stores.iter().filter(|(n, _)| n.starts_with("log")) {
            let mut cells = Vec::new();
            for d in depths {
                let v = format!("HEAD~{d}");
                let (ms, _) = time(&b.store, q, Some(&v), runs);
                cells.push(format!("{ms:.2}"));
                sweep.push(json!({"query": qname, "store": sname, "depth": d, "ms": ms}));
            }
            println!("| {qname} | {sname} | {} |", cells.join(" | "));
        }
    }

    let name = if entities == 20_000 && commits == 500 {
        "as-of-latency.json".to_owned()
    } else {
        format!("as-of-latency-{entities}-{commits}.json")
    };
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("results")
        .join(name);
    std::fs::write(
        &out,
        serde_json::to_string_pretty(&json!({
            "entities": entities, "triples": entities * 4, "commits": commits,
            "log_rows": log_rows, "runs": runs, "backend": "bundled SQLite, in memory",
            "writes": writes, "results": report, "depth": sweep,
        }))
        .unwrap(),
    )
    .unwrap();
    eprintln!("wrote {}", out.display());
}
