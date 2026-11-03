//! λProlog (ELPI) programs over the store. The engine tests need the native library:
//! `LEANELPI_DIST=… cargo test -p oxilite --features elpi-native --test elpi`.
#![cfg(feature = "elpi")]

use oxilite::io::RdfFormat;
use oxilite::store::Store;

const TTL: &str = "@prefix ex: <http://example.com/> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy .
ex:ada a ex:Person ; ex:name \"Ada\" ; ex:age 36 .
ex:bob a ex:Person ; ex:age 12 .
ex:g1 { ex:cy ex:likes \"chat\"@fr . }
";

/// Ancestors over `ox.triple`, used by several tests.
const FAMILY: &str = r#"
pred parent o:term, o:term.
parent X Y :- ox.triple X (iri "http://example.com/parent") Y.
pred anc o:term, o:term.
anc X Y :- parent X Y.
anc X Z :- parent X Y, anc Y Z.
"#;

fn store() -> Store {
    let s = Store::new().unwrap();
    s.load_from_slice(RdfFormat::TriG, TTL.as_bytes()).unwrap();
    s
}

// @tg: verifies:: [[openspec:elpi-programs#Optional ELPI engine#Build without the library]] {since: "2026-11-03", level: integration}
// @tg: verifies:: [[tests#ELPI reasoning#Without the engine every run is unsupported]] {level: integration}
#[cfg(not(feature = "elpi-native"))]
#[test]
fn without_the_engine_every_run_is_unsupported() {
    let e = store().elpi_query(FAMILY, "anc X Y").unwrap_err();
    assert!(matches!(e, oxilite::elpi::ElpiError::Unsupported(_)), "{e}");
    assert!(!oxilite::elpi::available());
}

#[cfg(feature = "elpi-native")]
mod native {
    use super::*;
    use oxilite::elpi::{Binding, ElpiError, Options};
    use oxilite::model::{GraphName, NamedNode, Quad, Term};
    use oxilite::sparql::QueryOptions;
    use oxilite_core::QueryOutput;

    fn iri(n: &str) -> Binding {
        Binding::Term(NamedNode::new_unchecked(format!("http://example.com/{n}")).into())
    }

    // @tg: verifies:: [[tests#ELPI reasoning#Goals read the store through host builtins]] {level: integration}
    #[test]
    fn goals_read_the_store_through_host_builtins() {
        let s = store();
        let r = s
            .elpi_query(FAMILY, r#"anc (iri "http://example.com/ada") Y"#)
            .unwrap();
        let mut ys: Vec<String> = r
            .solutions
            .iter()
            .map(|s| format!("{:?}", s.bindings["Y"]))
            .collect();
        ys.sort();
        assert_eq!(
            ys,
            [format!("{:?}", iri("bob")), format!("{:?}", iri("cy"))]
        );
        assert!(r.exhausted);
        assert!(r.rows_read >= 3, "bound arguments become pattern scans");
    }

    // @tg: verifies:: [[openspec:elpi-programs#RDF terms as typed values#Arithmetic on stored numbers]] {since: "2026-11-03", level: integration}
    // @tg: verifies:: [[tests#ELPI reasoning#RDF terms are typed ELPI values]] {level: integration}
    #[test]
    fn rdf_terms_are_typed_elpi_values() {
        let s = store();
        // integers are ELPI ints: arithmetic works on them
        let adults = r#"
            pred adult o:term.
            adult P :- ox.triple P (iri "http://example.com/age") (num A), A >= 18.
        "#;
        let r = s.elpi_query(adults, "adult P").unwrap();
        assert_eq!(r.solutions.len(), 1);
        assert_eq!(r.solutions[0].bindings["P"], iri("ada"));
        // language-tagged strings and graphs
        let r = s.elpi_query("", r#"ox.quad S P (lang L "fr") G"#).unwrap();
        assert_eq!(r.solutions.len(), 1);
        assert_eq!(r.solutions[0].bindings["G"], iri("g1"));
        assert_eq!(
            r.solutions[0].bindings["L"],
            Binding::Value("\"chat\"".into())
        );
        // the default graph is a constant
        let r = s
            .elpi_query("", r#"ox.quad (iri "http://example.com/ada") (iri "http://example.com/name") N default-graph"#)
            .unwrap();
        assert_eq!(
            r.solutions[0].bindings["N"],
            Binding::Term(oxilite::model::Literal::new_simple_literal("Ada").into())
        );
    }

    // @tg: verifies:: [[openspec:elpi-programs#Goal queries#Hypothetical goal]] {since: "2026-11-03", level: integration}
    // @tg: verifies:: [[tests#ELPI reasoning#Hypothetical reasoning]] {level: integration}
    #[test]
    fn hypothetical_reasoning() {
        let s = store();
        let program = r#"
            pred edge o:term, o:term.
            edge X Y :- ox.triple X (iri "http://example.com/parent") Y.
            pred reach o:term, o:term.
            reach X Y :- edge X Y.
            reach X Z :- edge X Y, reach Y Z.
        "#;
        let before = s.len().unwrap();
        let dan = r#"(iri "http://example.com/dan")"#;
        let cy = r#"(iri "http://example.com/cy")"#;
        let ada = r#"(iri "http://example.com/ada")"#;
        assert!(s
            .elpi_query(program, &format!("reach {ada} {dan}"))
            .unwrap()
            .solutions
            .is_empty());
        // assume one more edge, only for this goal
        let r = s
            .elpi_query(program, &format!("(edge {cy} {dan} => reach {ada} {dan})"))
            .unwrap();
        assert_eq!(r.solutions.len(), 1);
        assert_eq!(s.len().unwrap(), before, "the assumption is never written");
    }

    // @tg: verifies:: [[openspec:elpi-programs#Constraint checks#Missing name is an error]] {since: "2026-11-03", level: integration}
    // @tg: verifies:: [[tests#ELPI reasoning#Checks report violations]] {level: integration}
    #[test]
    fn checks_report_violations() {
        let s = store();
        let program = r#"
            ox.violation error "missing-name" "a person needs a name" [P] :-
              ox.triple P (iri "http://www.w3.org/1999/02/22-rdf-syntax-ns#type") (iri "http://example.com/Person"),
              not (ox.triple P (iri "http://example.com/name") _).
            ox.violation warning "minor" "a person under 18" [P] :-
              ox.triple P (iri "http://example.com/age") (num A), A < 18.
        "#;
        let v = s.elpi_check(program).unwrap();
        assert_eq!(v.len(), 2, "{v:?}");
        let by = |n: &str| v.iter().find(|x| x.name == n).unwrap();
        let missing = by("missing-name");
        assert_eq!(missing.severity, "error");
        assert_eq!(missing.message, "a person needs a name");
        assert_eq!(
            missing.focus,
            [Term::from(NamedNode::new_unchecked(
                "http://example.com/bob"
            ))]
        );
        assert_eq!(by("minor").severity, "warning");
    }

    // @tg: verifies:: [[openspec:elpi-programs#Materialized conclusions#Conclusions as inferences]] {since: "2026-11-03", level: integration}
    // @tg: verifies:: [[tests#ELPI reasoning#Conclusions materialize as inferences]] {level: integration}
    #[test]
    fn conclusions_materialize_as_inferences() {
        let s = store();
        let program = format!(
            r#"{FAMILY}
            ox.conclude (triple X (iri "http://example.com/ancestor") Y) :- anc X Y.
            ox.conclude (triple (str "not a subject") (iri "http://example.com/p") (num 1))."#
        );
        let stats = s.elpi_materialize(&program).unwrap();
        assert_eq!((stats.inferred, stats.unstorable), (3, 1));
        let q = "SELECT ?y WHERE { <http://example.com/ada> <http://example.com/ancestor> ?y }";
        let inferred = QueryOptions {
            include_inferred: true,
            ..QueryOptions::default()
        };
        let QueryOutput::Solutions { rows, .. } = s.query_output(q, &inferred).unwrap() else {
            panic!("not a SELECT")
        };
        assert_eq!(rows.len(), 2);
        let quad = Quad::new(
            NamedNode::new_unchecked("http://example.com/ada"),
            NamedNode::new_unchecked("http://example.com/ancestor"),
            NamedNode::new_unchecked("http://example.com/cy"),
            GraphName::DefaultGraph,
        );
        assert_eq!(s.inference_producers(&quad).unwrap(), ["elpi"]);
        // a rerun replaces them
        s.update("DELETE DATA { <http://example.com/bob> <http://example.com/parent> <http://example.com/cy> }")
            .unwrap();
        assert_eq!(s.elpi_materialize(&program).unwrap().inferred, 1);
    }

    // @tg: verifies:: [[openspec:elpi-programs#Bounded runs#Endless recursion runs out of fuel]] {since: "2026-11-03", level: integration}
    // @tg: verifies:: [[tests#ELPI reasoning#Runs are bounded]] {level: integration}
    #[test]
    fn runs_are_bounded() {
        let s = store();
        let looping = "pred loop.\nloop :- loop.";
        let e = s
            .elpi_query_with(
                looping,
                "loop",
                &Options {
                    fuel: 10_000,
                    ..Options::default()
                },
            )
            .unwrap_err();
        assert!(matches!(e, ElpiError::OutOfFuel(_)), "{e}");
        let e = s
            .elpi_query_with(
                FAMILY,
                "anc X Y",
                &Options {
                    max_rows: 2,
                    ..Options::default()
                },
            )
            .unwrap_err();
        assert!(matches!(e, ElpiError::Budget(_)), "{e}");
        // a program error names the problem
        let e = s
            .elpi_query("pred p o:int.\np X :- q X.", "p X")
            .unwrap_err();
        assert!(
            matches!(e, ElpiError::Program(_) | ElpiError::Run(_)),
            "{e}"
        );
    }
}
