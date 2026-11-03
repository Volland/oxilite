//! A parsed N3 rule set and what it reads: the predicates of its rule bodies (to prune the
//! input), whether a body has a variable predicate (no pruning then), and its monotonicity.

use crate::error::{N3Error, Result};
use eyeron::ast::{LOG_COLLECT_ALL_IN, LOG_FOR_ALL_IN, LOG_NOT_INCLUDES, TIME_LOCAL_TIME};
use eyeron::{Document, Graph, Term as NTerm};
use std::collections::BTreeSet;

/// Builtins whose answer can change when a fact is added: negation as failure and aggregates.
const NON_MONOTONIC: [&str; 3] = [LOG_NOT_INCLUDES, LOG_COLLECT_ALL_IN, LOG_FOR_ALL_IN];

/// Is this a builtin predicate (evaluated by the reasoner, never read from the store)?
pub fn is_builtin(iri: &str) -> bool {
    iri.starts_with("http://www.w3.org/2000/10/swap/") || iri.starts_with("urn:eyeron:")
}

/// Whether adding facts can only add conclusions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Monotonicity {
    Monotonic,
    /// A rule uses negation as failure or an aggregate (`log:notIncludes`, `log:collectAllIn`,
    /// `log:forAllIn`): its conclusions are only right for the input it ran on.
    NonMonotonic {
        rule: usize,
        builtin: String,
    },
    /// A rule reads the clock (`time:localTime`): pin it with `Options::now` for reproducible runs.
    ClockDependent {
        rule: usize,
        builtin: String,
    },
}

/// What a rule set is, as reported by [`RuleSet::info`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSetInfo {
    /// Forward (`=>`), backward (`<=`) and query rules.
    pub rules: usize,
    /// Facts written in the rule text itself (they are part of every run's input).
    pub facts: usize,
    /// Store predicates the rule bodies read, sorted.
    pub predicates: Vec<String>,
    /// A body has a variable predicate, so the whole scope is loaded.
    pub variable_predicate: bool,
    pub monotonicity: Monotonicity,
}

/// N3 rule text, parsed once and reusable across runs.
#[derive(Debug, Clone)]
pub struct RuleSet {
    pub(crate) document: Document,
    info: RuleSetInfo,
}

fn line_of(text: &str, offset: usize) -> usize {
    1 + text
        .as_bytes()
        .iter()
        .take(offset)
        .filter(|&&b| b == b'\n')
        .count()
}

impl RuleSet {
    /// Parses N3 rule text (`{ … } => { … } .`, `<=`, `log:query`, facts). eyeron's usual
    /// prefixes (`rdf:`, `rdfs:`, `owl:`, `xsd:`, `log:`, `math:`, `string:`, `list:`,
    /// `time:`) are predeclared.
    pub fn parse(n3: &str) -> Result<Self> {
        let document = eyeron::parse_n3(n3, None).map_err(|e| N3Error::Parse {
            line: e.offset.map_or(0, |o| line_of(n3, o)),
            message: e.message.clone(),
        })?;
        Ok(Self::from_document(document))
    }

    fn from_document(document: Document) -> Self {
        let mut body = BodyUse::default();
        let mut non_monotonic = None;
        let mut clock = None;
        for (i, rule) in document.rules.iter().enumerate() {
            let mut b = BodyUse::default();
            b.graph(&rule.premise);
            // a conclusion may itself be a rule, whose body then reads the store too
            for t in &rule.conclusion.triples {
                for term in [&t.s, &t.p, &t.o] {
                    b.term(term);
                }
            }
            let has = |p: &str| b.preds.contains(p);
            if non_monotonic.is_none() {
                if let Some(x) = NON_MONOTONIC.into_iter().find(|x| has(x)) {
                    non_monotonic = Some((i, x.to_owned()));
                }
            }
            if clock.is_none() && has(TIME_LOCAL_TIME) {
                clock = Some((i, TIME_LOCAL_TIME.to_owned()));
            }
            body.var_pred |= b.var_pred;
            body.preds.extend(b.preds);
        }
        let monotonicity = match (non_monotonic, clock) {
            (Some((rule, builtin)), _) => Monotonicity::NonMonotonic { rule, builtin },
            (None, Some((rule, builtin))) => Monotonicity::ClockDependent { rule, builtin },
            (None, None) => Monotonicity::Monotonic,
        };
        let info = RuleSetInfo {
            rules: document.rules.len(),
            facts: document.facts.len(),
            predicates: body.preds.into_iter().filter(|p| !is_builtin(p)).collect(),
            variable_predicate: body.var_pred,
            monotonicity,
        };
        Self { document, info }
    }

    pub fn info(&self) -> &RuleSetInfo {
        &self.info
    }

    /// The predicates to load, or `None` for every predicate (a variable predicate).
    pub fn pruning(&self) -> Option<&[String]> {
        (!self.info.variable_predicate).then_some(self.info.predicates.as_slice())
    }
}

/// The predicates a rule body reads, and whether it has a variable predicate.
#[derive(Default)]
struct BodyUse {
    preds: BTreeSet<String>,
    var_pred: bool,
}

impl BodyUse {
    fn graph(&mut self, g: &Graph) {
        for t in &g.triples {
            match &t.p {
                NTerm::Iri(p) => {
                    self.preds.insert(p.as_str().to_owned());
                }
                NTerm::Var(_) | NTerm::Blank(_) => self.var_pred = true,
                _ => {}
            }
            // `((items) pred) list:map ?out` applies `pred` as a predicate
            if matches!(&t.p, NTerm::Iri(p) if p.as_str() == eyeron::ast::LIST_MAP) {
                if let NTerm::List(args) = &t.s {
                    match args.get(1) {
                        Some(NTerm::Iri(p)) => {
                            self.preds.insert(p.as_str().to_owned());
                        }
                        Some(NTerm::Var(_) | NTerm::Blank(_)) => self.var_pred = true,
                        _ => {}
                    }
                }
            }
            self.term(&t.s);
            self.term(&t.o);
        }
    }

    /// Formulas nested in a term (and in its lists).
    fn term(&mut self, t: &NTerm) {
        match t {
            NTerm::Formula(g) => self.graph(g),
            NTerm::List(items) => items.iter().for_each(|i| self.term(i)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_reads_bodies() {
        let r = RuleSet::parse(
            "@prefix : <http://ex.org/> .
             { ?x :parent ?y . ?y :parent ?z } => { ?x :grandparent ?z } .",
        )
        .unwrap();
        assert_eq!(r.info().predicates, ["http://ex.org/parent"]);
        assert_eq!(r.info().monotonicity, Monotonicity::Monotonic);
        assert!(r.pruning().is_some());

        let v = RuleSet::parse("{ ?s ?p ?o } => { ?o <http://ex.org/seen> true } .").unwrap();
        assert!(v.pruning().is_none());

        let n = RuleSet::parse(
            "@prefix : <http://ex.org/> .
             { ?x a :Person . ?SCOPE log:notIncludes { ?x :email ?e } } => { ?x a :Unreachable } .",
        )
        .unwrap();
        assert!(matches!(
            n.info().monotonicity,
            Monotonicity::NonMonotonic { rule: 0, .. }
        ));
        // builtins are never loaded from the store
        assert!(!n.info().predicates.iter().any(|p| is_builtin(p)));
    }

    #[test]
    fn parse_errors_carry_a_line() {
        let e = RuleSet::parse("@prefix : <http://ex.org/> .\n{ ?x :p ?y  => { } .").unwrap_err();
        assert!(matches!(e, N3Error::Parse { line, .. } if line >= 1), "{e}");
    }
}
