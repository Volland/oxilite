//! Query-time RDFS / OWL QL reasoning and OWL 2 RL materialization.

use oxilite::io::RdfFormat;
use oxilite::model::Term;
use oxilite::sparql::{QueryOptions, Reasoning};
use oxilite::store::Store;
use oxilite_core::QueryOutput;
use oxilite_core::SyncBackend;
use std::collections::BTreeSet;

type Mk<B> = fn() -> Store<B>;

fn native() -> Store {
    Store::new().unwrap()
}

/// The platform's SQLite (`OXILITE_SQLITE_LIBRARY`, or the system library), whose parser
/// stack is limited: reasoned SQL must stay shallow enough for it.
fn dylib() -> Store<oxilite::dylib::DylibBackend> {
    let lib = std::env::var("OXILITE_SQLITE_LIBRARY").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/usr/lib/libsqlite3.dylib".into()
        } else {
            "libsqlite3.so.0".into()
        }
    });
    Store::open_with_library(lib, ":memory:").unwrap()
}

fn load<B: SyncBackend + Send + Sync + 'static>(mk: Mk<B>, ttl: &str) -> Store<B> {
    let store = mk();
    store
        .load_from_slice(
            RdfFormat::Turtle,
            format!(
                "@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
                 @prefix owl: <http://www.w3.org/2002/07/owl#> . {ttl}"
            )
            .as_bytes(),
        )
        .unwrap();
    store
}

fn opts(reasoning: Reasoning) -> QueryOptions {
    QueryOptions {
        reasoning,
        ..QueryOptions::default()
    }
}

/// Values of the first variable, as N-Triples strings, sorted.
fn values<B: SyncBackend + Send + Sync + 'static>(
    store: &Store<B>,
    query: &str,
    options: &QueryOptions,
) -> Vec<String> {
    let q = format!("PREFIX ex: <http://example.com/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> PREFIX owl: <http://www.w3.org/2002/07/owl#> {query}");
    match store.query_output(q.as_str(), options).unwrap() {
        QueryOutput::Solutions { rows, .. } => {
            let mut v: Vec<String> = rows
                .into_iter()
                .map(|r| r[0].as_ref().map_or("UNDEF".into(), Term::to_string))
                .collect();
            v.sort();
            v
        }
        QueryOutput::Boolean(b) => vec![b.to_string()],
        QueryOutput::Graph(t) => t.iter().map(ToString::to_string).collect(),
    }
}

fn ex(names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = names
        .iter()
        .map(|n| format!("<http://example.com/{n}>"))
        .collect();
    v.sort();
    v
}

// @lat: [[tests#Reasoning#Default has no inference]]
#[test]
fn default_has_no_inference() {
    default_has_no_inference_on(native);
    default_has_no_inference_on(dylib);
}

fn default_has_no_inference_on<B: SyncBackend + Send + Sync + 'static>(mk: Mk<B>) {
    let s = load(mk, "ex:Dog rdfs:subClassOf ex:Animal . ex:rex a ex:Dog .");
    assert!(values(
        &s,
        "SELECT ?x WHERE { ?x a ex:Animal }",
        &QueryOptions::default()
    )
    .is_empty());
    assert_eq!(
        values(
            &s,
            "SELECT ?x WHERE { ?x a ex:Animal }",
            &opts(Reasoning::Rdfs)
        ),
        ex(&["rex"])
    );
}

// @lat: [[tests#Reasoning#Transitive subclass chain]]
#[test]
fn transitive_subclass_chain_and_schema_updates() {
    transitive_subclass_chain_and_schema_updates_on(native);
    transitive_subclass_chain_and_schema_updates_on(dylib);
}

fn transitive_subclass_chain_and_schema_updates_on<B: SyncBackend + Send + Sync + 'static>(
    mk: Mk<B>,
) {
    let s = load(
        mk,
        "ex:A rdfs:subClassOf ex:B . ex:B rdfs:subClassOf ex:C . ex:a a ex:A . ex:b a ex:B .",
    );
    let q = "SELECT ?x WHERE { ?x a ex:C }";
    assert_eq!(values(&s, q, &opts(Reasoning::Rdfs)), ex(&["a", "b"]));
    // A SPARQL update of schema triples refreshes the closure in the same transaction.
    s.update("PREFIX ex: <http://example.com/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> INSERT DATA { ex:C rdfs:subClassOf ex:D . ex:z a ex:Z . ex:Z rdfs:subClassOf ex:A }")
        .unwrap();
    assert_eq!(
        values(&s, "SELECT ?x WHERE { ?x a ex:D }", &opts(Reasoning::Rdfs)),
        ex(&["a", "b", "z"])
    );
    // Schema patterns are answered by the closure.
    assert_eq!(
        values(
            &s,
            "SELECT ?c WHERE { ex:Z rdfs:subClassOf ?c }",
            &opts(Reasoning::Rdfs)
        ),
        ex(&["A", "B", "C", "D"])
    );
    s.update("PREFIX ex: <http://example.com/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> DELETE DATA { ex:B rdfs:subClassOf ex:C }")
        .unwrap();
    assert_eq!(values(&s, q, &opts(Reasoning::Rdfs)), Vec::<String>::new());
}

#[test]
fn subproperties_domains_and_ranges() {
    subproperties_domains_and_ranges_on(native);
    subproperties_domains_and_ranges_on(dylib);
}

fn subproperties_domains_and_ranges_on<B: SyncBackend + Send + Sync + 'static>(mk: Mk<B>) {
    let s = load(mk,
        "ex:hasMother rdfs:subPropertyOf ex:hasParent . ex:hasParent rdfs:domain ex:Person ; rdfs:range ex:Person . \
         ex:Person rdfs:subClassOf ex:Agent . ex:alice ex:hasMother ex:carol . ex:bob ex:age 3 . ex:age rdfs:range ex:Number .",
    );
    let r = opts(Reasoning::Rdfs);
    assert_eq!(
        values(&s, "SELECT ?y WHERE { ex:alice ex:hasParent ?y }", &r),
        ex(&["carol"])
    );
    assert_eq!(
        values(&s, "SELECT ?x WHERE { ?x a ex:Agent }", &r),
        ex(&["alice", "carol"])
    );
    // Literals never get types.
    assert!(values(&s, "SELECT ?x WHERE { ?x a ex:Number }", &r).is_empty());
    // Entailed triples appear once, even with several derivations.
    assert_eq!(
        values(&s, "SELECT ?c WHERE { ex:alice a ?c }", &r),
        ex(&["Agent", "Person"])
    );
    // A variable predicate sees every entailed triple.
    let all = values(&s, "SELECT ?p WHERE { ex:alice ?p ex:carol }", &r);
    assert_eq!(all, ex(&["hasMother", "hasParent"]));
    // Paths use entailed edges.
    assert_eq!(
        values(&s, "SELECT ?y WHERE { ex:alice ex:hasParent+ ?y }", &r),
        ex(&["carol"])
    );
}

#[test]
fn owl_ql_inverse_symmetric_transitive_equivalent() {
    owl_ql_inverse_symmetric_transitive_equivalent_on(native);
    owl_ql_inverse_symmetric_transitive_equivalent_on(dylib);
}

fn owl_ql_inverse_symmetric_transitive_equivalent_on<B: SyncBackend + Send + Sync + 'static>(
    mk: Mk<B>,
) {
    let s = load(mk,
        "ex:hasChild owl:inverseOf ex:hasParent . ex:knows a owl:SymmetricProperty . \
         ex:ancestor a owl:TransitiveProperty . ex:hasParent rdfs:subPropertyOf ex:ancestor . \
         ex:Human owl:equivalentClass ex:Person . \
         ex:carol ex:hasChild ex:alice . ex:alice ex:hasParent ex:bob . ex:bob ex:hasParent ex:dan . \
         ex:alice ex:knows ex:eve . ex:eve a ex:Human .",
    );
    let q = opts(Reasoning::OwlQl);
    assert_eq!(
        values(&s, "SELECT ?y WHERE { ex:alice ex:hasParent ?y }", &q),
        ex(&["bob", "carol"])
    );
    assert_eq!(
        values(&s, "SELECT ?y WHERE { ex:eve ex:knows ?y }", &q),
        ex(&["alice"])
    );
    assert_eq!(
        values(&s, "SELECT ?y WHERE { ex:alice ex:ancestor ?y }", &q),
        ex(&["bob", "carol", "dan"])
    );
    assert_eq!(
        values(&s, "SELECT ?x WHERE { ?x ex:ancestor ex:dan }", &q),
        ex(&["alice", "bob"])
    );
    assert_eq!(
        values(&s, "SELECT ?x WHERE { ?x a ex:Person }", &q),
        ex(&["eve"])
    );
    // RDFS does not use OWL property axioms.
    assert_eq!(
        values(
            &s,
            "SELECT ?y WHERE { ex:eve ex:knows ?y }",
            &opts(Reasoning::Rdfs)
        ),
        Vec::<String>::new()
    );
    // Queries the compiler cannot express still reason (spareval over entailed scans).
    assert_eq!(
        values(
            &s,
            "SELECT ?y WHERE { ex:alice ex:hasParent ?y FILTER(REGEX(STR(?y), \"b.b\")) }",
            &q
        ),
        ex(&["bob"])
    );
}

// @lat: [[tests#Reasoning#Reasoning keeps single statements]]
#[test]
fn reasoning_keeps_single_statements() {
    reasoning_keeps_single_statements_on(native);
    reasoning_keeps_single_statements_on(dylib);
}

fn reasoning_keeps_single_statements_on<B: SyncBackend + Send + Sync + 'static>(mk: Mk<B>) {
    let s = load(
        mk,
        "ex:Dog rdfs:subClassOf ex:Animal . ex:rex a ex:Dog ; ex:name \"Rex\" .",
    );
    let plan = s
        .explain_opt(
            "PREFIX ex: <http://example.com/> SELECT ?x ?n WHERE { ?x a ex:Animal ; ex:name ?n }",
            &opts(Reasoning::Rdfs),
        )
        .unwrap();
    assert!(plan.starts_with("-- oxilite: fully compiled"), "{plan}");
    assert!(plan.contains("tbox_closure"), "{plan}");
}

#[test]
fn union_default_graph_merges_entailments() {
    union_default_graph_merges_entailments_on(native);
    union_default_graph_merges_entailments_on(dylib);
}

fn union_default_graph_merges_entailments_on<B: SyncBackend + Send + Sync + 'static>(mk: Mk<B>) {
    let s = mk();
    s.load_from_slice(
        RdfFormat::TriG,
        b"@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
          ex:g1 { ex:Dog rdfs:subClassOf ex:Animal . ex:rex a ex:Dog . }
          ex:g2 { ex:rex a ex:Animal . }"
            .as_slice(),
    )
    .unwrap();
    let o = QueryOptions {
        reasoning: Reasoning::Rdfs,
        union_default_graph: true,
        ..QueryOptions::default()
    };
    assert_eq!(
        values(&s, "SELECT ?x WHERE { ?x a ex:Animal }", &o),
        ex(&["rex"])
    );
    // Per graph, only g1 entails it (schema from any graph).
    assert_eq!(
        values(
            &s,
            "SELECT ?g WHERE { GRAPH ?g { ex:rex a ex:Animal } }",
            &opts(Reasoning::Rdfs)
        ),
        ex(&["g1", "g2"])
    );
}

// @lat: [[tests#Reasoning#Materialize then query]]
#[test]
fn materialize_then_query() {
    materialize_then_query_on(native);
    materialize_then_query_on(dylib);
}

fn materialize_then_query_on<B: SyncBackend + Send + Sync + 'static>(mk: Mk<B>) {
    let s = load(mk,
        "ex:rex owl:sameAs ex:rexy . ex:rex ex:name \"Rex\" . ex:Dog rdfs:subClassOf ex:Animal . ex:rexy a ex:Dog . \
         ex:hasOwner owl:inverseOf ex:owns . ex:rex ex:hasOwner ex:alice .",
    );
    let n = s.materialize().unwrap();
    assert!(n > 0);
    let inf = QueryOptions {
        include_inferred: true,
        ..QueryOptions::default()
    };
    assert_eq!(
        values(&s, "SELECT ?n WHERE { ex:rexy ex:name ?n }", &inf),
        vec!["\"Rex\"".to_string()]
    );
    assert_eq!(
        values(&s, "SELECT ?x WHERE { ?x a ex:Animal }", &inf),
        ex(&["rex", "rexy"])
    );
    assert_eq!(
        values(&s, "SELECT ?x WHERE { ex:alice ex:owns ?x }", &inf),
        ex(&["rex", "rexy"])
    );
    assert!(values(
        &s,
        "SELECT ?n WHERE { ex:rexy ex:name ?n }",
        &QueryOptions::default()
    )
    .is_empty());
    // Re-running replaces the inferences.
    s.update("PREFIX ex: <http://example.com/> PREFIX owl: <http://www.w3.org/2002/07/owl#> DELETE DATA { ex:rex owl:sameAs ex:rexy }")
        .unwrap();
    s.materialize().unwrap();
    assert!(values(&s, "SELECT ?n WHERE { ex:rexy ex:name ?n }", &inf).is_empty());
    s.clear_inferences().unwrap();
    assert!(values(&s, "SELECT ?x WHERE { ?x a ex:Animal }", &inf).is_empty());
}

#[test]
fn materialization_rules() {
    materialization_rules_on(native);
    materialization_rules_on(dylib);
}

fn materialization_rules_on<B: SyncBackend + Send + Sync + 'static>(mk: Mk<B>) {
    let s = load(mk,
        "ex:Parent owl:intersectionOf (ex:Person ex:HasChild) . ex:ann a ex:Person , ex:HasChild . \
         ex:Pet owl:unionOf (ex:Dog ex:Cat) . ex:tom a ex:Cat . \
         ex:DogOwner owl:onProperty ex:owns ; owl:someValuesFrom ex:Dog . ex:bob ex:owns ex:rex . ex:rex a ex:Dog . \
         ex:Red owl:onProperty ex:color ; owl:hasValue ex:red . ex:car a ex:Red . \
         ex:uncle owl:propertyChainAxiom (ex:parent ex:brother) . ex:kim ex:parent ex:lee . ex:lee ex:brother ex:max . \
         ex:mother a owl:FunctionalProperty . ex:sue ex:mother ex:m1 , ex:m2 .",
    );
    s.materialize().unwrap();
    let inf = QueryOptions {
        include_inferred: true,
        ..QueryOptions::default()
    };
    let has = |q: &str| values(&s, &format!("ASK {{ {q} }}"), &inf) == vec!["true".to_string()];
    assert!(has("ex:ann a ex:Parent"));
    assert!(has("ex:tom a ex:Pet"));
    assert!(has("ex:bob a ex:DogOwner"));
    assert!(has("ex:car ex:color ex:red"));
    assert!(has("ex:kim ex:uncle ex:max"));
    assert!(has("ex:m1 owl:sameAs ex:m2"));
    let set: BTreeSet<_> = values(&s, "SELECT ?c WHERE { ex:ann a ?c }", &inf)
        .into_iter()
        .collect();
    assert!(set.contains("<http://example.com/Parent>"));
}
