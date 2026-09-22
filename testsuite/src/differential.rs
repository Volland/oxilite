//! Differential corpus: seeded datasets and query families run on Oxigraph and on every
//! oxilite variant; results must be identical (up to blank-node renaming).
//!
// @lat: [[test-plan#Oxigraph compatibility harness#Differential corpus]]

use crate::engine::{variants, OxiliteEngine};
use crate::sparql_evaluator::compare_query_results;
use anyhow::{bail, Context, Result};
use oxigraph::model::vocab::{rdf, rdfs, xsd};
use oxigraph::model::*;
use spargebra::SparqlParser;

/// Small deterministic PRNG (no dependency, stable across platforms).
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn ex(s: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("http://example.com/{s}"))
}

/// A dataset exercising every term kind, named graphs, duplicates across graphs,
/// class hierarchies, cycles and triple terms.
pub fn dataset(seed: u64, entities: usize) -> Dataset {
    let mut r = Lcg::new(seed);
    let mut d = Dataset::new();
    let dg = GraphName::DefaultGraph;
    let g1: GraphName = ex("g1").into();
    let g2: GraphName = ex("g2").into();
    let add = |d: &mut Dataset, s: NamedOrBlankNode, p: NamedNode, o: Term, g: &GraphName| {
        d.insert(&Quad::new(s, p, o, g.clone()));
    };
    for k in 0..5 {
        add(
            &mut d,
            ex(&format!("C{k}")).into(),
            rdfs::SUB_CLASS_OF.into_owned(),
            ex(&format!("C{}", k + 1)).into(),
            &dg,
        );
    }
    for i in 0..entities {
        let e: NamedOrBlankNode = ex(&format!("e{i}")).into();
        add(
            &mut d,
            e.clone(),
            rdf::TYPE.into_owned(),
            ex(&format!("C{}", i % 5)).into(),
            &dg,
        );
        add(
            &mut d,
            e.clone(),
            ex("age"),
            Literal::from((r.below(90)) as i64).into(),
            &dg,
        );
        add(
            &mut d,
            e.clone(),
            ex("name"),
            Literal::new_simple_literal(format!("name {}", r.below(20))).into(),
            &dg,
        );
        if i % 2 == 0 {
            add(
                &mut d,
                e.clone(),
                ex("label"),
                Literal::new_language_tagged_literal_unchecked(
                    format!("label {i}"),
                    if i % 4 == 0 { "en" } else { "fr" },
                )
                .into(),
                &dg,
            );
        }
        if i % 3 == 0 {
            add(
                &mut d,
                e.clone(),
                ex("score"),
                Literal::new_typed_literal(
                    format!("{}.{}", r.below(10), r.below(100)),
                    xsd::DECIMAL,
                )
                .into(),
                &dg,
            );
            add(
                &mut d,
                e.clone(),
                ex("weight"),
                Literal::new_typed_literal(
                    format!("{}e{}", r.below(9) + 1, r.below(3)),
                    xsd::DOUBLE,
                )
                .into(),
                &dg,
            );
        }
        if i % 4 == 1 {
            let flag = ["true", "false", "1", "0"][r.below(4) as usize];
            add(
                &mut d,
                e.clone(),
                ex("flag"),
                Literal::new_typed_literal(flag, xsd::BOOLEAN).into(),
                &dg,
            );
        }
        if i % 5 == 2 {
            let tz = ["", "Z", "+02:00"][r.below(3) as usize];
            add(
                &mut d,
                e.clone(),
                ex("born"),
                Literal::new_typed_literal(
                    format!(
                        "19{:02}-0{}-1{}{tz}",
                        r.below(99),
                        r.below(9) + 1,
                        r.below(9)
                    ),
                    xsd::DATE,
                )
                .into(),
                &dg,
            );
            add(
                &mut d,
                e.clone(),
                ex("seen"),
                Literal::new_typed_literal(
                    format!(
                        "2020-0{}-1{}T1{}:00:00{tz}",
                        r.below(9) + 1,
                        r.below(9),
                        r.below(9)
                    ),
                    xsd::DATE_TIME,
                )
                .into(),
                &dg,
            );
        }
        if i % 7 == 3 {
            // Non-canonical integer: a distinct term from "7".
            add(
                &mut d,
                e.clone(),
                ex("code"),
                Literal::new_typed_literal(format!("00{}", r.below(9)), xsd::INTEGER).into(),
                &dg,
            );
        }
        for _ in 0..2 {
            add(
                &mut d,
                e.clone(),
                ex("knows"),
                ex(&format!("e{}", r.below(entities as u64))).into(),
                &dg,
            );
        }
        add(
            &mut d,
            e.clone(),
            ex("next"),
            ex(&format!("e{}", (i + 1) % entities)).into(),
            &dg,
        );
        if i % 6 == 0 {
            let b = BlankNode::new_unchecked(format!("addr{i}"));
            add(&mut d, e.clone(), ex("addr"), b.clone().into(), &dg);
            add(
                &mut d,
                b.into(),
                ex("city"),
                ex(&format!("city{}", i % 3)).into(),
                &dg,
            );
        }
        // Named graphs, with duplicates across graphs and with the default graph.
        if i % 3 == 0 {
            add(
                &mut d,
                e.clone(),
                ex("inG"),
                Literal::from(i as i64).into(),
                &g1,
            );
        }
        if i % 4 == 0 {
            add(
                &mut d,
                e.clone(),
                ex("inG"),
                Literal::from(i as i64).into(),
                &g2,
            );
            add(
                &mut d,
                e.clone(),
                ex("knows"),
                ex(&format!("e{}", (i + 3) % entities)).into(),
                &g2,
            );
        }
        if i % 8 == 0 {
            add(
                &mut d,
                e.clone(),
                ex("age"),
                Literal::from(99_i64).into(),
                &g1,
            );
        }
    }
    // RDF 1.2 triple terms.
    for i in 0..3 {
        let t = Triple::new(
            ex(&format!("e{i}")),
            ex("knows"),
            ex(&format!("e{}", i + 1)),
        );
        add(
            &mut d,
            ex(&format!("e{i}")).into(),
            ex("says"),
            t.into(),
            &dg,
        );
    }
    d
}

/// A query of the corpus.
pub struct CorpusQuery {
    pub id: String,
    pub query: String,
    /// Compare solution order too (ORDER BY on a unique key).
    pub ordered: bool,
}

const P: &str = "PREFIX ex: <http://example.com/> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> ";

fn q(id: &str, body: &str, ordered: bool) -> CorpusQuery {
    CorpusQuery {
        id: format!("corpus:{id}"),
        query: format!("{P}{body}"),
        ordered,
    }
}

/// Query families for M1 (BGP shapes × filters × datasets × modifiers) and M2 (OPTIONAL,
/// UNION, MINUS, EXISTS, VALUES, BIND, aggregates, paths, subqueries).
pub fn queries() -> Vec<CorpusQuery> {
    let mut v = vec![
        // BGP shapes
        q("star", "SELECT * WHERE { ?s a ex:C1 ; ex:age ?a ; ex:name ?n }", false),
        q("chain", "SELECT * WHERE { ?a ex:knows ?b . ?b ex:knows ?c }", false),
        q("cycle", "SELECT * WHERE { ?a ex:knows ?b . ?b ex:knows ?c . ?c ex:knows ?a }", false),
        q("cartesian", "SELECT * WHERE { ?a a ex:C0 . ?b a ex:C4 } LIMIT 1000", false),
        q("unbound-predicate", "SELECT ?p (COUNT(*) AS ?c) WHERE { ?s ?p ?o } GROUP BY ?p", false),
        q("constant-subject", "SELECT ?p ?o WHERE { ex:e1 ?p ?o }", false),
        q("same-var-twice", "SELECT ?s WHERE { ?s ex:knows ?s }", false),
        q("bnode-object", "SELECT ?s ?c WHERE { ?s ex:addr [ ex:city ?c ] }", false),
        q("triple-term", "SELECT * WHERE { ?s ex:says <<( ?a ex:knows ?b )>> }", false),
        q("triple-term-var", "SELECT ?t WHERE { ?s ex:says ?t }", false),
        // Datasets
        q("graph-var", "SELECT * WHERE { GRAPH ?g { ?s ex:inG ?v } }", false),
        q("graph-const", "SELECT * WHERE { GRAPH ex:g2 { ?s ?p ?o } }", false),
        q("graph-missing", "SELECT * WHERE { GRAPH ex:nope { ?s ?p ?o } }", false),
        q("from", "SELECT * FROM ex:g1 WHERE { ?s ?p ?o }", false),
        q("from-two", "SELECT * FROM ex:g1 FROM ex:g2 WHERE { ?s ex:inG ?o }", false),
        q("from-named", "SELECT * FROM NAMED ex:g1 WHERE { GRAPH ?g { ?s ex:inG ?o } }", false),
        q("graph-join", "SELECT * WHERE { ?s a ex:C0 . GRAPH ?g { ?s ex:inG ?v } }", false),
        q("graph-empty", "SELECT ?g WHERE { GRAPH ?g { } }", false),
        // Modifiers
        q("distinct", "SELECT DISTINCT ?n WHERE { ?s ex:name ?n }", false),
        q("order-limit", "SELECT ?s WHERE { ?s a ex:C2 } ORDER BY ?s LIMIT 5", true),
        q("order-desc-offset", "SELECT ?s WHERE { ?s a ex:C3 } ORDER BY DESC(?s) OFFSET 2 LIMIT 4", true),
        q("order-by-value", "SELECT ?s ?a WHERE { ?s ex:age ?a } ORDER BY ?a ?s", true),
        q("order-mixed", "SELECT ?o WHERE { ex:e0 ?p ?o } ORDER BY ?o", false),
        q("reduced", "SELECT REDUCED ?n WHERE { ?s ex:name ?n }", false),
        q("ask-true", "ASK { ?s ex:knows ?o }", false),
        q("ask-false", "ASK { ?s ex:nope ?o }", false),
        q("construct", "CONSTRUCT { ?s ex:friend ?o . ?o ex:linked [ ex:via ?s ] } WHERE { ?s ex:knows ?o } ORDER BY ?s ?o LIMIT 20", false),
        q("describe", "DESCRIBE ex:e6", false),
        // OPTIONAL / UNION / MINUS / EXISTS
        q("optional", "SELECT * WHERE { ?s a ex:C0 OPTIONAL { ?s ex:label ?l } }", false),
        q("optional-filter", "SELECT * WHERE { ?s a ex:C1 OPTIONAL { ?s ex:age ?a FILTER(?a > 50) } }", false),
        q("optional-nested", "SELECT * WHERE { ?s a ex:C2 OPTIONAL { ?s ex:addr ?b OPTIONAL { ?b ex:city ?c } } }", false),
        q("optional-bind", "SELECT * WHERE { ?s a ex:C3 OPTIONAL { ?s ex:score ?x BIND(1 AS ?one) } }", false),
        q("optional-shared-null", "SELECT * WHERE { ?s a ex:C0 OPTIONAL { ?s ex:label ?l } OPTIONAL { ?s ex:score ?l } }", false),
        q("union", "SELECT * WHERE { { ?s ex:label ?x } UNION { ?s ex:score ?x } }", false),
        q("union-vars", "SELECT * WHERE { { ?s ex:label ?l } UNION { ?s ex:age ?a FILTER(?a < 10) } }", false),
        q("minus", "SELECT ?s WHERE { ?s a ex:C0 MINUS { ?s ex:label ?l } }", false),
        q("minus-disjoint", "SELECT ?s WHERE { ?s a ex:C0 MINUS { ?x ex:label ?l } }", false),
        q("exists", "SELECT ?s WHERE { ?s a ex:C1 FILTER EXISTS { ?s ex:knows ?o . ?o a ex:C2 } }", false),
        q("not-exists", "SELECT ?s WHERE { ?s a ex:C1 FILTER NOT EXISTS { ?s ex:flag ?f } }", false),
        q("values", "SELECT * WHERE { VALUES ?s { ex:e1 ex:e2 ex:missing } ?s ex:age ?a }", false),
        q("values-undef", "SELECT * WHERE { VALUES (?s ?a) { (ex:e1 UNDEF) (UNDEF 42) } ?s ex:age ?a }", false),
        q("bind", "SELECT ?s ?b WHERE { ?s ex:age ?a BIND(?a * 2 + 1 AS ?b) }", false),
        q("bind-string", "SELECT ?s ?u WHERE { ?s ex:name ?n BIND(UCASE(?n) AS ?u) }", false),
        q("subquery", "SELECT * WHERE { { SELECT ?s WHERE { ?s a ex:C1 } ORDER BY ?s LIMIT 3 } ?s ex:age ?a }", false),
        // Aggregates
        q("count", "SELECT (COUNT(*) AS ?c) WHERE { ?s ex:knows ?o }", false),
        q("count-empty", "SELECT (COUNT(*) AS ?c) WHERE { ?s ex:nope ?o }", false),
        q("count-distinct", "SELECT (COUNT(DISTINCT ?o) AS ?c) WHERE { ?s ex:knows ?o }", false),
        q("group-count", "SELECT ?t (COUNT(?s) AS ?c) WHERE { ?s a ?t } GROUP BY ?t", false),
        q("sum-avg", "SELECT ?t (SUM(?a) AS ?sum) (AVG(?a) AS ?avg) WHERE { ?s a ?t ; ex:age ?a } GROUP BY ?t", false),
        q("sum-decimal", "SELECT (SUM(?x) AS ?sum) WHERE { ?s ex:score ?x }", false),
        q("min-max", "SELECT ?t (MAX(?a) AS ?m) WHERE { ?s a ?t ; ex:age ?a } GROUP BY ?t", false),
        q("min-date", "SELECT (MIN(?d) AS ?m) WHERE { ?s ex:seen ?d }", false),
        q("having", "SELECT ?t (COUNT(*) AS ?c) WHERE { ?s a ?t } GROUP BY ?t HAVING (COUNT(*) > 3)", false),
        q("group-concat-single", "SELECT ?s (GROUP_CONCAT(?n) AS ?all) WHERE { ?s ex:name ?n } GROUP BY ?s", false),
        q("count-star-distinct", "SELECT (COUNT(DISTINCT *) AS ?c) WHERE { ?s ex:knows ?o }", false),
        // Property paths
        q("path-plus-const", "SELECT ?o WHERE { ex:e0 ex:next+ ?o }", false),
        q("path-star-const", "SELECT ?o WHERE { ex:e0 ex:knows* ?o }", false),
        q("path-star-object", "SELECT ?s WHERE { ?s ex:next* ex:e3 }", false),
        q("path-plus-unbound", "SELECT (COUNT(*) AS ?c) WHERE { ?a ex:next+ ?b }", false),
        q("path-seeded", "SELECT ?c ?x WHERE { ?c a ex:C4 . ?c ex:knows+ ?x }", false),
        q("path-seq", "SELECT * WHERE { ?a ex:knows/ex:knows ?c }", false),
        q("path-alt", "SELECT * WHERE { ex:e1 (ex:knows|ex:next) ?x }", false),
        q("path-inverse", "SELECT * WHERE { ex:e1 ^ex:knows ?x }", false),
        q("path-negated", "SELECT * WHERE { ex:e1 !(ex:knows|ex:next) ?x }", false),
        q("path-optional", "SELECT * WHERE { ex:e1 ex:next? ?x }", false),
        q("path-subclass", "SELECT ?c WHERE { ex:C0 rdfs:subClassOf* ?c }", false),
        q("path-in-graph", "SELECT * WHERE { GRAPH ?g { ex:e0 ex:knows+ ?x } }", false),
        q("path-same-var", "SELECT ?s WHERE { ?s ex:next+ ?s }", false),
    ];
    // Filter families over every literal kind.
    for (i, f) in [
        "?a > 30",
        "?a = 42",
        "?a >= 10 && ?a < 20",
        "?a != 5",
        "?a IN (1, 2, 3, 42)",
        "!(?a < 50)",
        "?a > 30 || ?n = \"name 3\"",
        "CONTAINS(?n, \"1\")",
        "STRSTARTS(?n, \"name 1\")",
        "REGEX(?n, \"^name 1\")",
        "REGEX(?n, \"1$\")",
        "STRLEN(?n) > 6",
        "?n < \"name 5\"",
        "ISLITERAL(?a) && ISNUMERIC(?a)",
        "DATATYPE(?a) = xsd:integer",
        "?a + 1 > 50",
        "?a / 2 > 20",
        "ABS(?a - 45) < 5",
    ]
    .iter()
    .enumerate()
    {
        v.push(q(
            &format!("filter-age-{i}"),
            &format!("SELECT ?s ?a WHERE {{ ?s ex:age ?a ; ex:name ?n FILTER({f}) }}"),
            false,
        ));
    }
    for (i, f) in [
        "?x > 5",
        "?x < 2.5",
        "?x = 1.5",
        "DATATYPE(?x) = xsd:decimal",
        "?x > 10",
        "xsd:integer(?x) > 1",
        "STR(?x) != \"\"",
    ]
    .iter()
    .enumerate()
    {
        v.push(q(&format!("filter-num-{i}"), &format!("SELECT ?s ?x WHERE {{ {{ ?s ex:score ?x }} UNION {{ ?s ex:weight ?x }} FILTER({f}) }}"), false));
    }
    for (i, f) in [
        "?d > \"1950-01-01\"^^xsd:date",
        "?d < \"1950-01-01Z\"^^xsd:date",
        "YEAR(?d) > 1950",
        "MONTH(?d) = 3",
    ]
    .iter()
    .enumerate()
    {
        v.push(q(
            &format!("filter-date-{i}"),
            &format!("SELECT ?s ?d WHERE {{ ?s ex:born ?d FILTER({f}) }}"),
            false,
        ));
    }
    for (i, f) in [
        "?t > \"2020-05-01T00:00:00Z\"^^xsd:dateTime",
        "HOURS(?t) >= 15",
        "TZ(?t) = \"Z\"",
    ]
    .iter()
    .enumerate()
    {
        v.push(q(
            &format!("filter-datetime-{i}"),
            &format!("SELECT ?s ?t WHERE {{ ?s ex:seen ?t FILTER({f}) }}"),
            false,
        ));
    }
    for (i, f) in [
        "?f",
        "!?f",
        "?f = true",
        "xsd:boolean(?f)",
        "STR(?f) = \"1\"",
    ]
    .iter()
    .enumerate()
    {
        v.push(q(
            &format!("filter-bool-{i}"),
            &format!("SELECT ?s ?f WHERE {{ ?s ex:flag ?f FILTER({f}) }}"),
            false,
        ));
    }
    for (i, f) in [
        "LANG(?l) = \"en\"",
        "LANGMATCHES(LANG(?l), \"fr\")",
        "?l = \"label 0\"@en",
        "STR(?l) = \"label 2\"",
        "CONTAINS(?l, \"4\")",
        "UCASE(?l) = \"LABEL 4\"@en",
    ]
    .iter()
    .enumerate()
    {
        v.push(q(
            &format!("filter-lang-{i}"),
            &format!("SELECT ?s ?l WHERE {{ ?s ex:label ?l FILTER({f}) }}"),
            false,
        ));
    }
    for (i, f) in [
        "?c = 7",
        "?c = \"007\"^^xsd:integer",
        "STR(?c) = \"003\"",
        "?c > 4",
    ]
    .iter()
    .enumerate()
    {
        v.push(q(
            &format!("filter-code-{i}"),
            &format!("SELECT ?s ?c WHERE {{ ?s ex:code ?c FILTER({f}) }}"),
            false,
        ));
    }
    for (i, f) in [
        "ISBLANK(?o)",
        "ISIRI(?o)",
        "BOUND(?l)",
        "!BOUND(?l)",
        "COALESCE(?l, \"none\") = \"none\"",
    ]
    .iter()
    .enumerate()
    {
        v.push(q(
            &format!("filter-kind-{i}"),
            &format!("SELECT ?s ?o WHERE {{ ?s ex:addr|ex:knows ?o OPTIONAL {{ ?s ex:label ?l }} FILTER({f}) }}"),
            false,
        ));
    }
    v
}

/// Runs one corpus query on Oxigraph and on every oxilite variant.
pub fn check(dataset: &Dataset, cq: &CorpusQuery) -> Result<()> {
    let query = SparqlParser::new()
        .parse_query(&cq.query)
        .with_context(|| format!("{}: bad corpus query", cq.id))?;
    let reference = oxigraph::store::Store::new()?;
    reference.extend(dataset.iter().map(QuadRef::into_owned))?;
    #[expect(deprecated)]
    let expected = || reference.query(cq.query.as_str());
    let mut errors = Vec::new();
    for variant in variants() {
        let engine = OxiliteEngine::new(variant, dataset)?;
        let actual = match engine.query(&query) {
            Ok(a) => a,
            Err(e) => {
                errors.push(format!(
                    "oxilite[{variant}] failed: {e}\n{}",
                    engine.explain(&query)
                ));
                continue;
            }
        };
        if let Err(e) = compare_query_results(expected()?, actual, cq.ordered) {
            errors.push(format!(
                "oxilite[{variant}] differs from Oxigraph:\n{e}\n{}",
                engine.explain(&query)
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("{} ({}):\n{}", cq.id, cq.query, errors.join("\n"))
    }
}

/// Dataset of Oxigraph's `optimizer_regression.rs` (OPTIONAL on a foreign key): persons with
/// orders pointing back to them.
pub fn orders_dataset(persons: usize, orders_per_person: usize) -> Dataset {
    let mut d = Dataset::new();
    let s = |x: &str| NamedNode::new_unchecked(format!("http://schema.org/{x}"));
    let e = |x: String| NamedNode::new_unchecked(format!("http://example.org/{x}"));
    let g = GraphName::DefaultGraph;
    for p in 0..persons {
        let person = e(format!("person/{p}"));
        d.insert(&Quad::new(
            person.clone(),
            rdf::TYPE,
            s("Person"),
            g.clone(),
        ));
        d.insert(&Quad::new(
            person.clone(),
            s("addressCountry"),
            Literal::new_simple_literal("FR"),
            g.clone(),
        ));
        d.insert(&Quad::new(
            person.clone(),
            e("segment".into()),
            Literal::new_simple_literal("retail"),
            g.clone(),
        ));
        for o in 0..orders_per_person {
            let order = e(format!("order/{p}/{o}"));
            d.insert(&Quad::new(order.clone(), rdf::TYPE, s("Order"), g.clone()));
            d.insert(&Quad::new(
                order.clone(),
                s("customer"),
                person.clone(),
                g.clone(),
            ));
            d.insert(&Quad::new(
                order,
                s("totalPrice"),
                Literal::from((p * 100 + o) as i64),
                g.clone(),
            ));
        }
    }
    d
}

/// The query of Oxigraph's `optimizer_regression.rs`.
pub const ORDERS_QUERY: &str = "
PREFIX schema: <http://schema.org/>
PREFIX ex: <http://example.org/>
SELECT * WHERE {
  ?c a schema:Person ;
     schema:addressCountry ?country ;
     ex:segment ?segment .
  OPTIONAL {
    ?order a schema:Order ;
           schema:customer ?c ;
           schema:totalPrice ?total .
  }
}
";
