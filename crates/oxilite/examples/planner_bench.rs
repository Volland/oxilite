//! Planner benchmark: oxilite's statistics-driven join order vs. SQLite's own planner.
//!
//! `cargo run --release -p oxilite --example planner_bench [people]`
//!
// @lat: [[tests#Planner benchmark]]

use oxilite::model::*;
use oxilite::sparql::QueryResults;
use oxilite::store::Store;
use oxilite::QueryOptions;
use std::time::Instant;

fn ex(s: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("http://example.com/{s}"))
}

fn count(r: QueryResults<'_>) -> usize {
    match r {
        QueryResults::Solutions(s) => s.count(),
        _ => 0,
    }
}

fn main() -> oxilite::Result<()> {
    let people: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(50_000);
    let store = Store::new()?;
    let mut quads = Vec::new();
    let (person, knows, name, age, city, badge) = (
        ex("Person"),
        ex("knows"),
        ex("name"),
        ex("age"),
        ex("city"),
        ex("badge"),
    );
    for i in 0..people {
        let p = ex(&format!("p{i}"));
        quads.push(Quad::new(
            p.clone(),
            vocab::rdf::TYPE,
            person.clone(),
            GraphName::DefaultGraph,
        ));
        quads.push(Quad::new(
            p.clone(),
            name.clone(),
            Literal::from(format!("Person {i}")),
            GraphName::DefaultGraph,
        ));
        quads.push(Quad::new(
            p.clone(),
            age.clone(),
            Literal::from((i % 90) as i64),
            GraphName::DefaultGraph,
        ));
        quads.push(Quad::new(
            p.clone(),
            city.clone(),
            ex(&format!("city{}", i % 100)),
            GraphName::DefaultGraph,
        ));
        for k in 1..=3 {
            quads.push(Quad::new(
                p.clone(),
                knows.clone(),
                ex(&format!("p{}", (i * 7 + k * 13) % people)),
                GraphName::DefaultGraph,
            ));
        }
        if i % 5_000 == 0 {
            quads.push(Quad::new(
                p.clone(),
                badge.clone(),
                Literal::from("gold"),
                GraphName::DefaultGraph,
            ));
        }
    }
    let t = Instant::now();
    let mut loader = store.bulk_loader();
    loader.load_quads(quads)?;
    loader.commit()?; // also runs optimize()
    println!("loaded {} quads in {:.2?}", store.len()?, t.elapsed());

    let queries = [
        (
            "rare-badge-star",
            "SELECT ?n ?a WHERE { ?p a <http://example.com/Person> ; <http://example.com/name> ?n ; <http://example.com/age> ?a ; <http://example.com/badge> \"gold\" }",
        ),
        (
            "friends-of-badged",
            "SELECT ?fn WHERE { ?p <http://example.com/badge> \"gold\" . ?p <http://example.com/knows> ?f . ?f a <http://example.com/Person> ; <http://example.com/name> ?fn }",
        ),
        (
            "city-age-filter",
            "SELECT ?p WHERE { ?p a <http://example.com/Person> ; <http://example.com/city> <http://example.com/city7> ; <http://example.com/age> ?a FILTER(?a > 80) }",
        ),
    ];
    println!("| query | oxilite planner | SQLite planner | rows |");
    println!("|---|---|---|---|");
    for (name, q) in queries {
        let mut timings = Vec::new();
        let mut rows = 0;
        for sqlite_planner in [false, true] {
            let options = QueryOptions {
                sqlite_planner,
                ..QueryOptions::default()
            };
            let _ = count(store.query_opt(q, options.clone())?); // warm-up
            let t = Instant::now();
            for _ in 0..3 {
                rows = count(store.query_opt(q, options.clone())?);
            }
            timings.push(t.elapsed() / 3);
        }
        println!(
            "| {name} | {:.2?} | {:.2?} | {rows} |",
            timings[0], timings[1]
        );
    }
    Ok(())
}
