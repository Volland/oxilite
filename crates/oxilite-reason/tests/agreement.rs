//! The SQL OWL 2 RL rules (every backend) agree with the `reasonable` reasoner.

use oxilite::io::RdfFormat;
use oxilite::sparql::QueryOptions;
use oxilite::store::Store;
use oxilite_core::QueryOutput;
use std::collections::BTreeSet;

const PREFIXES: &str = "@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
    @prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .";

/// Sample ontologies, each exercising a group of OWL 2 RL rules.
const SAMPLES: &[(&str, &str)] = &[
    (
        "rdfs",
        "ex:Dog rdfs:subClassOf ex:Mammal . ex:Mammal rdfs:subClassOf ex:Animal . ex:Cat rdfs:subClassOf ex:Mammal .
         ex:hasMother rdfs:subPropertyOf ex:hasParent . ex:hasParent rdfs:subPropertyOf ex:hasAncestor .
         ex:hasParent rdfs:domain ex:Animal ; rdfs:range ex:Animal . ex:hasAncestor rdfs:range ex:Animal .
         ex:rex a ex:Dog ; ex:hasMother ex:lady . ex:tom a ex:Cat ; ex:hasParent ex:felix .",
    ),
    (
        "properties",
        "ex:hasChild owl:inverseOf ex:hasParent . ex:knows a owl:SymmetricProperty . ex:ancestor a owl:TransitiveProperty .
         ex:hasParent rdfs:subPropertyOf ex:ancestor . ex:spouse a owl:SymmetricProperty .
         ex:hasBirthMother a owl:FunctionalProperty . ex:ssn a owl:InverseFunctionalProperty .
         ex:parentOf owl:equivalentProperty ex:hasChild .
         ex:carol ex:hasChild ex:alice . ex:alice ex:hasParent ex:bob . ex:bob ex:hasParent ex:dan .
         ex:alice ex:knows ex:eve . ex:eve ex:spouse ex:frank .
         ex:zoe ex:hasBirthMother ex:m1 , ex:m2 . ex:p1 ex:ssn \"123\" . ex:p2 ex:ssn \"123\" .",
    ),
    (
        "equality",
        "ex:rex owl:sameAs ex:rexy . ex:rexy owl:sameAs ex:rx . ex:rex ex:name \"Rex\" ; a ex:Dog .
         ex:owner ex:owns ex:rexy . ex:likes owl:sameAs ex:enjoys . ex:rx ex:likes ex:bone .",
    ),
    (
        "classes",
        "ex:Parent owl:intersectionOf (ex:Person ex:HasChild) . ex:ann a ex:Person , ex:HasChild .
         ex:Pet owl:unionOf (ex:Dog ex:Cat) . ex:tom a ex:Cat .
         ex:DogOwner owl:onProperty ex:owns ; owl:someValuesFrom ex:Dog . ex:bob ex:owns ex:rex . ex:rex a ex:Dog .
         ex:Red owl:onProperty ex:color ; owl:hasValue ex:red . ex:car a ex:Red . ex:apple ex:color ex:red .
         ex:Vegan owl:onProperty ex:eats ; owl:allValuesFrom ex:Plant . ex:vic a ex:Vegan ; ex:eats ex:kale .
         ex:Human owl:equivalentClass ex:Person . ex:hal a ex:Human .
         ex:uncle owl:propertyChainAxiom (ex:parent ex:brother) . ex:kim ex:parent ex:lee . ex:lee ex:brother ex:max .",
    ),
    (
        "schema",
        "ex:A rdfs:subClassOf ex:B . ex:B rdfs:subClassOf ex:C . ex:C rdfs:subClassOf ex:A .
         ex:p rdfs:domain ex:A . ex:q rdfs:subPropertyOf ex:p . ex:r rdfs:subPropertyOf ex:q .
         ex:E owl:equivalentClass ex:F . ex:s owl:equivalentProperty ex:t .
         ex:x ex:r ex:y . ex:w a ex:E .",
    ),
];

fn load(ttl: &str) -> Store {
    let s = Store::new().unwrap();
    s.load_from_slice(RdfFormat::Turtle, format!("{PREFIXES} {ttl}").as_bytes())
        .unwrap();
    s
}

fn all_triples(s: &Store, inferred: bool) -> BTreeSet<String> {
    let opts = QueryOptions {
        include_inferred: inferred,
        ..QueryOptions::default()
    };
    match s
        .query_output("SELECT ?s ?p ?o WHERE { ?s ?p ?o }", &opts)
        .unwrap()
    {
        QueryOutput::Solutions { rows, .. } => rows
            .into_iter()
            .map(|r| {
                r.iter()
                    .map(|t| t.as_ref().unwrap().to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect(),
        _ => unreachable!(),
    }
}

// @lat: [[tests#Reasoning#Agreement with reasonable]]
#[test]
fn sql_rules_agree_with_reasonable() {
    let mut failures = Vec::new();
    for (name, ttl) in SAMPLES {
        // One store, so both see the same blank nodes.
        let store = load(ttl);
        store.materialize().unwrap();
        let ours = all_triples(&store, true);
        oxilite_reason::materialize(store.backend()).unwrap();
        let theirs = all_triples(&store, true);

        let missing: Vec<_> = theirs.difference(&ours).collect();
        let extra: Vec<_> = ours.difference(&theirs).collect();
        if !missing.is_empty() || !extra.is_empty() {
            failures.push(format!(
                "{name}:\n  only reasonable: {missing:#?}\n  only SQL rules: {extra:#?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
