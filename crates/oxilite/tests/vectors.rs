//! Vector indexes on Turso: definitions as RDF, trigger-maintained tables, and search from the
//! store API, SPARQL, Cypher and Datalog.
#![cfg(feature = "turso")]

use oxilite::model::{NamedNode, Term};
use oxilite::sparql::QueryResults;
use oxilite::store::Store;
use oxilite::turso::TursoBackend;
use oxilite::vector::{ElementType, Metric, QueryVector, VectorIndex};

const PREFIXES: &str = "PREFIX ex: <http://example.com/> PREFIX oxl: <https://oxilite.dev/ns#> ";

fn ex(l: &str) -> NamedNode {
    NamedNode::new(format!("http://example.com/{l}")).unwrap()
}

fn docs() -> Store<TursoBackend> {
    let store = Store::new_turso().unwrap();
    store
        .update(format!(
            r#"{PREFIXES} INSERT DATA {{
                ex:a ex:embedding "[1, 0, 0]" ; ex:title "Alpha" ; ex:year 2021 .
                ex:b ex:embedding "[0.7, 0.7, 0]" ; ex:title "Beta" ; ex:year 2019 ; a ex:Doc .
                ex:c ex:embedding "[0, 1, 0]" ; ex:title "Gamma" ; ex:year 2022 .
            }}"#
        ))
        .unwrap();
    store
}

fn index() -> VectorIndex {
    VectorIndex::new("docs", ex("embedding"), 3)
}

fn nodes(hits: &[oxilite::vector::VectorHit]) -> Vec<String> {
    hits.iter().map(|h| h.node.to_string()).collect()
}

fn select(store: &Store<TursoBackend>, q: &str) -> Vec<Vec<Option<Term>>> {
    let QueryResults::Solutions(s) = store.query(format!("{PREFIXES}{q}").as_str()).unwrap() else {
        panic!("not a SELECT")
    };
    let vars = s.variables().to_vec();
    s.map(|r| {
        let r = r.unwrap();
        vars.iter().map(|v| r.get(v).cloned()).collect()
    })
    .collect()
}

// @lat: [[tests#Vector indexes#Creation back-fills and search orders by distance]]
#[test]
fn creation_backfills_and_search_orders() {
    let store = docs();
    store.create_vector_index(&index()).unwrap();
    let hits = store
        .vector_search("docs", &QueryVector::vector(&[1.0, 0.1, 0.0]), 2)
        .unwrap();
    assert_eq!(
        nodes(&hits),
        ["<http://example.com/a>", "<http://example.com/b>"]
    );
    assert!(hits[0].distance < hits[1].distance);
    assert!((hits[0].score - (1.0 - hits[0].distance / 2.0)).abs() < 1e-9);
    // By node: the node itself first, at distance 0.
    let hits = store
        .vector_search("docs", &QueryVector::node(ex("a")), 3)
        .unwrap();
    assert_eq!(hits[0].node, Term::from(ex("a")));
    assert!(hits[0].distance.abs() < 1e-6);
    let info = store.vector_indexes().unwrap();
    assert_eq!(info.len(), 1);
    assert!(info[0].built);
    assert_eq!(info[0].rows, 3);
    // The definition is RDF.
    let defs = select(
        &store,
        "SELECT ?i WHERE { GRAPH <oxilite:vectors> { ?i a oxl:VectorIndex } }",
    );
    assert_eq!(
        defs,
        vec![vec![Some(
            NamedNode::new("oxilite:vector/docs").unwrap().into()
        )]]
    );
}

// @lat: [[tests#Vector indexes#Writes keep the index current]]
#[test]
fn writes_keep_the_index_current() {
    let store = docs();
    store.create_vector_index(&index()).unwrap();
    let near = |s: &Store<TursoBackend>| {
        nodes(
            &s.vector_search("docs", &QueryVector::vector(&[0.0, 0.0, 1.0]), 1)
                .unwrap(),
        )
    };
    store
        .update(format!(
            r#"{PREFIXES} INSERT DATA {{ GRAPH ex:g {{ ex:d ex:embedding "[0, 0, 1]" }} }}"#
        ))
        .unwrap();
    assert_eq!(near(&store), ["<http://example.com/d>"]);
    store.update("CLEAR GRAPH <http://example.com/g>").unwrap();
    assert_ne!(near(&store), ["<http://example.com/d>"]);
    // A write rolled back with its request leaves nothing behind.
    let failed = store.update(format!(
        r#"{PREFIXES} INSERT DATA {{ ex:e ex:embedding "[0, 0, 1]" }} ; CLEAR GRAPH <http://example.com/missing>"#
    ));
    assert!(failed.is_err());
    assert_ne!(near(&store), ["<http://example.com/e>"]);
    // Cypher writes go through the same triggers.
    let options = oxilite::cypher::CypherOptions {
        vocabulary: oxilite::cypher::Vocabulary::new("http://example.com/"),
        ..Default::default()
    };
    store
        .cypher_with(
            "CREATE (:Doc {embedding: '[0, 0.1, 1]'})",
            &Default::default(),
            &options,
        )
        .unwrap();
    assert_eq!(store.vector_indexes().unwrap()[0].rows, 4);
    // Deleting through the store API.
    store
        .update(format!(
            "{PREFIXES} DELETE WHERE {{ ex:a ex:embedding ?e }}"
        ))
        .unwrap();
    assert_eq!(store.vector_indexes().unwrap()[0].rows, 3);
}

// @lat: [[tests#Vector indexes#Malformed embeddings abort the write]]
#[test]
fn malformed_embeddings_abort_the_write() {
    let store = docs();
    store.create_vector_index(&index()).unwrap();
    let err = store
        .update(format!(
            r#"{PREFIXES} INSERT DATA {{ ex:x ex:embedding "[1, 2]" }}"#
        ))
        .unwrap_err();
    assert!(err.to_string().contains("expects 3 dimensions"), "{err}");
    assert_eq!(
        select(&store, "SELECT ?e WHERE { ex:x ex:embedding ?e }").len(),
        0
    );
    assert!(store
        .update(format!(
            r#"{PREFIXES} INSERT DATA {{ ex:x ex:embedding "nope" }}"#
        ))
        .is_err());
    // Creation over bad data fails and leaves nothing.
    let store = docs();
    store
        .update(format!(
            r#"{PREFIXES} INSERT DATA {{ ex:bad ex:embedding "[1]" }}"#
        ))
        .unwrap();
    let err = store.create_vector_index(&index()).unwrap_err();
    assert!(err.to_string().contains("http://example.com/bad"), "{err}");
    assert!(store.vector_indexes().unwrap().is_empty());
    assert_eq!(
        select(
            &store,
            "SELECT ?s WHERE { GRAPH <oxilite:vectors> { ?s ?p ?o } }"
        )
        .len(),
        0
    );
    // Search checks the query too.
    let store = docs();
    store.create_vector_index(&index()).unwrap();
    let err = store
        .vector_search("docs", &QueryVector::vector(&[1.0, 2.0]), 1)
        .unwrap_err();
    assert!(err.to_string().contains("the index 3"), "{err}");
}

// @lat: [[tests#Vector indexes#Definitions from SPARQL Update]]
#[test]
fn definitions_from_sparql_update() {
    let store = docs();
    store
        .update(format!(
            "{PREFIXES} INSERT DATA {{ GRAPH <oxilite:vectors> {{
                <oxilite:vector/docs> a oxl:VectorIndex ; oxl:indexName \"docs\" ;
                    oxl:property ex:embedding ; oxl:dimensions 3 ; oxl:metric oxl:Euclidean .
            }} }}"
        ))
        .unwrap();
    let info = store.vector_indexes().unwrap();
    assert!(info[0].built);
    assert_eq!(info[0].index.metric, Metric::Euclidean);
    let hits = store
        .vector_search("docs", &QueryVector::vector(&[1.0, 0.0, 0.0]), 1)
        .unwrap();
    assert!(hits[0].distance.abs() < 1e-6);
    // Changing the metric rebuilds with the new one.
    store
        .update(format!(
            "{PREFIXES} DELETE {{ GRAPH <oxilite:vectors> {{ ?i oxl:metric ?m }} }}
             INSERT {{ GRAPH <oxilite:vectors> {{ ?i oxl:metric oxl:DotProduct }} }}
             WHERE {{ GRAPH <oxilite:vectors> {{ ?i oxl:metric ?m }} }}"
        ))
        .unwrap();
    let hits = store
        .vector_search("docs", &QueryVector::vector(&[1.0, 0.0, 0.0]), 1)
        .unwrap();
    assert!((hits[0].distance + 1.0).abs() < 1e-6, "{hits:?}");
    // Deleting the definition drops the table.
    store
        .update("DELETE WHERE { GRAPH <oxilite:vectors> { <oxilite:vector/docs> ?p ?o } }")
        .unwrap();
    assert!(store.vector_indexes().unwrap().is_empty());
    assert!(store.stats().vector_built.is_empty());
}

// @lat: [[tests#Vector indexes#A loaded definition is built on sync]]
#[test]
fn loaded_definition_is_built_on_sync() {
    let source = docs();
    source.create_vector_index(&index()).unwrap();
    let mut dump = Vec::new();
    source
        .dump_to_writer(oxilite::io::RdfFormat::NQuads, &mut dump)
        .unwrap();
    let target = Store::new_turso().unwrap();
    target
        .load_from_reader(oxilite::io::RdfFormat::NQuads, dump.as_slice())
        .unwrap();
    assert!(!target.vector_indexes().unwrap()[0].built);
    assert!(target.sync_vector_indexes().unwrap());
    let info = target.vector_indexes().unwrap();
    assert!(info[0].built);
    assert_eq!(info[0].rows, 3);
}

// @lat: [[tests#Vector indexes#Refused without vector functions]]
#[test]
fn refused_without_vector_functions() {
    let store = Store::new().unwrap();
    let err = store.create_vector_index(&index()).unwrap_err();
    assert!(
        err.is_unsupported() && err.to_string().contains("Turso"),
        "{err}"
    );
    assert_eq!(store.len().unwrap(), 0);
    let err = store
        .query(format!("{PREFIXES} SELECT ?n WHERE {{ SERVICE <oxilite:vector/docs> {{ [] oxl:query \"[1,0,0]\" ; oxl:node ?n }} }}").as_str())
        .err()
        .expect("unsupported");
    assert!(err.to_string().contains("vector"), "{err}");
    // Invalid definitions.
    let turso = docs();
    for bad in [
        VectorIndex::new("1docs", ex("embedding"), 3),
        VectorIndex::new("docs", ex("embedding"), 0),
        VectorIndex::new("docs", ex("embedding"), 3).metric(Metric::Jaccard),
    ] {
        assert!(turso.create_vector_index(&bad).is_err(), "{bad:?}");
    }
    turso.create_vector_index(&index()).unwrap();
    assert!(turso
        .create_vector_index(&VectorIndex::new("DOCS", ex("embedding"), 3))
        .unwrap_err()
        .to_string()
        .contains("already exists"));
}

// @lat: [[tests#Vector indexes#Class restriction and element types]]
#[test]
fn class_restriction_and_element_types() {
    let store = docs();
    store
        .create_vector_index(&index().class(ex("Doc")))
        .unwrap();
    let hits = store
        .vector_search("docs", &QueryVector::vector(&[1.0, 0.0, 0.0]), 5)
        .unwrap();
    assert_eq!(nodes(&hits), ["<http://example.com/b>"]);
    for (i, t) in [ElementType::Float64, ElementType::Int8, ElementType::Bit1]
        .into_iter()
        .enumerate()
    {
        let name = format!("q{i}");
        store
            .create_vector_index(&VectorIndex::new(&name, ex("embedding"), 3).element_type(t))
            .unwrap();
        let hits = store
            .vector_search(&name, &QueryVector::vector(&[1.0, 0.0, 0.0]), 1)
            .unwrap();
        assert_eq!(hits.len(), 1, "{t:?}");
    }
}

// @lat: [[tests#Vector indexes#Sparse Jaccard index]]
#[test]
fn sparse_jaccard_index() {
    let store = Store::new_turso().unwrap();
    store
        .update(format!(
            r#"{PREFIXES} INSERT DATA {{
                ex:a ex:bag "[1, 0, 0, 1, 0]" . ex:b ex:bag "[0, 1, 1, 0, 0]" . ex:c ex:bag "[1, 0, 0, 1, 1]" .
            }}"#
        ))
        .unwrap();
    store
        .create_vector_index(
            &VectorIndex::new("bags", ex("bag"), 5)
                .metric(Metric::Jaccard)
                .element_type(ElementType::SparseFloat32),
        )
        .unwrap();
    let hits = store
        .vector_search("bags", &QueryVector::vector(&[1.0, 0.0, 0.0, 1.0, 0.0]), 3)
        .unwrap();
    assert_eq!(hits[0].node, Term::from(ex("a")));
    assert!(hits.windows(2).all(|w| w[0].distance <= w[1].distance));
}

// @lat: [[tests#Vector indexes#SPARQL search joins the graph in one statement]]
#[test]
fn sparql_search_joins_the_graph() {
    let store = docs();
    store.create_vector_index(&index()).unwrap();
    let q = "SELECT ?doc ?title ?d ?s WHERE {
        SERVICE <oxilite:vector/docs> {
            [] oxl:query \"[1, 0.1, 0]\" ; oxl:k 3 ; oxl:node ?doc ; oxl:distance ?d ; oxl:score ?s .
        }
        ?doc ex:title ?title ; ex:year ?y FILTER(?y >= 2020)
    } ORDER BY ?d";
    let rows = select(&store, q);
    let titles: Vec<String> = rows
        .iter()
        .map(|r| match &r[1] {
            Some(Term::Literal(l)) => l.value().to_owned(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(titles, ["Alpha", "Gamma"]);
    let Some(Term::Literal(d)) = &rows[0][2] else {
        panic!()
    };
    assert_eq!(
        d.datatype().as_str(),
        "http://www.w3.org/2001/XMLSchema#double"
    );
    let explain = store.explain(format!("{PREFIXES}{q}").as_str()).unwrap();
    assert!(explain.contains("vector_distance_cos"), "{explain}");
    // A constant node: is ex:c among the two nearest?
    let rows = select(
        &store,
        "ASK { SERVICE <oxilite:vector/docs> { [] oxl:query ex:a ; oxl:k 2 ; oxl:node ex:b } }"
            .replace("ASK", "SELECT (1 AS ?one) WHERE")
            .as_str(),
    );
    assert_eq!(rows.len(), 1);
    // Errors name the index or the predicate.
    for (q, msg) in [
        ("SELECT ?n WHERE { SERVICE <oxilite:vector/nope> { [] oxl:query \"[1,0,0]\" ; oxl:node ?n } }", "nope"),
        ("SELECT ?n WHERE { SERVICE <oxilite:vector/docs> { [] oxl:query \"[1,0,0]\" ; ex:other ?n } }", "ex"),
        ("SELECT ?n WHERE { SERVICE <oxilite:vector/docs> { [] oxl:query \"[1,0]\" ; oxl:node ?n } }", "dimensions"),
    ] {
        let err = store
            .query(format!("{PREFIXES}{q}").as_str())
            .err()
            .expect("error");
        assert!(err.to_string().contains(msg), "{q}: {err}");
    }
}

fn cypher_options() -> oxilite::cypher::CypherOptions {
    oxilite::cypher::CypherOptions {
        vocabulary: oxilite::cypher::Vocabulary::new("http://example.com/"),
        ..Default::default()
    }
}

fn cypher(store: &Store<TursoBackend>, q: &str) -> oxilite::cypher::CypherResult {
    store
        .cypher_with(q, &Default::default(), &cypher_options())
        .unwrap_or_else(|e| panic!("{q}: {e}"))
}

// @lat: [[tests#Vector indexes#Cypher DDL]]
#[test]
fn cypher_ddl() {
    use oxilite::cypher::Value;
    let store = docs();
    cypher(
        &store,
        "CREATE VECTOR INDEX docs FOR (d:Doc) ON (d.embedding) \
         OPTIONS { indexConfig: { `vector.dimensions`: 3, `vector.similarity_function`: 'euclidean' } }",
    );
    let info = store.vector_indexes().unwrap();
    assert_eq!(info[0].index.property, ex("embedding"));
    assert_eq!(info[0].index.class, Some(ex("Doc")));
    assert_eq!(info[0].index.metric, Metric::Euclidean);
    // IF NOT EXISTS is a no-op; without it, a second creation fails.
    cypher(
        &store,
        "CREATE VECTOR INDEX docs IF NOT EXISTS FOR (d:Doc) ON (d.embedding) OPTIONS { indexConfig: { `vector.dimensions`: 3 } }",
    );
    assert!(store
        .cypher_with(
            "CREATE VECTOR INDEX docs FOR (d:Doc) ON (d.embedding) OPTIONS { indexConfig: { `vector.dimensions`: 3 } }",
            &Default::default(),
            &cypher_options()
        )
        .is_err());
    let r = cypher(&store, "SHOW VECTOR INDEXES");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("docs".into()));
    assert_eq!(r.rows[0][1], Value::String("Doc".into()));
    assert_eq!(r.rows[0][2], Value::String("embedding".into()));
    assert_eq!(r.rows[0][3], Value::Int(3));
    assert_eq!(r.rows[0][4], Value::String("euclidean".into()));
    assert_eq!(r.rows[0][7], Value::String("ONLINE".into()));
    cypher(&store, "DROP INDEX docs");
    assert!(store.vector_indexes().unwrap().is_empty());
    cypher(&store, "DROP INDEX docs IF EXISTS");
    assert!(store
        .cypher_with("DROP INDEX docs", &Default::default(), &cypher_options())
        .is_err());
}

// @lat: [[tests#Vector indexes#Cypher queryNodes binds a node]]
#[test]
fn cypher_query_nodes() {
    use oxilite::cypher::Value;
    let store = docs();
    store.create_vector_index(&index()).unwrap();
    store
        .update(format!(
            "{PREFIXES} INSERT DATA {{ ex:a ex:cites ex:c . ex:b ex:cites ex:c }}"
        ))
        .unwrap();
    let r = cypher(
        &store,
        "CALL db.index.vector.queryNodes('docs', 2, [1, 0.1, 0]) YIELD node, score \
         RETURN node.title AS title, score ORDER BY score DESC",
    );
    assert_eq!(
        r.rows.iter().map(|row| row[0].clone()).collect::<Vec<_>>(),
        [Value::String("Alpha".into()), Value::String("Beta".into())]
    );
    let (Value::Float(s0), Value::Float(s1)) = (&r.rows[0][1], &r.rows[1][1]) else {
        panic!("{:?}", r.rows)
    };
    assert!(s0 > s1);
    // `node` is a node: it matches further patterns and comes back with its properties.
    let mut params = oxilite::cypher::Params::new();
    params.insert(
        "q".into(),
        Value::List(vec![Value::Float(1.0), Value::Float(0.1), Value::Int(0)]),
    );
    let r = store
        .cypher_with(
            "CALL db.index.vector.queryNodes('docs', 3, $q) YIELD node, distance \
             MATCH (node)-[:cites]->(x) WHERE distance < 0.5 RETURN node, x.title AS cited",
            &params,
            &cypher_options(),
        )
        .unwrap();
    assert_eq!(r.rows.len(), 2, "{:?}", r.rows);
    let Value::Node(n) = &r.rows[0][0] else {
        panic!("{:?}", r.rows[0][0])
    };
    assert!(n.properties.contains_key("title"));
    assert_eq!(r.rows[0][1], Value::String("Gamma".into()));
    // A trailing CALL returns what it yields.
    let r = cypher(
        &store,
        "CALL db.index.vector.queryNodes('docs', 1, 'http://example.com/c') YIELD node",
    );
    assert_eq!(r.columns, ["node"]);
    assert_eq!(r.rows.len(), 1);
    // The search compiles into the statement's SQL.
    let explain = store
        .explain_cypher(
            "CALL db.index.vector.queryNodes('docs', 2, [1, 0, 0]) YIELD node RETURN node.title",
            &Default::default(),
            &cypher_options(),
        )
        .unwrap();
    assert!(explain.contains("vector_distance_cos"), "{explain}");
}

// @lat: [[tests#Vector indexes#Datalog nearest ranks neighbours]]
#[test]
fn datalog_nearest() {
    let store = docs();
    store.create_vector_index(&index()).unwrap();
    let run = |p: &str| {
        store
            .datalog(&format!("@prefix ex: <http://example.com/> .\n{p}"))
            .unwrap_or_else(|e| panic!("{p}: {e}"))
    };
    let r = run(
        r#"top(?d, ?r) :- nearest("docs", "[1, 0.1, 0]", 3, ?d, ?r), ?r <= 2. ?- top(?d, ?r)."#,
    );
    let mut rows: Vec<(String, String)> = (0..r.rows.len())
        .map(|i| {
            (
                r.get(i, "d").unwrap().to_string(),
                match r.get(i, "r").unwrap() {
                    Term::Literal(l) => l.value().to_owned(),
                    t => t.to_string(),
                },
            )
        })
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        [
            ("<http://example.com/a>".into(), "1".into()),
            ("<http://example.com/b>".into(), "2".into())
        ]
    );
    // By node, without rank, joined with the graph.
    let r = run(r#"t(?t) :- nearest("docs", ex:c, 1, ?d), ex:title(?d, ?t). ?- t(?t)."#);
    assert_eq!(r.rows.len(), 1);
    assert_eq!(
        r.get(0, "t"),
        Some(&Term::from(oxilite::model::Literal::new_simple_literal(
            "Gamma"
        )))
    );
    // Arguments that must be constants.
    let err = store
        .datalog(r#"@prefix ex: <http://example.com/> . p(?d) :- ex:title(?x, ?n), nearest(?n, "[1,0,0]", 2, ?d). ?- p(?d)."#)
        .unwrap_err();
    assert!(
        err.to_string().contains("index name must be a constant"),
        "{err}"
    );
    // A program defining `nearest` keeps its own relation.
    let r = run("nearest(?x) :- ex:title(?x, ?t). ?- nearest(?x).");
    assert_eq!(r.rows.len(), 3);
}
