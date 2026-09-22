//! rudof's RDF traits over an oxilite store.

use oxilite::sparql::QueryOptions;
use oxilite::store::Store;
use oxilite_core::{QueryOutput, SyncBackend};
use oxrdf::{
    BlankNode, GraphNameRef, Literal, NamedNode, NamedNodeRef, NamedOrBlankNode,
    NamedOrBlankNodeRef, Term, TermRef, Triple,
};
use prefixmap::{PrefixMap, PrefixMapError};
use rudof_iri::IriS;
use rudof_rdf::rdf_core::query::{
    QueryRDF, QueryResultFormat, QuerySolution, QuerySolutions, VarName,
};
use rudof_rdf::rdf_core::{Matcher, NeighsRDF, Rdf};
use std::collections::HashSet;
use std::fmt::{self, Debug, Display};
use std::str::FromStr;

/// An oxilite store seen as a rudof graph: its default graph, or every graph merged.
pub struct StoreGraph<B: SyncBackend + Send + Sync + 'static> {
    store: Store<B>,
    union: bool,
    pm: PrefixMap,
}

impl<B: SyncBackend + Send + Sync + 'static> StoreGraph<B> {
    /// The default graph of `store`.
    pub fn new(store: Store<B>) -> Self {
        Self {
            store,
            union: false,
            pm: PrefixMap::new(),
        }
    }

    /// Reads the union of all graphs instead of the default graph.
    pub fn with_union(mut self, union: bool) -> Self {
        self.union = union;
        self
    }

    /// Prefixes used to show nodes in reports.
    pub fn with_prefixmap(mut self, pm: PrefixMap) -> Self {
        self.pm = pm;
        self
    }

    fn options(&self) -> QueryOptions {
        QueryOptions {
            union_default_graph: self.union,
            ..QueryOptions::default()
        }
    }
}

impl<B: SyncBackend + Send + Sync + 'static> Debug for StoreGraph<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreGraph")
            .field("union", &self.union)
            .finish()
    }
}

/// Store errors, as rudof's graph error type.
#[derive(Debug)]
pub struct GraphError(pub String);

impl Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GraphError {}

impl From<oxilite_core::Error> for GraphError {
    fn from(e: oxilite_core::Error) -> Self {
        Self(e.to_string())
    }
}

impl<B: SyncBackend + Send + Sync + 'static> Rdf for StoreGraph<B> {
    type IRI = NamedNode;
    type BNode = BlankNode;
    type Literal = Literal;
    type Subject = NamedOrBlankNode;
    type Term = Term;
    type Triple = Triple;
    type Err = GraphError;

    fn resolve_prefix_local(&self, prefix: &str, local: &str) -> Result<IriS, PrefixMapError> {
        self.pm.resolve_prefix_local(prefix, local)
    }

    fn qualify_iri(&self, node: &NamedNode) -> String {
        match IriS::from_str(node.as_str()) {
            Ok(iri) => self.pm.qualify(&iri),
            Err(_) => node.to_string(),
        }
    }

    fn qualify_subject(&self, subj: &NamedOrBlankNode) -> String {
        match subj {
            NamedOrBlankNode::NamedNode(n) => self.qualify_iri(n),
            NamedOrBlankNode::BlankNode(b) => b.to_string(),
        }
    }

    fn qualify_term(&self, term: &Term) -> String {
        match term {
            Term::NamedNode(n) => self.qualify_iri(n),
            t => t.to_string(),
        }
    }

    fn prefixmap(&self) -> Option<PrefixMap> {
        Some(self.pm.clone())
    }
}

impl<B: SyncBackend + Send + Sync + 'static> NeighsRDF for StoreGraph<B> {
    fn triples(&self) -> Result<impl Iterator<Item = Triple>, GraphError> {
        self.scan(None, None, None)
    }

    fn triples_matching<S, P, O>(
        &self,
        subject: &S,
        predicate: &P,
        object: &O,
    ) -> Result<impl Iterator<Item = Triple> + '_, GraphError>
    where
        S: Matcher<NamedOrBlankNode>,
        P: Matcher<NamedNode>,
        O: Matcher<Term>,
    {
        self.scan(
            subject.value().map(NamedOrBlankNode::as_ref),
            predicate.value().map(NamedNode::as_ref),
            object.value().map(Term::as_ref),
        )
    }
}

impl<B: SyncBackend + Send + Sync + 'static> StoreGraph<B> {
    fn scan(
        &self,
        s: Option<NamedOrBlankNodeRef<'_>>,
        p: Option<NamedNodeRef<'_>>,
        o: Option<TermRef<'_>>,
    ) -> Result<std::vec::IntoIter<Triple>, GraphError> {
        let graph = (!self.union).then_some(GraphNameRef::DefaultGraph);
        let mut triples = Vec::new();
        let mut seen = HashSet::new();
        for q in self.store.quads_for_pattern(s, p, o, graph) {
            let t = Triple::from(q?);
            if !self.union || seen.insert(t.clone()) {
                triples.push(t);
            }
        }
        Ok(triples.into_iter())
    }

    fn output(&self, query: &str) -> Result<QueryOutput, GraphError> {
        Ok(self.store.query_output(query, &self.options())?)
    }
}

impl<B: SyncBackend + Send + Sync + 'static> QueryRDF for StoreGraph<B> {
    fn query_select(&self, query: &str) -> Result<QuerySolutions<Self>, GraphError> {
        let QueryOutput::Solutions { variables, rows } = self.output(query)? else {
            return Ok(QuerySolutions::empty());
        };
        let names: Vec<VarName> = variables.iter().map(|v| VarName::new(v.as_str())).collect();
        let solutions = rows
            .into_iter()
            .map(|row| QuerySolution::new(names.clone(), row))
            .collect();
        Ok(QuerySolutions::new(solutions, self.pm.clone()))
    }

    fn query_construct(
        &self,
        query: &str,
        _format: &QueryResultFormat,
    ) -> Result<String, GraphError> {
        let QueryOutput::Graph(triples) = self.output(query)? else {
            return Ok(String::new());
        };
        let mut s =
            oxrdfio::RdfSerializer::from_format(oxrdfio::RdfFormat::Turtle).for_writer(Vec::new());
        for t in &triples {
            s.serialize_triple(t)
                .map_err(|e| GraphError(e.to_string()))?;
        }
        let bytes = s.finish().map_err(|e| GraphError(e.to_string()))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn query_ask(&self, query: &str) -> Result<bool, GraphError> {
        Ok(matches!(self.output(query)?, QueryOutput::Boolean(true)))
    }
}
