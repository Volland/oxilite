//! Differential corpus: the same questions asked through SPARQL and through Cypher over one
//! dataset must give the same answers (multisets of rows, or sequences when ordered). Cypher
//! runs on the native store and on the D1 code path, SPARQL natively.

use futures::executor::block_on;
use oxilite::cypher::{CypherOptions, Params, Value, Vocabulary};
use oxilite::rusqlite::RusqliteBackend;
use oxilite::sparql::{QueryResults, Reasoning};
use oxilite::store::Store;
use oxilite::{AsyncBackend, AsyncStore, Capabilities, QueryOptions};
use oxilite_core::{Request, Response, SyncBackend};
use oxrdfio::RdfFormat;

struct D1Like(RusqliteBackend, Capabilities);

impl AsyncBackend for D1Like {
    async fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        self.0.execute(request)
    }
    fn capabilities(&self) -> &Capabilities {
        &self.1
    }
}

const DATA: &str = r#"
@prefix ex: <http://example.com/> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:Employee rdfs:subClassOf ex:Person .
ex:hasManager owl:inverseOf ex:manages .

ex:ada a ex:Person ; ex:name "Ada" ; ex:age 36 ; ex:tag "math", "code" ; ex:born "1815-12-10"^^xsd:date .
ex:alan a ex:Person ; ex:name "Alan" ; ex:age 41 ; ex:tag "code" ; ex:born "1912-06-23"^^xsd:date .
ex:grace a ex:Employee ; ex:name "Grace" ; ex:age 85 ; ex:born "1906-12-09"^^xsd:date .
ex:linus a ex:Person ; ex:name "Linus" .
ex:acme a ex:Company ; ex:name "Acme" .

ex:ada ex:knows ex:alan , ex:grace .
ex:alan ex:knows ex:grace .
ex:grace ex:knows ex:ada .
ex:linus ex:knows ex:ada .
ex:grace ex:worksFor ex:acme .
ex:ada ex:manages ex:alan .

ex:ada ex:rated ex:acme .
ex:r1 rdf:reifies <<( ex:ada ex:rated ex:acme )>> ; ex:stars 4 .
ex:r2 rdf:reifies <<( ex:ada ex:rated ex:acme )>> ; ex:stars 2 .
ex:k1 rdf:reifies <<( ex:ada ex:knows ex:alan )>> ; ex:since 1840 .
"#;

/// (question, SPARQL, Cypher, ordered, reasoning)
const CASES: &[(&str, &str, &str, bool, Reasoning)] = &[
    ("labels", "SELECT ?n WHERE { ?p a ex:Person ; ex:name ?n }", "MATCH (p:Person) RETURN p.name", false, Reasoning::None),
    ("numeric filter", "SELECT ?n WHERE { ?p ex:name ?n ; ex:age ?a FILTER(?a > 40) }", "MATCH (p) WHERE p.age > 40 RETURN p.name", false, Reasoning::None),
    ("two hops", "SELECT ?x ?z WHERE { ?a ex:knows ?b . ?b ex:knows ?c . ?a ex:name ?x . ?c ex:name ?z FILTER(?a != ?c) }", "MATCH (a)-[:knows]->(b)-[:knows]->(c) WHERE a <> c RETURN a.name, c.name", false, Reasoning::None),
    ("optional", "SELECT ?n ?c WHERE { ?p a ex:Person ; ex:name ?n OPTIONAL { ?p ex:worksFor ?co . ?co ex:name ?c } }", "MATCH (p:Person) OPTIONAL MATCH (p)-[:worksFor]->(co) RETURN p.name, co.name", false, Reasoning::None),
    ("group count", "SELECT ?n (COUNT(?f) AS ?c) WHERE { ?p ex:name ?n ; ex:knows ?f } GROUP BY ?n", "MATCH (p)-[:knows]->(f) RETURN p.name, count(f)", false, Reasoning::None),
    ("order and limit", "SELECT ?n WHERE { ?p ex:name ?n ; ex:age ?a } ORDER BY DESC(?a) LIMIT 2", "MATCH (p) WHERE p.age IS NOT NULL RETURN p.name ORDER BY p.age DESC LIMIT 2", true, Reasoning::None),
    ("distinct", "SELECT DISTINCT ?n WHERE { ?p ex:knows ?f . ?f ex:name ?n }", "MATCH (p)-[:knows]->(f) RETURN DISTINCT f.name", false, Reasoning::None),
    ("relationship property", "SELECT ?s WHERE { ?r rdf:reifies <<( ex:ada ex:knows ex:alan )>> ; ex:since ?s }", "MATCH ({name: 'Ada'})-[k:knows]->({name: 'Alan'}) WHERE k.since IS NOT NULL RETURN k.since", false, Reasoning::None),
    ("parallel relationships", "SELECT ?stars WHERE { ex:ada ex:rated ex:acme . ?r rdf:reifies <<( ex:ada ex:rated ex:acme )>> ; ex:stars ?stars }", "MATCH ({name: 'Ada'})-[r:rated]->({name: 'Acme'}) RETURN r.stars", false, Reasoning::None),
    ("reachability", "SELECT DISTINCT ?n WHERE { ex:linus ex:knows+ ?x . ?x ex:name ?n }", "MATCH ({name: 'Linus'})-[:knows*]->(x) RETURN DISTINCT x.name", false, Reasoning::None),
    ("undirected", "SELECT ?n WHERE { { ex:grace ex:knows ?x } UNION { ?x ex:knows ex:grace } ?x ex:name ?n }", "MATCH ({name: 'Grace'})-[:knows]-(x) RETURN x.name", false, Reasoning::None),
    ("not exists", "SELECT ?n WHERE { ?p a ex:Person ; ex:name ?n FILTER NOT EXISTS { ?x ex:knows ?p } }", "MATCH (p:Person) WHERE NOT ()-[:knows]->(p) RETURN p.name", false, Reasoning::None),
    ("string predicate", "SELECT ?n WHERE { ?p ex:name ?n FILTER(STRSTARTS(?n, \"A\")) }", "MATCH (p) WHERE p.name STARTS WITH 'A' RETURN p.name", false, Reasoning::None),
    ("in list", "SELECT ?a WHERE { ?p ex:name ?n ; ex:age ?a FILTER(?n IN (\"Ada\", \"Grace\")) }", "MATCH (p) WHERE p.name IN ['Ada', 'Grace'] RETURN p.age", false, Reasoning::None),
    ("aggregates", "SELECT (MIN(?a) AS ?mi) (MAX(?a) AS ?ma) (SUM(?a) AS ?s) WHERE { ?p ex:age ?a }", "MATCH (p) WHERE p.age IS NOT NULL RETURN min(p.age), max(p.age), sum(p.age)", false, Reasoning::None),
    ("multi-valued property", "SELECT ?t WHERE { ex:ada ex:tag ?t }", "MATCH (p {name: 'Ada'}) UNWIND p.tag AS t RETURN t", false, Reasoning::None),
    ("dates", "SELECT ?n WHERE { ?p ex:name ?n ; ex:born ?b FILTER(?b > \"1900-01-01\"^^xsd:date) }", "MATCH (p) WHERE p.born > date('1900-01-01') RETURN p.name", false, Reasoning::None),
    ("conditional", "SELECT ?n (IF(?a > 40, \"old\", \"young\") AS ?g) WHERE { ?p ex:name ?n ; ex:age ?a }", "MATCH (p) WHERE p.age IS NOT NULL RETURN p.name, CASE WHEN p.age > 40 THEN 'old' ELSE 'young' END", false, Reasoning::None),
    ("union", "SELECT ?n WHERE { { ?p a ex:Company } UNION { ?p a ex:Employee } ?p ex:name ?n }", "MATCH (p:Company) RETURN p.name AS n UNION ALL MATCH (p:Employee) RETURN p.name AS n", false, Reasoning::None),
    ("subclass reasoning", "SELECT ?n WHERE { ?p a ex:Person ; ex:name ?n }", "MATCH (p:Person) RETURN p.name", false, Reasoning::Rdfs),
    ("inverse reasoning", "SELECT ?m ?e WHERE { ?x ex:hasManager ?y . ?x ex:name ?e . ?y ex:name ?m }", "MATCH (x)-[:hasManager]->(y) RETURN y.name, x.name", false, Reasoning::OwlQl),
];

fn norm_term(t: &oxrdf::Term) -> String {
    Value::from_term(t).to_string()
}

fn sparql_rows(r: QueryResults<'static>) -> Vec<Vec<String>> {
    let QueryResults::Solutions(sol) = r else {
        panic!("not SELECT")
    };
    let vars: Vec<oxrdf::Variable> = sol.variables().to_vec();
    sol.map(|s| {
        let s = s.unwrap();
        vars.iter()
            .map(|v| s.get(v).map_or("null".into(), norm_term))
            .collect()
    })
    .collect()
}

fn cypher_rows(r: oxilite::cypher::CypherResult) -> Vec<Vec<String>> {
    r.rows
        .into_iter()
        .map(|row| row.iter().map(ToString::to_string).collect())
        .collect()
}

fn opts(reasoning: Reasoning) -> (QueryOptions, CypherOptions) {
    let q = QueryOptions {
        reasoning,
        ..Default::default()
    };
    let c = CypherOptions {
        vocabulary: Vocabulary::new("http://example.com/"),
        query: q.clone(),
        ..Default::default()
    };
    (q, c)
}

fn check(
    name: &str,
    engine: &str,
    sparql: Vec<Vec<String>>,
    cypher: Vec<Vec<String>>,
    ordered: bool,
) {
    let (mut a, mut b) = (sparql, cypher);
    if !ordered {
        a.sort();
        b.sort();
    }
    assert_eq!(
        a, b,
        "[{engine}] {name}: SPARQL (left) and Cypher (right) disagree"
    );
}

// @lat: [[tests#Cypher#SPARQL and Cypher agree]]
#[test]
fn sparql_and_cypher_agree() {
    let prefixes = "PREFIX ex: <http://example.com/> PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> ";
    let native = Store::new().unwrap();
    native
        .load_from_slice(RdfFormat::Turtle, DATA.as_bytes())
        .unwrap();
    let d1 = block_on(AsyncStore::open(D1Like(
        RusqliteBackend::memory().unwrap(),
        Capabilities::d1(),
    )))
    .unwrap();
    block_on(d1.load_from_slice(RdfFormat::Turtle, DATA.as_bytes())).unwrap();
    for (name, sparql, cypher, ordered, reasoning) in CASES {
        let q = format!("{prefixes}{sparql}");
        let (qo, co) = opts(*reasoning);
        let s = sparql_rows(native.query_opt(q.as_str(), qo.clone()).unwrap());
        assert!(!s.is_empty(), "{name}: the SPARQL side has no rows");
        let c = cypher_rows(
            native
                .cypher_with(cypher, &Params::new(), &co)
                .unwrap_or_else(|e| panic!("{name}: {e}")),
        );
        check(name, "native", s.clone(), c, *ordered);
        // On D1, Cypher is checked against the native SPARQL answer (some of these SPARQL
        // queries need the fallback, which D1 does not have; Cypher runs them in Rust).
        let _ = qo;
        let c2 = cypher_rows(
            block_on(d1.cypher_with(cypher, &Params::new(), &co))
                .unwrap_or_else(|e| panic!("[d1] {name}: {e}")),
        );
        check(name, "d1", s, c2, *ordered);
    }
}
