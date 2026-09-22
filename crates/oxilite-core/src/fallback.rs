//! Fallback evaluation with `spareval` for queries the SQL compiler cannot express.
//!
//! Only available on synchronous backends: `spareval` pulls quads through an iterator
//! interface. Hash ids make `internalize_term` free; `externalize_term` is a cached lookup.
//!
// @lat: [[architecture#SPARQL to SQL compiler#Fallback evaluator]]

use crate::encoding::{
    decode_inline, decode_row, make_triple, tag_of, term_id, Tag, DEFAULT_GRAPH_ID,
};
use crate::error::{Error, Result};
use crate::job::SyncBackend;
use crate::query::QueryOutput;
use crate::sql::{Request, SqlValue, Statement};
use oxrdf::Term;
use spareval::{InternalQuad, QueryEvaluator, QueryResults, QueryableDataset};
use spargebra::Query;
use std::cell::RefCell;
use std::collections::HashMap;

/// A `spareval` dataset over a SQL backend.
pub struct SqlDataset<'a, B: SyncBackend> {
    backend: &'a B,
    cache: RefCell<HashMap<i64, Term>>,
}

impl<'a, B: SyncBackend> SqlDataset<'a, B> {
    pub fn new(backend: &'a B) -> Self {
        Self {
            backend,
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn id_col(&self, c: &str) -> String {
        if self.backend.capabilities().int64_as_text {
            format!("CAST({c} AS TEXT)")
        } else {
            c.into()
        }
    }

    fn rows(&self, sql: String) -> Result<Vec<Vec<SqlValue>>> {
        let mut r = self
            .backend
            .execute(&Request::read(vec![Statement::new(sql)]))?;
        Ok(r.pop().map(|rs| rs.rows).unwrap_or_default())
    }

    pub fn lookup(&self, id: i64) -> Result<Term> {
        if let Some(t) = self.cache.borrow().get(&id) {
            return Ok(t.clone());
        }
        if let Some(t) = decode_inline(id) {
            return Ok(t);
        }
        let t = if tag_of(id) == Some(Tag::Triple) {
            let rows = self.rows(format!(
                "SELECT {}, {}, {} FROM triple_terms WHERE id = {id}",
                self.id_col("s"),
                self.id_col("p"),
                self.id_col("o")
            ))?;
            let row = rows
                .first()
                .ok_or_else(|| Error::corrupted(format!("triple term {id} missing")))?;
            let get = |i: usize| {
                row.get(i)
                    .and_then(SqlValue::as_i64)
                    .ok_or_else(|| Error::corrupted("bad triple term"))
            };
            Term::from(make_triple(
                self.lookup(get(0)?)?,
                self.lookup(get(1)?)?,
                self.lookup(get(2)?)?,
            )?)
        } else {
            let rows = self.rows(format!(
                "SELECT lex, dt, lang, dir FROM terms WHERE id = {id}"
            ))?;
            let mut row = rows
                .into_iter()
                .next()
                .ok_or_else(|| Error::corrupted(format!("term {id} missing")))?
                .into_iter();
            let lex = row
                .next()
                .and_then(SqlValue::into_string)
                .unwrap_or_default();
            let dt = row.next().and_then(SqlValue::into_string);
            let lang = row.next().and_then(SqlValue::into_string);
            let dir = row.next().and_then(|v| v.as_i64());
            decode_row(id, lex, dt, lang, dir)?
        };
        self.cache.borrow_mut().insert(id, t.clone());
        Ok(t)
    }
}

impl<'a, B: SyncBackend> QueryableDataset<'a> for SqlDataset<'a, B> {
    type InternalTerm = i64;
    type Error = Error;

    fn internal_quads_for_pattern(
        &self,
        subject: Option<&i64>,
        predicate: Option<&i64>,
        object: Option<&i64>,
        graph_name: Option<Option<&i64>>,
    ) -> impl Iterator<Item = Result<InternalQuad<i64>>> + use<'a, B> {
        let mut w = Vec::new();
        if let Some(s) = subject {
            w.push(format!("s = {s}"));
        }
        if let Some(p) = predicate {
            w.push(format!("p = {p}"));
        }
        if let Some(o) = object {
            w.push(format!("o = {o}"));
        }
        match graph_name {
            None => w.push(format!("g <> {DEFAULT_GRAPH_ID}")),
            Some(None) => w.push(format!("g = {DEFAULT_GRAPH_ID}")),
            Some(Some(g)) => w.push(format!("g = {g}")),
        }
        let sql = format!(
            "SELECT {}, {}, {}, {} FROM quads WHERE {}",
            self.id_col("s"),
            self.id_col("p"),
            self.id_col("o"),
            self.id_col("g"),
            w.join(" AND ")
        );
        let result: Vec<Result<InternalQuad<i64>>> = match self.rows(sql) {
            Ok(rows) => rows
                .into_iter()
                .map(|row| {
                    let ids: Vec<i64> = row.iter().filter_map(SqlValue::as_i64).collect();
                    let [s, p, o, g] = ids[..] else {
                        return Err(Error::corrupted("bad quad row"));
                    };
                    Ok(InternalQuad {
                        subject: s,
                        predicate: p,
                        object: o,
                        graph_name: (g != DEFAULT_GRAPH_ID).then_some(g),
                    })
                })
                .collect(),
            Err(e) => vec![Err(e)],
        };
        result.into_iter()
    }

    fn internal_named_graphs(&self) -> impl Iterator<Item = Result<i64>> + use<'a, B> {
        let r: Vec<Result<i64>> =
            match self.rows(format!("SELECT {} FROM graphs", self.id_col("id"))) {
                Ok(rows) => rows
                    .into_iter()
                    .filter_map(|r| r.first().and_then(SqlValue::as_i64))
                    .map(Ok)
                    .collect(),
                Err(e) => vec![Err(e)],
            };
        r.into_iter()
    }

    fn contains_internal_graph_name(&self, graph_name: &i64) -> Result<bool> {
        let rows = self.rows(format!(
            "SELECT EXISTS (SELECT 1 FROM graphs WHERE id = {graph_name}) OR EXISTS (SELECT 1 FROM quads WHERE g = {graph_name})"
        ))?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(SqlValue::as_i64)
            == Some(1))
    }

    fn internalize_term(&self, term: Term) -> Result<i64> {
        let id = term_id(term.as_ref());
        self.cache.borrow_mut().entry(id).or_insert(term);
        Ok(id)
    }

    fn externalize_term(&self, term: i64) -> Result<Term> {
        self.lookup(term)
    }
}

/// Applies the dataset options (union default graph, explicit default / named graphs) to a
/// spareval dataset specification.
pub fn apply_dataset_options(
    spec: &mut spareval::QueryDatasetSpecification,
    query: &Query,
    options: &crate::QueryOptions,
    lookup: impl Fn(i64) -> Option<oxrdf::Term>,
) {
    if options.union_default_graph && query_dataset(query).is_none() {
        spec.set_default_graph_as_union();
    }
    let graph = |id: &i64| -> Option<oxrdf::GraphName> {
        if *id == DEFAULT_GRAPH_ID {
            return Some(oxrdf::GraphName::DefaultGraph);
        }
        match lookup(*id)? {
            Term::NamedNode(n) => Some(n.into()),
            Term::BlankNode(b) => Some(b.into()),
            _ => None,
        }
    };
    if let Some(d) = &options.default_graph {
        spec.set_default_graph(d.iter().filter_map(graph).collect());
    }
    if let Some(n) = &options.named_graphs {
        spec.set_available_named_graphs(
            n.iter()
                .filter_map(|id| match graph(id)? {
                    oxrdf::GraphName::NamedNode(n) => Some(n.into()),
                    oxrdf::GraphName::BlankNode(b) => Some(b.into()),
                    oxrdf::GraphName::DefaultGraph => None,
                })
                .collect(),
        );
    }
}

/// Evaluates a query with `spareval` over a sync backend.
pub fn evaluate<B: SyncBackend>(
    backend: &B,
    query: &Query,
    options: &crate::QueryOptions,
) -> Result<QueryOutput> {
    let evaluator = QueryEvaluator::new();
    let mut prepared = evaluator.prepare(query);
    let dataset = SqlDataset::new(backend);
    apply_dataset_options(prepared.dataset_mut(), query, options, |id| {
        dataset.lookup(id).ok()
    });
    Ok(match prepared.execute(dataset)? {
        QueryResults::Solutions(solutions) => {
            let variables = solutions.variables().to_vec();
            let mut rows = Vec::new();
            for s in solutions {
                let s = s?;
                rows.push(variables.iter().map(|v| s.get(v).cloned()).collect());
            }
            QueryOutput::Solutions { variables, rows }
        }
        QueryResults::Boolean(b) => QueryOutput::Boolean(b),
        QueryResults::Graph(triples) => QueryOutput::Graph(triples.collect::<Result<Vec<_>, _>>()?),
    })
}

fn query_dataset(q: &Query) -> Option<&spargebra::algebra::QueryDataset> {
    match q {
        Query::Select { dataset, .. }
        | Query::Construct { dataset, .. }
        | Query::Describe { dataset, .. }
        | Query::Ask { dataset, .. } => dataset.as_ref(),
    }
}

/// Evaluates a `DELETE/INSERT … WHERE` operation with `spareval`, returning the quads to
/// delete and to insert (both computed before any modification).
pub fn delete_insert<B: SyncBackend>(
    backend: &B,
    op: &spargebra::GraphUpdateOperation,
    base_iri: Option<&oxiri::Iri<String>>,
) -> Result<(Vec<oxrdf::Quad>, Vec<oxrdf::Quad>)> {
    let spargebra::GraphUpdateOperation::DeleteInsert {
        delete,
        insert,
        using,
        pattern,
    } = op
    else {
        return Err(Error::Other("not a DELETE/INSERT operation".into()));
    };
    let evaluator = QueryEvaluator::new();
    let prepared = evaluator.prepare_delete_insert(
        delete.clone(),
        insert.clone(),
        base_iri.cloned(),
        using.clone(),
        pattern,
    );
    let mut deletes = Vec::new();
    let mut inserts = Vec::new();
    for q in prepared.execute(SqlDataset::new(backend))? {
        match q? {
            spareval::DeleteInsertQuad::Delete(q) => deletes.push(q),
            spareval::DeleteInsertQuad::Insert(q) => inserts.push(q),
        }
    }
    Ok((deletes, inserts))
}
