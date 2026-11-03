//! N3 rules run by eyeron over the store: read-only reasoning and materialization.
#![cfg(feature = "n3")]

use oxilite::io::RdfFormat;
use oxilite::model::{GraphName, NamedNode, Quad, Term};
use oxilite::n3::{Limits, N3Error, Options};
use oxilite::sparql::QueryOptions;
use oxilite::store::Store;
use oxilite_core::QueryOutput;

const PREFIXES: &str = "@prefix ex: <http://example.com/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix log: <http://www.w3.org/2000/10/swap/log#> .
@prefix time: <http://www.w3.org/2000/10/swap/time#> .
";

const GRANDPARENT: &str = "{ ?x ex:parent ?y . ?y ex:parent ?z } => { ?x ex:grandparent ?z } .";

fn store(ttl: &str) -> Store {
    let s = Store::new().unwrap();
    s.load_from_slice(RdfFormat::TriG, format!("{PREFIXES}{ttl}").as_bytes())
        .unwrap();
    s
}

fn rules(n3: &str) -> String {
    format!("{PREFIXES}{n3}")
}

fn values(store: &Store, query: &str, inferred: bool) -> Vec<String> {
    let q = format!("PREFIX ex: <http://example.com/> {query}");
    let options = QueryOptions {
        include_inferred: inferred,
        ..QueryOptions::default()
    };
    match store.query_output(q.as_str(), &options).unwrap() {
        QueryOutput::Solutions { rows, .. } => {
            let mut v: Vec<String> = rows
                .into_iter()
                .map(|r| r[0].as_ref().map_or("UNDEF".into(), Term::to_string))
                .collect();
            v.sort();
            v
        }
        other => panic!("not a SELECT: {other:?}"),
    }
}

fn ex(n: &str) -> String {
    format!("<http://example.com/{n}>")
}

const FAMILY: &str = "ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy . ex:cy ex:parent ex:dan .";

// @tg: verifies:: [[openspec:n3-rules#Read-only reasoning#Conclusions without writes]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#Read-only reasoning writes nothing]] {level: integration}
#[test]
fn read_only_reasoning_writes_nothing() {
    let s = store(FAMILY);
    let before = s.len().unwrap();
    let r = s.n3_reason(&rules(GRANDPARENT)).unwrap();
    let got: Vec<String> = r.derived.iter().map(ToString::to_string).collect();
    assert_eq!(
        got,
        [
            "<http://example.com/ada> <http://example.com/grandparent> <http://example.com/cy>",
            "<http://example.com/bob> <http://example.com/grandparent> <http://example.com/dan>",
        ]
    );
    assert!(r.complete);
    assert_eq!(s.len().unwrap(), before);
    assert!(values(&s, "SELECT ?z WHERE { ?x ex:grandparent ?z }", true).is_empty());
}

// @tg: verifies:: [[openspec:n3-rules#Materialized conclusions#Inferences for queries]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#Materialized conclusions are attributed inferences]] {level: integration}
#[test]
fn materialized_conclusions_are_attributed_inferences() {
    let s = store(FAMILY);
    let stats = s.n3_materialize(&rules(GRANDPARENT)).unwrap();
    assert_eq!(stats.inferred, 2);
    assert_eq!(stats.producer, "n3");
    let q = "SELECT ?z WHERE { ex:ada ex:grandparent ?z }";
    assert_eq!(values(&s, q, true), [ex("cy")]);
    assert!(
        values(&s, q, false).is_empty(),
        "asserted data is unchanged"
    );
    let quad = Quad::new(
        NamedNode::new_unchecked("http://example.com/ada"),
        NamedNode::new_unchecked("http://example.com/grandparent"),
        NamedNode::new_unchecked("http://example.com/cy"),
        GraphName::DefaultGraph,
    );
    assert_eq!(s.inference_producers(&quad).unwrap(), ["n3"]);
}

// @tg: verifies:: [[openspec:n3-rules#Materialized conclusions#A rerun reflects the store]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#A rerun replaces the producer's conclusions]] {level: integration}
#[test]
fn rerun_replaces_the_producers_conclusions() {
    let s = store(FAMILY);
    s.n3_materialize(&rules(GRANDPARENT)).unwrap();
    s.update("PREFIX ex: <http://example.com/> DELETE DATA { ex:bob ex:parent ex:cy }")
        .unwrap();
    let stats = s.n3_materialize(&rules(GRANDPARENT)).unwrap();
    assert_eq!(stats.inferred, 0);
    assert!(values(&s, "SELECT ?z WHERE { ?x ex:grandparent ?z }", true).is_empty());
    // another producer's inferences are not touched by an N3 run
    s.materialize().unwrap();
    let named = Options {
        producer: "family".into(),
        ..Options::default()
    };
    s.n3_materialize_with(
        &rules("{ ?x ex:parent ?y } => { ?y ex:child ?x } ."),
        &named,
    )
    .unwrap();
    s.n3_materialize(&rules(GRANDPARENT)).unwrap();
    assert_eq!(
        values(&s, "SELECT ?c WHERE { ex:bob ex:child ?c }", true),
        [ex("ada")]
    );
}

// @tg: verifies:: [[openspec:n3-rules#Scoped and pruned input#Only body predicates are read]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#Input is pruned to the predicates rules read]] {level: integration}
#[test]
fn input_is_pruned_to_the_predicates_rules_read() {
    let s = store(&format!(
        "{FAMILY} ex:ada ex:name \"Ada\" . ex:bob ex:age 40 ."
    ));
    let r = s.n3_reason(&rules(GRANDPARENT)).unwrap();
    assert_eq!(r.loaded, 3, "only ex:parent statements are read");
    assert_eq!(r.info.predicates, ["http://example.com/parent"]);
    // a variable predicate reads the whole scope
    let any = s
        .n3_reason(&rules("{ ?s ?p ?o } => { ?s ex:mentioned true } ."))
        .unwrap();
    assert_eq!(any.loaded, 5);
    // without pruning too
    let off = Options {
        prune: false,
        ..Options::default()
    };
    assert_eq!(
        s.n3_reason_with(&rules(GRANDPARENT), &off).unwrap().loaded,
        5
    );
}

// @tg: verifies:: [[openspec:n3-rules#Scoped and pruned input#Graph scope]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#Graph scope]] {level: integration}
#[test]
fn graph_scope() {
    let s = store(
        "ex:g1 { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy . }
         ex:g2 { ex:eve ex:parent ex:fay . ex:fay ex:parent ex:gus . }",
    );
    let all = s.n3_reason(&rules(GRANDPARENT)).unwrap();
    assert_eq!(all.derived.len(), 2, "graphs merged");
    let g1 = Options {
        graphs: Some(vec![
            NamedNode::new_unchecked("http://example.com/g1").into()
        ]),
        ..Options::default()
    };
    let r = s.n3_reason_with(&rules(GRANDPARENT), &g1).unwrap();
    assert_eq!(r.derived.len(), 1);
    assert_eq!(r.derived[0].subject.to_string(), ex("ada"));
}

// @tg: verifies:: [[openspec:n3-rules#Own conclusions are never input#A stale conclusion does not feed itself]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#A producer never feeds on its own conclusions]] {level: integration}
#[test]
fn a_producer_never_feeds_on_its_own_conclusions() {
    let s = store("ex:a ex:p ex:b .");
    let copy = rules("{ ?x ex:p ?y } => { ?x ex:q ?y } .");
    s.n3_materialize(&copy).unwrap();
    // the asserted premise goes; with include_inferred the old conclusion must not keep itself
    s.update("PREFIX ex: <http://example.com/> DELETE DATA { ex:a ex:p ex:b }")
        .unwrap();
    let own = Options {
        include_inferred: true,
        ..Options::default()
    };
    let stats = s
        .n3_materialize_with(&rules("{ ?x ex:q ?y } => { ?x ex:q ?y } ."), &own)
        .unwrap();
    assert_eq!(stats.inferred, 0);
    // another producer's conclusions are ordinary input
    s.update("PREFIX ex: <http://example.com/> INSERT DATA { ex:a ex:p ex:b }")
        .unwrap();
    let first = Options {
        producer: "first".into(),
        ..Options::default()
    };
    s.n3_materialize_with(&copy, &first).unwrap();
    let second = Options {
        producer: "second".into(),
        include_inferred: true,
        ..Options::default()
    };
    let r = s
        .n3_reason_with(&rules("{ ?x ex:q ?y } => { ?y ex:r ?x } ."), &second)
        .unwrap();
    assert_eq!(r.derived.len(), 1);
}

// @tg: verifies:: [[openspec:n3-rules#Inference fuses#Fuse aborts materialization]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#An inference fuse aborts materialization]] {level: integration}
#[test]
fn an_inference_fuse_aborts_materialization() {
    let s = store("ex:rex a ex:Dog , ex:Cat .");
    let fuse = rules(
        "{ ?x a ex:Dog } => { ?x a ex:Animal } .
         { ?x a ex:Dog . ?x a ex:Cat } => false .",
    );
    let r = s.n3_reason(&fuse).unwrap();
    assert!(r.fuse.is_some(), "reasoning reports the fuse");
    let e = s.n3_materialize(&fuse).unwrap_err();
    assert!(matches!(e, N3Error::Fuse(_)), "{e}");
    assert!(values(&s, "SELECT ?x WHERE { ?x a ex:Animal }", true).is_empty());
}

// @tg: verifies:: [[openspec:n3-rules#Bounded and reproducible runs#Incomplete runs are not stored]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#Limits make a run incomplete]] {level: integration}
#[test]
fn limits_make_a_run_incomplete() {
    let s = store(FAMILY);
    let succ = rules(
        "{ ?x ex:parent ?y } => { ?x ex:ancestor ?y } .
                      { ?x ex:parent ?y . ?y ex:ancestor ?z } => { ?x ex:ancestor ?z } .",
    );
    let tight = Options {
        limits: Limits {
            max_iterations: Some(1),
            ..Limits::default()
        },
        ..Options::default()
    };
    let r = s.n3_reason_with(&succ, &tight).unwrap();
    assert!(!r.complete);
    assert!(!r.limits.is_empty());
    let e = s.n3_materialize_with(&succ, &tight).unwrap_err();
    assert!(matches!(e, N3Error::Incomplete(_)), "{e}");
    assert_eq!(s.n3_reason(&succ).unwrap().derived.len(), 6);
}

// @tg: verifies:: [[openspec:n3-rules#Bounded and reproducible runs#Pinned clock]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#The clock can be pinned]] {level: integration}
#[test]
fn the_clock_can_be_pinned() {
    let s = store("ex:run ex:started true .");
    let clock = rules("{ ?r ex:started true . \"\" time:localTime ?t } => { ?r ex:at ?t } .");
    let pinned = Options {
        now: Some(1_700_000_000_000),
        ..Options::default()
    };
    let a = s.n3_reason_with(&clock, &pinned).unwrap();
    let b = s.n3_reason_with(&clock, &pinned).unwrap();
    assert_eq!(a.derived, b.derived);
    assert_eq!(a.derived.len(), 1);
    assert!(
        a.derived[0].object.to_string().contains("2023-11-14"),
        "{}",
        a.derived[0].object
    );
    assert!(matches!(
        a.info.monotonicity,
        oxilite::n3::Monotonicity::ClockDependent { .. }
    ));
}

// @tg: verifies:: [[openspec:n3-rules#Unstorable conclusions#Literal subjects are reported]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#Unstorable conclusions are reported, not stored]] {level: integration}
#[test]
fn unstorable_conclusions_are_reported_not_stored() {
    let s = store("ex:ada ex:name \"Ada\" .");
    let r = s
        .n3_reason(&rules(
            "{ ?x ex:name ?n } => { ?n ex:nameOf ?x . ?x ex:tags (1 2) . ?x ex:named true } .",
        ))
        .unwrap();
    assert_eq!(r.derived.len(), 1, "{:?}", r.derived);
    assert_eq!(r.unstorable.len(), 2, "{:?}", r.unstorable);
    let stats = s
        .n3_materialize(&rules(
            "{ ?x ex:name ?n } => { ?n ex:nameOf ?x . ?x ex:named true } .",
        ))
        .unwrap();
    assert_eq!((stats.inferred, stats.unstorable), (1, 1));
}

// @tg: verifies:: [[tests#N3 reasoning#Proofs on request]] {level: integration}
#[test]
fn proofs_on_request() {
    let s = store(FAMILY);
    let r = s
        .n3_reason_with(
            &rules(GRANDPARENT),
            &Options {
                proof: true,
                ..Options::default()
            },
        )
        .unwrap();
    let proof = r.proof.unwrap();
    assert!(proof.contains("grandparent"), "{proof}");
    assert!(s.n3_reason(&rules(GRANDPARENT)).unwrap().proof.is_none());
}

// @tg: verifies:: [[openspec:n3-rules#Every backend#Async store]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#N3 reasoning#The async store runs the same jobs]] {level: integration}
#[test]
fn the_async_store_runs_the_same_jobs() {
    use oxilite_core::AsyncBackend;
    // An async adapter over the bundled SQLite: the jobs are sans-IO, so this is what D1 runs.
    struct Async(oxilite::rusqlite::RusqliteBackend);
    impl AsyncBackend for Async {
        async fn execute(
            &self,
            request: &oxilite_core::Request,
        ) -> oxilite_core::Result<oxilite_core::Response> {
            oxilite_core::SyncBackend::execute(&self.0, request)
        }
        fn capabilities(&self) -> &oxilite_core::Capabilities {
            oxilite_core::SyncBackend::capabilities(&self.0)
        }
    }
    futures::executor::block_on(async {
        let s =
            oxilite::AsyncStore::open(Async(oxilite::rusqlite::RusqliteBackend::memory().unwrap()))
                .await
                .unwrap();
        s.update(format!("PREFIX ex: <http://example.com/> INSERT DATA {{ {FAMILY} }}").as_str())
            .await
            .unwrap();
        assert_eq!(
            s.n3_reason(&rules(GRANDPARENT))
                .await
                .unwrap()
                .derived
                .len(),
            2
        );
        assert_eq!(
            s.n3_materialize(&rules(GRANDPARENT))
                .await
                .unwrap()
                .inferred,
            2
        );
    });
}
