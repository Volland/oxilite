//! Plan checks on the BSBM query templates (`bench/bsbm-tools/queries`): every explore and
//! business-intelligence query compiles to SQL that fits D1's statement limit, runs, and
//! never scans the quad table or a whole quad index.

use oxilite::io::RdfFormat;
use oxilite::sparql::QueryOptions;
use oxilite::store::Store;
use oxilite_core::{Request, Statement, SyncBackend};
use std::fmt::Write;
use std::path::Path;

const INST: &str = "http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/instances/";
const COUNTRY: &str = "http://downlode.org/rdf/iso-3166/countries#";

/// A small dataset with BSBM's vocabulary and shape (types, features, producers, offers,
/// vendors, reviews, persons).
fn data() -> String {
    let mut t = String::from(
        "@prefix bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/> .
         @prefix inst: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/instances/> .
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . @prefix dc: <http://purl.org/dc/elements/1.1/> .
         @prefix foaf: <http://xmlns.com/foaf/0.1/> . @prefix rev: <http://purl.org/stuff/rev#> .
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> . @prefix c: <http://downlode.org/rdf/iso-3166/countries#> .\n",
    );
    let countries = ["US", "DE", "GB", "FR"];
    let words = ["alpha", "beta", "gamma", "delta", "omega"];
    for ty in 2..=6 {
        writeln!(
            t,
            "inst:ProductType{ty} rdfs:subClassOf inst:ProductType1 ; rdfs:label \"type {ty}\" ."
        )
        .unwrap();
    }
    for f in 1..=30 {
        writeln!(
            t,
            "inst:ProductFeature{f} a bsbm:ProductFeature ; rdfs:label \"feature {f}\" ."
        )
        .unwrap();
    }
    for k in 1..=5 {
        writeln!(t, "inst:dataFromProducer{k}/Producer{k} a bsbm:Producer ; rdfs:label \"producer {k}\" ; foaf:homepage <http://producer{k}.example/> ; bsbm:country c:{} .", countries[k % 4]).unwrap();
    }
    for v in 1..=4 {
        writeln!(t, "inst:dataFromVendor{v}/Vendor{v} a bsbm:Vendor ; rdfs:label \"vendor {v}\" ; bsbm:country c:{} ; foaf:homepage <http://vendor{v}.example/> .", countries[v % 4]).unwrap();
    }
    for p in 1..=60 {
        let k = p % 5 + 1;
        let prod = format!("inst:dataFromProducer{k}/Product{p}");
        writeln!(
            t,
            "{prod} a bsbm:Product , inst:ProductType{ty} ; rdfs:label \"{w1} {w2} {p}\" ; rdfs:comment \"a {w2} product\" ; \
             bsbm:producer inst:dataFromProducer{k}/Producer{k} ; dc:publisher inst:dataFromProducer{k}/Producer{k} ; \
             bsbm:productFeature inst:ProductFeature{f1} , inst:ProductFeature{f2} , inst:ProductFeature{f3} ; \
             bsbm:productPropertyNumeric1 {n1} ; bsbm:productPropertyNumeric2 {n2} ; bsbm:productPropertyNumeric3 {n3} ; \
             bsbm:productPropertyTextual1 \"t{p}\" ; bsbm:productPropertyTextual2 \"u{p}\" ; bsbm:productPropertyTextual3 \"v{p}\" .",
            ty = p % 5 + 2,
            w1 = words[p % 5],
            w2 = words[(p / 5) % 5],
            f1 = p % 30 + 1,
            f2 = (p * 7) % 30 + 1,
            f3 = (p * 13) % 30 + 1,
            n1 = p * 17 % 500,
            n2 = p * 31 % 700,
            n3 = p * 7 % 900,
        )
        .unwrap();
        for o in 0..3 {
            let v = (p + o) % 4 + 1;
            writeln!(
                t,
                "inst:dataFromVendor{v}/Offer{p}_{o} a bsbm:Offer ; bsbm:product {prod} ; bsbm:vendor inst:dataFromVendor{v}/Vendor{v} ; \
                 bsbm:price \"{price}.5\"^^bsbm:USD ; bsbm:validFrom \"2008-0{m}-01T00:00:00\"^^xsd:dateTime ; \
                 bsbm:validTo \"2008-1{m}-01T00:00:00\"^^xsd:dateTime ; bsbm:deliveryDays {d} ; bsbm:offerWebpage <http://vendor{v}.example/o{p}_{o}> .",
                price = (p * 37 + o * 11) % 900 + 10,
                m = o + 1,
                d = (p + o) % 7 + 1,
            )
            .unwrap();
        }
        for r in 0..2 {
            let person = (p + r) % 10 + 1;
            writeln!(
                t,
                "inst:dataFromRatingSite1/Review{p}_{r} a rev:Review ; bsbm:reviewFor {prod} ; rev:reviewer inst:dataFromRatingSite1/Reviewer{person} ; \
                 bsbm:reviewDate \"2008-0{m}-15T00:00:00\"^^xsd:dateTime ; dc:title \"review {p}\" ; rev:text \"text {p}\"@en ; \
                 bsbm:rating1 {a} ; bsbm:rating2 {b} ; bsbm:rating3 {a} ; bsbm:rating4 {b} .",
                m = (p + r) % 6 + 4,
                a = (p + r) % 10 + 1,
                b = (p * 3 + r) % 10 + 1,
            )
            .unwrap();
        }
    }
    for person in 1..=10 {
        writeln!(t, "inst:dataFromRatingSite1/Reviewer{person} a foaf:Person ; foaf:name \"person {person}\" ; foaf:mbox_sha1sum \"sha{person}\" ; bsbm:country c:{} .", countries[person % 4]).unwrap();
    }
    // BSBM instance IRIs contain '/', which prefixed names cannot: write them in full.
    let mut out = String::with_capacity(t.len());
    let mut rest = t.as_str();
    while let Some(i) = rest.find("inst:dataFrom") {
        out.push_str(&rest[..i]);
        let name = &rest[i + "inst:".len()..];
        let end = name.find(char::is_whitespace).unwrap_or(name.len());
        write!(out, "<{INST}{}>", &name[..end]).unwrap();
        rest = &name[end..];
    }
    out.push_str(rest);
    out
}

fn substitute(template: &str) -> String {
    let product = format!("<{INST}dataFromProducer2/Product1>");
    [
        ("%ProductXYZ%", product.as_str()),
        ("%Product%", product.as_str()),
        ("%OfferXYZ%", &format!("<{INST}dataFromVendor2/Offer1_0>")),
        (
            "%ReviewXYZ%",
            &format!("<{INST}dataFromRatingSite1/Review1_0>"),
        ),
        ("%ProductType%", &format!("<{INST}ProductType3>")),
        ("%ProductFeature1%", &format!("<{INST}ProductFeature2>")),
        ("%ProductFeature2%", &format!("<{INST}ProductFeature8>")),
        ("%ProductFeature3%", &format!("<{INST}ProductFeature14>")),
        (
            "%Producer%",
            &format!("<{INST}dataFromProducer2/Producer2>"),
        ),
        ("%Country%", &format!("<{COUNTRY}US>")),
        ("%Country1%", &format!("<{COUNTRY}US>")),
        ("%Country2%", &format!("<{COUNTRY}DE>")),
        ("%x%", "100"),
        ("%y%", "500"),
        ("%word1%", "alpha"),
        (
            "%currentDate%",
            "\"2008-02-15T00:00:00\"^^<http://www.w3.org/2001/XMLSchema#dateTime>",
        ),
        ("%ConsecutiveMonth_0%", "2008-04-01"),
        ("%ConsecutiveMonth_1%", "2008-05-01"),
        ("%ConsecutiveMonth_2%", "2008-06-01"),
    ]
    .iter()
    .fold(template.to_string(), |q, (k, v)| q.replace(k, v))
}

fn plan_lines<B: SyncBackend + Send + Sync + 'static>(store: &Store<B>, sql: &str) -> Vec<String> {
    let rs = store
        .backend()
        .execute(&Request::read(vec![Statement::new(format!(
            "EXPLAIN QUERY PLAN {sql}"
        ))]))
        .unwrap();
    rs[0]
        .rows
        .iter()
        .filter_map(|r| r.get(3).and_then(|d| d.clone().into_string()))
        .collect()
}

// @lat: [[tests#Planner#BSBM plans use indexes]]
#[test]
fn bsbm_queries_use_indexes_and_fit_d1() {
    let store = Store::new().unwrap();
    store
        .load_from_slice(RdfFormat::Turtle, data().as_bytes())
        .unwrap();
    store.optimize().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/bsbm-tools/queries");
    let mut failures = Vec::new();
    let mut checked = 0;
    for (mix, queries) in [("explore", 1..=12), ("bi", 1..=8)] {
        for n in queries {
            let Ok(template) =
                std::fs::read_to_string(root.join(mix).join(format!("query{n}.txt")))
            else {
                continue;
            };
            let query = substitute(&template);
            let name = format!("{mix} Q{n}");
            let explain = store
                .explain(query.as_str())
                .unwrap_or_else(|e| format!("error {e}"));
            if !explain.starts_with("-- oxilite: fully compiled") {
                // DESCRIBE and CONSTRUCT forms run in steps; everything else must be one statement.
                if !template.contains("DESCRIBE") && !template.contains("CONSTRUCT") {
                    failures.push(format!(
                        "{name}: not fully compiled: {}",
                        explain.lines().next().unwrap_or("")
                    ));
                }
                continue;
            }
            let sql: String = explain
                .lines()
                .filter(|l| !l.starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            if sql.len() > 90_000 {
                failures.push(format!(
                    "{name}: {} bytes of SQL, over D1's limit",
                    sql.len()
                ));
            }
            for line in plan_lines(&store, &sql) {
                let scan = line.starts_with("SCAN q") || line.starts_with("SCAN quads");
                if scan {
                    failures.push(format!("{name}: {line}"));
                }
            }
            if let Err(e) = store.query_output(query.as_str(), &QueryOptions::default()) {
                failures.push(format!("{name}: {e}"));
            }
            checked += 1;
        }
    }
    assert!(
        checked >= 18,
        "only {checked} BSBM queries checked (git submodule update --init bench/bsbm-tools)"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
