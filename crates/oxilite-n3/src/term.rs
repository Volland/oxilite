//! RDF terms to eyeron's N3 terms and back.

use eyeron::{Literal as NLiteral, Term as NTerm, Triple as NTriple};
use oxrdf::vocab::xsd;
use oxrdf::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};

/// The N3 term of an RDF term; `None` for a triple term, which N3 has no form for.
pub fn to_n3(t: &Term) -> Option<NTerm> {
    Some(match t {
        Term::NamedNode(n) => NTerm::iri(n.as_str()),
        Term::BlankNode(b) => NTerm::blank(b.as_str()),
        Term::Literal(l) => NTerm::Literal(NLiteral {
            value: l.value().into(),
            datatype: match (l.language(), l.datatype()) {
                (Some(_), _) => None,
                (None, d) if d == xsd::STRING => None,
                (None, d) => Some(d.as_str().into()),
            },
            language: l.language().map(Into::into),
        }),
        #[allow(unreachable_patterns)]
        _ => return None,
    })
}

/// The N3 triple of an RDF triple; `None` when a term is a triple term.
pub fn triple_to_n3(t: &Triple) -> Option<NTriple> {
    Some(NTriple::new(
        to_n3(&t.subject.clone().into())?,
        NTerm::iri(t.predicate.as_str()),
        to_n3(&t.object)?,
    ))
}

/// The RDF term of an N3 term: IRIs, blank nodes and literals; `None` for lists, formulas and
/// variables, which the store cannot hold.
pub fn from_n3(t: &NTerm) -> Option<Term> {
    Some(match t {
        NTerm::Iri(i) => NamedNode::new(i.as_str()).ok()?.into(),
        NTerm::Blank(b) => blank(b.as_str()).into(),
        NTerm::Literal(l) => literal(l)?.into(),
        _ => return None,
    })
}

/// A blank node for an N3 label. eyeron's witness labels are deterministic for a rule firing,
/// so the same conclusion gets the same node in every run; labels RDF does not allow are hashed.
fn blank(label: &str) -> BlankNode {
    BlankNode::new(label).unwrap_or_else(|_| {
        let h = label.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
        BlankNode::new_unchecked(format!("n3{h:016x}"))
    })
}

fn literal(l: &NLiteral) -> Option<Literal> {
    let value = l.value.as_str();
    Some(match (&l.language, &l.datatype) {
        (Some(lang), _) => Literal::new_language_tagged_literal(value, lang.as_str()).ok()?,
        (None, Some(dt)) => Literal::new_typed_literal(value, NamedNode::new(dt.as_str()).ok()?),
        (None, None) => Literal::new_simple_literal(value),
    })
}

/// The RDF triple of an N3 triple, when the store can hold it: an IRI or blank subject, an IRI
/// predicate, and an IRI, blank or literal object.
// @tg: implements:: [[openspec:n3-rules#Unstorable conclusions]] {since: "2026-11-03"}
pub fn triple_from_n3(t: &NTriple) -> Option<Triple> {
    let subject: NamedOrBlankNode = match from_n3(&t.s)? {
        Term::NamedNode(n) => n.into(),
        Term::BlankNode(b) => b.into(),
        _ => return None,
    };
    let Term::NamedNode(predicate) = from_n3(&t.p)? else {
        return None;
    };
    Some(Triple::new(subject, predicate, from_n3(&t.o)?))
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
            Literal::new_language_tagged_literal_unchecked("hallo", "de").into(),
            Literal::new_typed_literal("5", xsd::INTEGER).into(),
        ];
        for t in terms {
            assert_eq!(from_n3(&to_n3(&t).unwrap()).as_ref(), Some(&t), "{t}");
        }
    }
}
