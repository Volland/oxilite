//! The host builtins `ox.triple S P O` and `ox.quad S P O G`: every call of one becomes one
//! pattern scan of the store, with its ground arguments as the pattern.

use crate::term::{
    graph_from_elpi, graph_to_elpi, iri_from_elpi, subject_from_elpi, to_elpi, ETerm,
};
use oxilite_core::ops::ReasoningInput;
use oxrdf::{GraphName, Quad, Triple};
use std::collections::HashSet;

/// The ELPI declarations of the host builtins, as `leanelpi::Program::declare` takes them.
pub const SIGNATURES: [(&str, &str); 2] = [
    ("ox.triple", "pred ox.triple o:term, o:term, o:term."),
    ("ox.quad", "pred ox.quad o:term, o:term, o:term, o:term."),
];

/// Reads quads matching a pattern (a scan of the store).
pub type Lookup<'a> = dyn FnMut(&ReasoningInput<'_>) -> oxilite_core::Result<Vec<Quad>> + 'a;

/// One run's builtins: the scope it reads, and the rows it has returned.
// @tg: implements:: [[openspec:elpi-programs#Store access through host builtins]] {since: "2026-11-03"}
pub struct Host<'a> {
    lookup: &'a mut Lookup<'a>,
    graphs: Option<Vec<GraphName>>,
    include_inferred: bool,
    exclude_producer: Option<String>,
    /// Rows returned so far.
    pub rows: u64,
    max_rows: u64,
    /// True once `max_rows` was exceeded.
    pub over_budget: bool,
}

impl<'a> Host<'a> {
    pub fn new(
        lookup: &'a mut Lookup<'a>,
        graphs: Option<Vec<GraphName>>,
        include_inferred: bool,
        exclude_producer: Option<String>,
        max_rows: u64,
    ) -> Self {
        Self {
            lookup,
            graphs,
            include_inferred,
            exclude_producer,
            rows: 0,
            max_rows,
            over_budget: false,
        }
    }

    /// Answers one call of builtin `name`: one row per answer, a term per argument.
    pub fn call(&mut self, name: &str, args: &[ETerm]) -> Result<Vec<Vec<ETerm>>, String> {
        let quad = match (name, args.len()) {
            ("ox.triple", 3) => false,
            ("ox.quad", 4) => true,
            _ => return Err(format!("unknown host predicate {name}/{}", args.len())),
        };
        let subject = ground(&args[0]).map(subject_from_elpi);
        let predicate = ground(&args[1]).map(iri_from_elpi);
        let object = ground(&args[2]).map(crate::term::from_elpi);
        let graph = if quad {
            ground(&args[3]).map(graph_from_elpi)
        } else {
            None
        };
        // a ground argument that is no RDF term of its position matches nothing
        if matches!(subject, Some(None))
            || matches!(predicate, Some(None))
            || matches!(object, Some(None))
            || matches!(graph, Some(None))
        {
            return Ok(Vec::new());
        }
        let (subject, predicate, object, graph) = (
            subject.flatten(),
            predicate.flatten(),
            object.flatten(),
            graph.flatten(),
        );
        // a graph argument narrows the scope; outside it the call matches nothing
        let graphs: Option<Vec<GraphName>> = match (&graph, &self.graphs) {
            (Some(g), Some(scope)) if !scope.contains(g) => return Ok(Vec::new()),
            (Some(g), _) => Some(vec![g.clone()]),
            (None, scope) => scope.clone(),
        };
        let input = ReasoningInput {
            subject: subject.as_ref().map(Into::into),
            object: object.as_ref().map(Into::into),
            predicates: predicate.as_ref().map(|p| vec![p.as_ref()]),
            graphs: graphs
                .as_ref()
                .map(|gs| gs.iter().map(GraphName::as_ref).collect()),
            include_inferred: self.include_inferred,
            exclude_producer: self.exclude_producer.as_deref(),
        };
        let quads = (self.lookup)(&input).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for q in quads {
            let row = if quad {
                let Some(g) = graph_to_elpi(&q.graph_name) else {
                    continue;
                };
                let Some(mut row) = elpi_triple(&Triple::from(q)) else {
                    continue;
                };
                row.push(g);
                row
            } else {
                let t = Triple::from(q);
                if !seen.insert(t.clone()) {
                    continue; // the same triple in several graphs answers once
                }
                match elpi_triple(&t) {
                    Some(r) => r,
                    None => continue,
                }
            };
            if row.len() == args.len() {
                out.push(row);
            }
        }
        self.rows += out.len() as u64;
        if self.max_rows > 0 && self.rows > self.max_rows {
            self.over_budget = true;
            return Err(format!(
                "the program read more than {} rows from the store",
                self.max_rows
            ));
        }
        Ok(out)
    }
}

/// The term, when it holds no variable (so it can select rows).
fn ground(t: &ETerm) -> Option<&ETerm> {
    t.is_ground().then_some(t)
}

/// `[S, P, O]` of a triple, `None` when a term is a triple term.
fn elpi_triple(t: &Triple) -> Option<Vec<ETerm>> {
    Some(vec![
        to_elpi(&t.subject.clone().into())?,
        to_elpi(&t.predicate.clone().into())?,
        to_elpi(&t.object)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, NamedNode};

    fn iri(s: &str) -> ETerm {
        ETerm::App("iri".into(), vec![ETerm::Str(format!("http://ex.org/{s}"))])
    }

    fn quad(s: &str, p: &str, o: &str, g: Option<&str>) -> Quad {
        let n = |x: &str| NamedNode::new_unchecked(format!("http://ex.org/{x}"));
        Quad::new(
            n(s),
            n(p),
            n(o),
            g.map_or(GraphName::DefaultGraph, |g| n(g).into()),
        )
    }

    // @tg: verifies:: [[openspec:elpi-programs#Store access through host builtins#Bound arguments select]] {since: "2026-11-03", level: unit}
    // @tg: verifies:: [[tests#ELPI reasoning#Host calls are pattern scans]] {level: unit}
    #[test]
    fn host_calls_are_pattern_scans() {
        let data = vec![
            quad("a", "p", "b", None),
            quad("a", "p", "b", Some("g")),
            quad("a", "p", "c", Some("g")),
        ];
        let mut asked = Vec::new();
        let mut lookup = |input: &ReasoningInput<'_>| {
            asked.push((
                input.subject.map(|s| s.to_string()),
                input.predicates.as_ref().map(|p| p.len()),
                input.graphs.as_ref().map(|g| g.len()),
            ));
            Ok(data
                .iter()
                .filter(|q| input.subject.is_none_or(|s| q.subject.as_ref() == s))
                .filter(|q| {
                    input
                        .graphs
                        .as_ref()
                        .is_none_or(|gs| gs.contains(&q.graph_name.as_ref()))
                })
                .cloned()
                .collect())
        };
        let mut h = Host::new(&mut lookup, None, false, None, 0);
        let var = |v: &str| ETerm::Var(v.into());
        // a ground subject and predicate select; the same triple in two graphs answers once
        let rows = h
            .call("ox.triple", &[iri("a"), iri("p"), var("O")])
            .unwrap();
        assert_eq!(rows.len(), 2);
        // ox.quad answers per graph, the default graph as a constant
        let rows = h
            .call("ox.quad", &[iri("a"), var("P"), var("O"), var("G")])
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows
            .iter()
            .any(|r| r[3] == ETerm::Const(crate::term::DEFAULT_GRAPH.into())));
        // a ground literal in subject position matches nothing, without a scan
        let lit = crate::term::to_elpi(&Literal::new_simple_literal("x").into()).unwrap();
        assert!(h
            .call("ox.triple", &[lit, var("P"), var("O")])
            .unwrap()
            .is_empty());
        assert_eq!(h.rows, 5);
        drop(h);
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0], (Some("<http://ex.org/a>".into()), Some(1), None));

        // a row budget fails the call
        let mut lookup = |_: &ReasoningInput<'_>| Ok(data.clone());
        let mut h = Host::new(&mut lookup, None, false, None, 2);
        assert!(h
            .call("ox.quad", &[var("S"), var("P"), var("O"), var("G")])
            .is_err());
        assert!(h.over_budget);
    }
}
