//! RDF terms as ELPI terms and back.
//!
//! ELPI is typed, so every RDF term is a value of the prelude's type `term`:
//!
//! | RDF | ELPI |
//! |---|---|
//! | IRI | `(iri "http://…")` |
//! | `xsd:string` literal | `(str "…")` |
//! | `xsd:integer` literal that fits 64 bits | `(num 42)` (an ELPI `int`) |
//! | `xsd:double` literal | `(dbl 1.5)` (an ELPI `float`) |
//! | language-tagged string | `(lang "chat" "fr")` |
//! | any other literal | `(lit "2026-11-03" "http://www.w3.org/2001/XMLSchema#date")` |
//! | blank node | `(bnode "b0")` |
//! | the default graph (in `ox.quad`) | `default-graph` |

use oxrdf::vocab::xsd;
use oxrdf::{BlankNode, GraphName, Literal, NamedNode, NamedOrBlankNode, Term};

/// An ELPI term as the engine exchanges it (mirrors `leanelpi::Term`, so the pure parts of
/// this crate build without the native library).
#[derive(Debug, Clone, PartialEq)]
pub enum ETerm {
    Const(String),
    App(String, Vec<ETerm>),
    Int(i64),
    Float(f64),
    Str(String),
    Var(String),
    Other(String),
}

impl ETerm {
    fn app(c: &str, args: Vec<ETerm>) -> Self {
        Self::App(c.to_owned(), args)
    }

    /// The items of an ELPI list (`[]` / `::` cells), `None` for anything else.
    pub fn as_list(&self) -> Option<Vec<&ETerm>> {
        let mut out = Vec::new();
        let mut t = self;
        loop {
            match t {
                ETerm::Const(c) if c == "[]" => return Some(out),
                ETerm::App(c, a) if c == "::" && a.len() == 2 => {
                    out.push(&a[0]);
                    t = &a[1];
                }
                _ => return None,
            }
        }
    }

    /// True when the term holds no variable, so it can select rows.
    pub fn is_ground(&self) -> bool {
        match self {
            ETerm::Var(_) | ETerm::Other(_) => false,
            ETerm::App(_, a) => a.iter().all(ETerm::is_ground),
            _ => true,
        }
    }
}

/// The constant naming the default graph in `ox.quad`.
pub const DEFAULT_GRAPH: &str = "default-graph";

/// The ELPI term of an RDF term; `None` for a triple term.
// @tg: implements:: [[openspec:elpi-programs#RDF terms as typed values]] {since: "2026-11-03"}
pub fn to_elpi(t: &Term) -> Option<ETerm> {
    Some(match t {
        Term::NamedNode(n) => ETerm::app("iri", vec![ETerm::Str(n.as_str().into())]),
        Term::BlankNode(b) => ETerm::app("bnode", vec![ETerm::Str(b.as_str().into())]),
        Term::Literal(l) => literal(l),
        #[allow(unreachable_patterns)]
        _ => return None,
    })
}

fn literal(l: &Literal) -> ETerm {
    if let Some(lang) = l.language() {
        return ETerm::app(
            "lang",
            vec![ETerm::Str(l.value().into()), ETerm::Str(lang.into())],
        );
    }
    let dt = l.datatype();
    if dt == xsd::STRING {
        return ETerm::app("str", vec![ETerm::Str(l.value().into())]);
    }
    if dt == xsd::INTEGER {
        if let Ok(n) = l.value().parse::<i64>() {
            return ETerm::app("num", vec![ETerm::Int(n)]);
        }
    }
    if dt == xsd::DOUBLE {
        if let Ok(f) = l.value().parse::<f64>() {
            if f.is_finite() {
                return ETerm::app("dbl", vec![ETerm::Float(f)]);
            }
        }
    }
    ETerm::app(
        "lit",
        vec![ETerm::Str(l.value().into()), ETerm::Str(dt.as_str().into())],
    )
}

/// The ELPI term of a graph name.
pub fn graph_to_elpi(g: &GraphName) -> Option<ETerm> {
    match g {
        GraphName::DefaultGraph => Some(ETerm::Const(DEFAULT_GRAPH.into())),
        GraphName::NamedNode(n) => to_elpi(&n.clone().into()),
        GraphName::BlankNode(b) => to_elpi(&b.clone().into()),
    }
}

/// The RDF term of a ground ELPI term of type `term`, `None` for anything else.
pub fn from_elpi(t: &ETerm) -> Option<Term> {
    let ETerm::App(c, a) = t else {
        return None;
    };
    let s = |i: usize| match a.get(i) {
        Some(ETerm::Str(s)) => Some(s.as_str()),
        _ => None,
    };
    Some(match (c.as_str(), a.len()) {
        ("iri", 1) => NamedNode::new(s(0)?).ok()?.into(),
        ("bnode", 1) => BlankNode::new(s(0)?).ok()?.into(),
        ("str", 1) => Literal::new_simple_literal(s(0)?).into(),
        ("lang", 2) => Literal::new_language_tagged_literal(s(0)?, s(1)?)
            .ok()?
            .into(),
        ("lit", 2) => Literal::new_typed_literal(s(0)?, NamedNode::new(s(1)?).ok()?).into(),
        ("num", 1) => match &a[0] {
            ETerm::Int(n) => Literal::from(*n).into(),
            _ => return None,
        },
        ("dbl", 1) => match &a[0] {
            ETerm::Float(f) => Literal::from(*f).into(),
            _ => return None,
        },
        _ => return None,
    })
}

/// The graph name of a ground ELPI graph term.
pub fn graph_from_elpi(t: &ETerm) -> Option<GraphName> {
    match t {
        ETerm::Const(c) if c == DEFAULT_GRAPH => Some(GraphName::DefaultGraph),
        t => match from_elpi(t)? {
            Term::NamedNode(n) => Some(n.into()),
            Term::BlankNode(b) => Some(b.into()),
            _ => None,
        },
    }
}

/// A subject: an IRI or a blank node.
pub fn subject_from_elpi(t: &ETerm) -> Option<NamedOrBlankNode> {
    match from_elpi(t)? {
        Term::NamedNode(n) => Some(n.into()),
        Term::BlankNode(b) => Some(b.into()),
        _ => None,
    }
}

/// An IRI.
pub fn iri_from_elpi(t: &ETerm) -> Option<NamedNode> {
    match from_elpi(t)? {
        Term::NamedNode(n) => Some(n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_round_trip() {
        let terms: Vec<Term> = vec![
            NamedNode::new_unchecked("http://ex.org/a").into(),
            BlankNode::new_unchecked("b1").into(),
            Literal::new_simple_literal("plain").into(),
            Literal::new_language_tagged_literal_unchecked("chat", "fr").into(),
            Literal::from(42_i64).into(),
            Literal::from(1.5_f64).into(),
            Literal::new_typed_literal("2026-11-03", xsd::DATE).into(),
        ];
        for t in terms {
            let e = to_elpi(&t).unwrap();
            assert!(e.is_ground());
            assert_eq!(from_elpi(&e).as_ref(), Some(&t), "{t}");
        }
        assert!(matches!(
            to_elpi(&Literal::from(42_i64).into()),
            Some(ETerm::App(c, _)) if c == "num"
        ));
        assert_eq!(
            graph_from_elpi(&graph_to_elpi(&GraphName::DefaultGraph).unwrap()),
            Some(GraphName::DefaultGraph)
        );
    }
}
