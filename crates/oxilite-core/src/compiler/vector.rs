//! `SERVICE <oxilite:vector/NAME> { … }`: nearest-neighbour search as a derived table.
//!
//! The body names the search with triples of one subject: `oxl:query` (a JSON array literal,
//! or the IRI of a node whose embedding is used), `oxl:k`, and the outputs `oxl:node`,
//! `oxl:distance` and `oxl:score`. The search is `vector::knn_sql`, placed in the FROM list, so
//! it joins, filters and orders with the rest of the query in the same statement.
//!
// @lat: [[architecture#Vector indexes#Search]]

use super::expr::V;
use super::plan::Pos;
use super::{Binding, Block, Col, Compiler, FromItem, Join};
use crate::error::{Error, Result};
use crate::vector::{self, vocab, QueryVector};
use oxrdf::Term;
use spargebra::algebra::GraphPattern;
use spargebra::term::{NamedNodePattern, TermPattern};

impl Compiler<'_> {
    pub(crate) fn vector_service(&mut self, name: &str, inner: &GraphPattern) -> Result<Block> {
        if !self.caps.vectors {
            return Err(Error::unsupported(format!(
                "vector search needs a backend with vector functions, such as Turso (oxilite-turso); {} has none",
                self.caps.name
            )));
        }
        let index = vector::find(&self.stats.vector_indexes, name)
            .ok_or_else(|| Error::Other(format!("there is no vector index named {name}")))?
            .clone();
        if !vector::is_built(&index, &self.stats.vector_built) {
            return Err(Error::Other(format!(
                "vector index {name} is defined but not built; run sync_vector_indexes"
            )));
        }
        let GraphPattern::Bgp { patterns } = inner else {
            return Err(Error::Other(format!(
                "SERVICE <oxilite:vector/{name}> takes triples naming the search (oxl:query, oxl:k, oxl:node, oxl:distance, oxl:score)"
            )));
        };
        let mut query = None;
        let mut k = vector::DEFAULT_K;
        let mut node = None;
        let mut distance = None;
        let mut score = None;
        for t in patterns {
            let NamedNodePattern::NamedNode(p) = &t.predicate else {
                return Err(Error::Other(
                    "a vector search predicate must be an oxl: IRI, not a variable".into(),
                ));
            };
            match p.as_str() {
                vocab::QUERY => {
                    query = Some(match &t.object {
                        TermPattern::Literal(l) => QueryVector::Vector(l.value().to_owned()),
                        TermPattern::NamedNode(n) => QueryVector::Node(n.clone().into()),
                        _ => {
                            return Err(Error::Other(
                                "oxl:query must be a vector literal (\"[0.1, 0.2]\") or a node IRI"
                                    .into(),
                            ))
                        }
                    })
                }
                vocab::K => {
                    k = match &t.object {
                        TermPattern::Literal(l) => l.value().parse::<u64>().ok(),
                        _ => None,
                    }
                    .ok_or_else(|| Error::Other("oxl:k must be a positive integer".into()))?
                }
                vocab::NODE => node = Some(t.object.clone()),
                vocab::DISTANCE => distance = Some(t.object.clone()),
                vocab::SCORE => score = Some(t.object.clone()),
                other => {
                    return Err(Error::Other(format!(
                        "<{other}> is not a vector search predicate (oxl:query, oxl:k, oxl:node, oxl:distance, oxl:score)"
                    )))
                }
            }
        }
        let query = query.ok_or_else(|| {
            Error::Other(format!(
                "SERVICE <oxilite:vector/{name}> needs an oxl:query"
            ))
        })?;
        if let QueryVector::Node(n) = &query {
            self.constant_id(n)?;
        }
        let sql = index.knn_sql(&query, k)?;
        let a = self.alias("knn");
        let mut b = Block::default();
        b.from.push(FromItem {
            join: Join::First,
            item: format!("({sql}) AS {a}"),
        });
        match node {
            Some(TermPattern::Variable(v)) => {
                let i = self.var(&v);
                self.bind_pos(&mut b, &format!("{a}.s"), Pos::Var(i))?;
            }
            Some(TermPattern::NamedNode(n)) => {
                let id = self.constant_id(&Term::from(n))?;
                self.bind_pos(&mut b, &format!("{a}.s"), Pos::Const(id))?;
            }
            Some(_) => return Err(Error::Other("oxl:node must be a variable or an IRI".into())),
            None => {}
        }
        let d = format!("{a}.d");
        for (out, value, what) in [
            (distance, d.clone(), "oxl:distance"),
            (score, index.metric.score_sql(&d), "oxl:score"),
        ] {
            let Some(out) = out else { continue };
            let TermPattern::Variable(v) = out else {
                return Err(Error::Other(format!("{what} must be a variable")));
            };
            let i = self.var(&v);
            if b.cols.contains_key(&i) {
                return Err(Error::Other(format!(
                    "?{} is bound twice in the vector search",
                    v.as_str()
                )));
            }
            // An xsd:double (numeric rank 4).
            b.cols.insert(
                i,
                Binding {
                    col: Col::Val(Box::new(V::numeric(value, "4".into()))),
                    nullable: false,
                    computed: true,
                    correlated: false,
                },
            );
        }
        self.notes.push(format!(
            "vector search on {} ({}, {} dimensions, k = {k})",
            index.name,
            index.metric.name(),
            index.dimensions
        ));
        Ok(b)
    }
}
