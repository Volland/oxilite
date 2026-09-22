//! Helpers shared by the blocking and async stores.

use oxilite_core::query::QueryOutput;
use oxilite_core::{Error, Result};
use oxrdf::{Quad, Variable};
use oxrdfio::{RdfParser, RdfSerializer};
use spareval::{QueryResults, QuerySolutionIter, QueryTripleIter};
use spargebra::{Query, SparqlParser, Update};
use std::io::Write;
use std::sync::Arc;

/// Anything that can be turned into a SPARQL query.
pub trait IntoQuery {
    fn into_query(self) -> Result<Query>;
}

impl IntoQuery for Query {
    fn into_query(self) -> Result<Query> {
        Ok(self)
    }
}

impl IntoQuery for &Query {
    fn into_query(self) -> Result<Query> {
        Ok(self.clone())
    }
}

impl IntoQuery for &str {
    fn into_query(self) -> Result<Query> {
        Ok(SparqlParser::new().parse_query(self)?)
    }
}

impl IntoQuery for &String {
    fn into_query(self) -> Result<Query> {
        self.as_str().into_query()
    }
}

impl IntoQuery for String {
    fn into_query(self) -> Result<Query> {
        self.as_str().into_query()
    }
}

/// Anything that can be turned into a SPARQL update.
pub trait IntoUpdate {
    fn into_update(self) -> Result<Update>;
}

impl IntoUpdate for Update {
    fn into_update(self) -> Result<Update> {
        Ok(self)
    }
}

impl IntoUpdate for &Update {
    fn into_update(self) -> Result<Update> {
        Ok(self.clone())
    }
}

impl IntoUpdate for &str {
    fn into_update(self) -> Result<Update> {
        Ok(SparqlParser::new().parse_update(self)?)
    }
}

impl IntoUpdate for &String {
    fn into_update(self) -> Result<Update> {
        self.as_str().into_update()
    }
}

impl IntoUpdate for String {
    fn into_update(self) -> Result<Update> {
        self.as_str().into_update()
    }
}

/// Converts decoded output into Oxigraph's `QueryResults`.
pub fn to_results(out: QueryOutput) -> QueryResults<'static> {
    match out {
        QueryOutput::Solutions { variables, rows } => {
            let variables: Arc<[Variable]> = variables.into();
            QueryResults::Solutions(QuerySolutionIter::from_tuples(
                variables,
                rows.into_iter().map(Ok),
            ))
        }
        QueryOutput::Boolean(b) => QueryResults::Boolean(b),
        QueryOutput::Graph(triples) => {
            QueryResults::Graph(QueryTripleIter::new(triples.into_iter().map(Ok)))
        }
    }
}

/// Parses a document into quads, renaming blank nodes (like Oxigraph's loader).
pub fn parse_all(
    parser: RdfParser,
    reader: impl std::io::Read,
    mut on_error: Option<&mut dyn FnMut(oxrdfio::RdfParseError) -> Result<()>>,
) -> Result<Vec<Quad>> {
    let mut out = Vec::new();
    for q in parser.rename_blank_nodes().for_reader(reader) {
        match q {
            Ok(q) => out.push(q),
            Err(e) => match on_error.as_mut() {
                Some(f) => f(e)?,
                None => return Err(e.into()),
            },
        }
    }
    Ok(out)
}

/// Serializes quads (dataset formats) to a writer.
pub fn serialize_quads<W: Write>(
    serializer: RdfSerializer,
    writer: W,
    quads: &[Quad],
) -> Result<W> {
    let mut s = serializer.for_writer(writer);
    for q in quads {
        s.serialize_quad(q)?;
    }
    Ok(s.finish()?)
}

/// Serializes the triples of quads (graph formats) to a writer.
pub fn serialize_triples<W: Write>(
    serializer: RdfSerializer,
    writer: W,
    quads: &[Quad],
) -> Result<W> {
    let mut s = serializer.for_writer(writer);
    for q in quads {
        s.serialize_triple(q.as_ref())?;
    }
    Ok(s.finish()?)
}

pub fn explain_unsupported(e: &Error) -> String {
    format!("-- oxilite: not fully compiled to SQL ({e}); evaluated by the spareval fallback on sync backends")
}
