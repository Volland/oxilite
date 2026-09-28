//! The agent-memory guide on the website (`site/articles/agent-memory-turso.html`): every query
//! it shows, on the data it shows, with the answers it prints.
#![cfg(feature = "turso")]

use oxilite::cypher::{CypherOptions, Params, Value, Vocabulary};
use oxilite::functions::HostFunction;
use oxilite::io::RdfFormat;
use oxilite::model::{Literal, NamedNode, Term};
use oxilite::sparql::QueryResults;
use oxilite::store::Store;
use oxilite::turso::TursoBackend;
use oxilite::vector::{Metric, VectorIndex};

const MEMORY: &str = r#"@prefix ex:  <http://example.com/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:alice   a ex:Person  ; ex:name "Alice" ; ex:worksFor ex:acme .
ex:bob     a ex:Person  ; ex:name "Bob"   ; ex:worksFor ex:acme .
ex:carol   a ex:Person  ; ex:name "Carol" ; ex:worksFor ex:globex .
ex:acme    a ex:Company ; ex:name "ACME" .
ex:globex  a ex:Company ; ex:name "Globex" .
ex:offsite a ex:Event   ; ex:name "ACME offsite" ; ex:city "Lisbon" ; ex:on "2026-10-05"^^xsd:date .

ex:chat12  a ex:Conversation ; ex:channel "slack" .
ex:mail7   a ex:Conversation ; ex:channel "email" .

ex:m1 a ex:Memory ; ex:text "Alice prefers aisle seats on long flights" ;
  ex:about ex:alice ; ex:visibleTo ex:alice ; ex:at "2026-09-01"^^xsd:date ;
  ex:source ex:chat12 ; ex:embedding "[0.9, 0.1, 0.0, 0.4]" .
ex:m2 a ex:Memory ; ex:text "Alice is flying to Lisbon for the ACME offsite" ;
  ex:about ex:alice , ex:offsite ; ex:visibleTo ex:alice , ex:bob ; ex:at "2026-09-20"^^xsd:date ;
  ex:source ex:mail7 ; ex:embedding "[0.8, 0.3, 0.1, 0.0]" .
ex:m3 a ex:Memory ; ex:text "Bob is allergic to peanuts" ;
  ex:about ex:bob ; ex:visibleTo ex:alice , ex:bob ; ex:at "2026-08-11"^^xsd:date ;
  ex:source ex:chat12 ; ex:embedding "[0.0, 0.9, 0.1, 0.2]" .
ex:m4 a ex:Memory ; ex:text "Bob is also going to the ACME offsite" ;
  ex:about ex:bob , ex:offsite ; ex:visibleTo ex:alice , ex:bob ; ex:at "2026-09-21"^^xsd:date ;
  ex:source ex:chat12 ; ex:embedding "[0.7, 0.2, 0.2, 0.1]" .
ex:m5 a ex:Memory ; ex:text "Carol booked a trip to Lisbon in October" ;
  ex:about ex:carol ; ex:visibleTo ex:carol ; ex:at "2026-09-18"^^xsd:date ;
  ex:source ex:mail7 ; ex:embedding "[0.85, 0.15, 0.0, 0.1]" .
ex:m6 a ex:Memory ; ex:text "The offsite agenda is due on Friday" ;
  ex:about ex:offsite ; ex:visibleTo ex:alice , ex:bob ; ex:at "2026-09-24"^^xsd:date ;
  ex:source ex:mail7 ; ex:embedding "[0.3, 0.0, 0.9, 0.0]" .
"#;

const PREFIXES: &str = "PREFIX oxl: <https://oxilite.dev/ns#>
PREFIX ex:  <http://example.com/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
PREFIX fn:  <http://example.com/fn#>
";

/// "What do I know about upcoming travel?", embedded.
const QUESTION: &str = "[0.85, 0.2, 0.05, 0.1]";

fn ex(local: &str) -> NamedNode {
    NamedNode::new(format!("http://example.com/{local}")).unwrap()
}

/// The guide's store: the data loaded and the index created from Rust.
fn memory() -> Store<TursoBackend> {
    let store = Store::new_turso().unwrap();
    store
        .load_from_reader(RdfFormat::Turtle, MEMORY.as_bytes())
        .unwrap();
    store
        .create_vector_index(
            &VectorIndex::new("memories", ex("embedding"), 4)
                .metric(Metric::Cosine)
                .class(ex("Memory")),
        )
        .unwrap();
    store
}

/// The rows of a SELECT, each value as a plain string (a literal's lexical form, an IRI's
/// local name under `ex:`).
fn select(store: &Store<TursoBackend>, body: &str) -> Vec<Vec<String>> {
    let q = format!("{PREFIXES}{body}");
    let QueryResults::Solutions(s) = store
        .query(q.as_str())
        .unwrap_or_else(|e| panic!("{body}: {e}"))
    else {
        panic!("not a SELECT")
    };
    let vars = s.variables().to_vec();
    s.map(|row| {
        let row = row.unwrap();
        vars.iter()
            .map(|v| match row.get(v) {
                Some(Term::Literal(l)) => l.value().to_owned(),
                Some(Term::NamedNode(n)) => n
                    .as_str()
                    .strip_prefix("http://example.com/")
                    .unwrap_or(n.as_str())
                    .to_owned(),
                Some(t) => t.to_string(),
                None => String::new(),
            })
            .collect()
    })
    .collect()
}

fn column(rows: &[Vec<String>], i: usize) -> Vec<&str> {
    rows.iter().map(|r| r[i].as_str()).collect()
}

/// The guide's helper: an embedding as the JSON array literal the index reads.
fn vector_literal(v: &[f32]) -> String {
    let items: Vec<String> = v.iter().map(f32::to_string).collect();
    format!("[{}]", items.join(", "))
}

/// The guide's `remember` tool.
fn remember(
    store: &Store<TursoBackend>,
    id: &str,
    user: &str,
    text: &str,
    about: &[&str],
    at: &str,
    embedding: &[f32],
) -> oxilite::Result<()> {
    let about: Vec<String> = about.iter().map(|a| format!("<{a}>")).collect();
    store.update(format!(
        r#"PREFIX ex: <http://example.com/>
           PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
           INSERT DATA {{
             <{id}> a ex:Memory ;
               ex:text {text} ;
               ex:about {about} ;
               ex:visibleTo <{user}> ;
               ex:at "{at}"^^xsd:date ;
               ex:embedding "{vector}" .
           }}"#,
        text = Literal::new_simple_literal(text),
        about = about.join(", "),
        vector = vector_literal(embedding),
    ))
}

/// The guide's `recall` tool: over-fetch, keep what the user may see, cut to `k`.
fn recall(
    store: &Store<TursoBackend>,
    user: &str,
    question: &[f32],
    k: usize,
) -> oxilite::Result<Vec<(String, f64)>> {
    let q = format!(
        r#"PREFIX oxl: <https://oxilite.dev/ns#>
           PREFIX ex:  <http://example.com/>
           SELECT ?text ?score WHERE {{
             SERVICE <oxilite:vector/memories> {{
               [] oxl:query "{vector}" ; oxl:k {fetch} ; oxl:node ?m ; oxl:score ?score .
             }}
             ?m ex:visibleTo <{user}> ; ex:text ?text .
           }}
           ORDER BY DESC(?score) LIMIT {k}"#,
        vector = vector_literal(question),
        fetch = k * 4,
    );
    let QueryResults::Solutions(rows) = store.query(q.as_str())? else {
        unreachable!("a SELECT")
    };
    let mut out = Vec::new();
    for row in rows {
        let row = row?;
        if let (Some(Term::Literal(text)), Some(Term::Literal(score))) =
            (row.get("text"), row.get("score"))
        {
            out.push((
                text.value().to_owned(),
                score.value().parse().unwrap_or(0.0),
            ));
        }
    }
    Ok(out)
}

// @lat: [[tests#Agent memory guide#Recall recipes]]
#[test]
fn recall_recipes() {
    let store = memory();

    // Plain top k: Carol's private memory is the nearest.
    let rows = select(
        &store,
        &format!(
            r#"SELECT ?memory ?text ?score WHERE {{
              SERVICE <oxilite:vector/memories> {{
                [] oxl:query "{QUESTION}" ; oxl:k 5 ; oxl:node ?memory ; oxl:score ?score .
              }}
              ?memory ex:text ?text .
            }} ORDER BY DESC(?score)"#
        ),
    );
    assert_eq!(column(&rows, 0), ["m5", "m2", "m4", "m1", "m6"]);
    assert!(rows[0][2].starts_with("0.9984"), "{rows:?}");

    // Scoped to what Alice may see: over-fetch, filter, cut.
    let rows = select(
        &store,
        &format!(
            r#"SELECT ?text ?score WHERE {{
              SERVICE <oxilite:vector/memories> {{
                [] oxl:query "{QUESTION}" ; oxl:k 20 ; oxl:node ?m ; oxl:score ?score .
              }}
              ?m ex:visibleTo ex:alice ; ex:text ?text .
            }} ORDER BY DESC(?score) LIMIT 3"#
        ),
    );
    assert_eq!(
        column(&rows, 0),
        [
            "Alice is flying to Lisbon for the ACME offsite",
            "Bob is also going to the ACME offsite",
            "Alice prefers aisle seats on long flights"
        ]
    );

    // k applies before the graph patterns: 3 nearest, then the filter, leaves 2.
    let rows = select(
        &store,
        &format!(
            r#"SELECT ?m WHERE {{
              SERVICE <oxilite:vector/memories> {{ [] oxl:query "{QUESTION}" ; oxl:k 3 ; oxl:node ?m }}
              ?m ex:visibleTo ex:alice .
            }}"#
        ),
    );
    assert_eq!(rows.len(), 2);

    // Recent and relevant.
    let rows = select(
        &store,
        &format!(
            r#"SELECT ?text ?at WHERE {{
              SERVICE <oxilite:vector/memories> {{
                [] oxl:query "{QUESTION}" ; oxl:k 20 ; oxl:node ?m ; oxl:score ?score .
              }}
              ?m ex:visibleTo ex:alice ; ex:text ?text ; ex:at ?at .
              FILTER(?at >= "2026-09-14"^^xsd:date && ?score > 0.8)
            }} ORDER BY DESC(?score)"#
        ),
    );
    assert_eq!(column(&rows, 1), ["2026-09-20", "2026-09-21"]);

    // Expand along the graph: the event the recalled memories are about, everyone going,
    // and everything Alice may see about them.
    let mut rows = select(
        &store,
        &format!(
            r#"SELECT DISTINCT ?who ?fact WHERE {{
              SERVICE <oxilite:vector/memories> {{
                [] oxl:query "{QUESTION}" ; oxl:k 20 ; oxl:node ?hit ; oxl:score ?score .
              }}
              FILTER(?score > 0.9)
              ?hit ex:visibleTo ex:alice ; ex:about ?event .
              ?event a ex:Event .
              ?other ex:about ?event , ?person .
              ?person a ex:Person ; ex:name ?who .
              ?m ex:about ?person ; ex:text ?fact ; ex:visibleTo ex:alice .
            }}"#
        ),
    );
    rows.sort();
    assert_eq!(
        rows,
        [
            ["Alice", "Alice is flying to Lisbon for the ACME offsite"],
            ["Alice", "Alice prefers aisle seats on long flights"],
            ["Bob", "Bob is allergic to peanuts"],
            ["Bob", "Bob is also going to the ACME offsite"],
        ]
    );

    // Provenance: chained single-pattern OPTIONALs.
    let rows = select(
        &store,
        &format!(
            r#"SELECT ?text ?channel WHERE {{
              SERVICE <oxilite:vector/memories> {{
                [] oxl:query "{QUESTION}" ; oxl:k 3 ; oxl:node ?m ; oxl:distance ?d .
              }}
              ?m ex:text ?text .
              OPTIONAL {{ ?m ex:source ?src }}
              OPTIONAL {{ ?src ex:channel ?channel }}
            }} ORDER BY ?d"#
        ),
    );
    assert_eq!(column(&rows, 1), ["email", "email", "slack"]);

    // Who the recalled memories are about.
    let rows = select(
        &store,
        &format!(
            r#"SELECT ?name (COUNT(?m) AS ?memories) (MAX(?score) AS ?best) WHERE {{
              SERVICE <oxilite:vector/memories> {{
                [] oxl:query "{QUESTION}" ; oxl:k 10 ; oxl:node ?m ; oxl:score ?score .
              }}
              ?m ex:about ?p . ?p a ex:Person ; ex:name ?name .
            }} GROUP BY ?name ORDER BY DESC(?best)"#
        ),
    );
    assert_eq!(column(&rows, 0), ["Carol", "Alice", "Bob"]);
    assert_eq!(column(&rows, 1), ["1", "2", "2"]);

    // A property path from each hit.
    let mut rows = select(
        &store,
        &format!(
            r#"SELECT DISTINCT ?text ?company WHERE {{
              SERVICE <oxilite:vector/memories> {{ [] oxl:query "{QUESTION}" ; oxl:k 4 ; oxl:node ?m }}
              ?m ex:text ?text ; ex:about/ex:worksFor/ex:name ?company .
            }}"#
        ),
    );
    rows.sort();
    assert_eq!(column(&rows, 1), ["ACME", "ACME", "ACME", "Globex"]);

    // The search is part of the one statement.
    let explain = store
        .explain(
            format!(
                r#"{PREFIXES}SELECT ?text WHERE {{
                  SERVICE <oxilite:vector/memories> {{ [] oxl:query "{QUESTION}" ; oxl:k 3 ; oxl:node ?m }}
                  ?m ex:text ?text }}"#
            )
            .as_str(),
        )
        .unwrap();
    assert!(explain.contains("fully compiled to SQL"), "{explain}");
    assert!(explain.contains("vector_distance_cos"), "{explain}");
}

// @lat: [[tests#Agent memory guide#Remember consolidate forget]]
#[test]
fn remember_consolidate_forget() {
    let store = memory();
    let rows = |s: &Store<TursoBackend>| s.vector_indexes().unwrap()[0].rows;
    assert_eq!(rows(&store), 6);

    remember(
        &store,
        "http://example.com/m7",
        "http://example.com/alice",
        "Alice asked for an aisle seat again",
        &["http://example.com/alice"],
        "2026-09-27",
        &[0.9, 0.1, 0.0, 0.38],
    )
    .unwrap();
    assert_eq!(rows(&store), 7);

    // A wrong-length embedding fails the whole write.
    let err = remember(
        &store,
        "http://example.com/m8",
        "http://example.com/alice",
        "broken",
        &["http://example.com/alice"],
        "2026-09-27",
        &[0.1, 0.2],
    )
    .unwrap_err();
    assert!(err.to_string().contains("expects 4 dimensions"), "{err}");
    assert!(select(&store, "SELECT ?t WHERE { ex:m8 ex:text ?t }").is_empty());

    // Consolidate: a near-duplicate about the same person.
    let found = select(
        &store,
        r#"SELECT ?older ?text ?score WHERE {
          SERVICE <oxilite:vector/memories> {
            [] oxl:query ex:m7 ; oxl:k 3 ; oxl:node ?older ; oxl:score ?score .
          }
          ?older ex:text ?text ; ex:about ?who .
          ex:m7 ex:about ?who .
          FILTER(?older != ex:m7 && ?score > 0.99)
        }"#,
    );
    assert_eq!(column(&found, 0), ["m1"]);
    assert!(found[0][2].starts_with("0.9999"), "{found:?}");

    // Forget: the index follows the delete.
    store
        .update("PREFIX ex: <http://example.com/> DELETE WHERE { ex:m7 ?p ?o }")
        .unwrap();
    assert_eq!(rows(&store), 6);

    // The recall tool, for Alice.
    let hits = recall(
        &store,
        "http://example.com/alice",
        &[0.85, 0.2, 0.05, 0.1],
        2,
    )
    .unwrap();
    assert_eq!(
        hits.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        [
            "Alice is flying to Lisbon for the ACME offsite",
            "Bob is also going to the ACME offsite"
        ]
    );
    assert!(hits[0].1 > hits[1].1);
}

/// Days since the civil epoch of an `xsd:date` literal (the guide uses chrono for this).
fn day_number(t: &Term) -> Option<i64> {
    let Term::Literal(l) = t else { return None };
    let mut parts = l.value().splitn(3, '-').map(str::parse::<i64>);
    let (y, m, d) = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    Some(365 * y + y / 4 - y / 100 + y / 400 + (153 * (m - 3) + 2) / 5 + d)
}

// @lat: [[tests#Agent memory guide#Recency decay host function]]
#[test]
fn recency_decay_host_function() {
    let store = memory();
    store
        .register_function(
            HostFunction::new("http://example.com/fn#decay", |args| {
                let age = (day_number(args.get(1)?)? - day_number(args.first()?)?) as f64;
                Some(Literal::from(0.5_f64.powf(age / 14.0)).into())
            })
            .cypher_name("fn.decay")
            .arity(2, 2)
            .description("half-life weighting: 1.0 today, 0.5 after 14 days"),
        )
        .unwrap();
    let rows = select(
        &store,
        &format!(
            r#"SELECT ?text ?rank WHERE {{
              SERVICE <oxilite:vector/memories> {{
                [] oxl:query "{QUESTION}" ; oxl:k 20 ; oxl:node ?m ; oxl:score ?score .
              }}
              ?m ex:visibleTo ex:alice ; ex:text ?text ; ex:at ?at .
              BIND(?score * fn:decay(?at, "2026-09-28"^^xsd:date) AS ?rank)
            }} ORDER BY DESC(?rank) LIMIT 3"#
        ),
    );
    assert_eq!(
        column(&rows, 0),
        [
            "Bob is also going to the ACME offsite",
            "Alice is flying to Lisbon for the ACME offsite",
            "The offsite agenda is due on Friday"
        ]
    );
}

// @lat: [[tests#Agent memory guide#Cypher and Datalog recall]]
#[test]
fn cypher_and_datalog_recall() {
    let store = memory();
    let options = CypherOptions {
        vocabulary: Vocabulary::new("http://example.com/"),
        ..Default::default()
    };
    let mut params = Params::new();
    params.insert(
        "question".into(),
        Value::List(
            [0.85, 0.2, 0.05, 0.1]
                .into_iter()
                .map(Value::Float)
                .collect(),
        ),
    );
    let r = store
        .cypher_with(
            "CALL db.index.vector.queryNodes('memories', 10, $question) YIELD node, score
             MATCH (node)-[:visibleTo]->(:Person {name: 'Alice'})
             MATCH (node)-[:about]->(p:Person)
             RETURN node.text AS memory, p.name AS about, score
             ORDER BY score DESC LIMIT 3",
            &params,
            &options,
        )
        .unwrap();
    let pairs: Vec<(Value, Value)> = r
        .rows
        .iter()
        .map(|row| (row[0].clone(), row[1].clone()))
        .collect();
    let s = |v: &str| Value::String(v.into());
    assert_eq!(
        pairs,
        [
            (
                s("Alice is flying to Lisbon for the ACME offsite"),
                s("Alice")
            ),
            (s("Bob is also going to the ACME offsite"), s("Bob")),
            (s("Alice prefers aisle seats on long flights"), s("Alice")),
        ]
    );

    let r = store
        .datalog(
            r#"@prefix ex: <http://example.com/> .

recalled(?m)     :- nearest("memories", "[0.85, 0.2, 0.05, 0.1]", 3, ?m, ?rank).
seen(?m)         :- recalled(?m), ex:visibleTo(?m, ex:alice).
event(?e)        :- seen(?m), ex:about(?m, ?e), ex:Event(?e).
attendee(?p)     :- event(?e), ex:about(?m, ?e), ex:about(?m, ?p), ex:Person(?p).
unseen(?who, ?t) :- attendee(?p), ex:name(?p, ?who), ex:about(?m, ?p), ex:text(?m, ?t),
                    ex:visibleTo(?m, ex:alice), not seen(?m).

?- unseen(?who, ?t)."#,
        )
        .unwrap();
    let mut rows: Vec<(String, String)> = (0..r.rows.len())
        .map(|i| {
            let text = |v| match r.get(i, v) {
                Some(Term::Literal(l)) => l.value().to_owned(),
                other => format!("{other:?}"),
            };
            (text("who"), text("t"))
        })
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        [
            (
                "Alice".into(),
                "Alice prefers aisle seats on long flights".into()
            ),
            ("Bob".into(), "Bob is allergic to peanuts".into()),
        ]
    );
}
