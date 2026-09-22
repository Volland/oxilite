//! Partial fallback: when a query cannot be compiled as a whole, its largest compilable
//! subtrees still run as SQL. They are rewritten into `SERVICE <urn:oxilite:sql:N>` calls
//! that spareval evaluates through a handler running the precompiled SQL; only the
//! operators above them are evaluated in Rust.
//!
// @lat: [[architecture#SPARQL to SQL compiler#Fallback evaluator]]

use oxilite_core::job::run_sync;
use oxilite_core::query::{compile_query, CompiledQuery, QueryJob, QueryOutput};
use oxilite_core::{Capabilities, Error, QueryOptions, Result, Stats, SyncBackend};
use oxiri::Iri;
use oxrdf::{NamedNode, Variable};
use spareval::{
    DefaultServiceHandler, QueryEvaluationError, QueryEvaluator, QueryResults, QuerySolutionIter,
};
use spargebra::algebra::{GraphPattern, QueryDataset};
use spargebra::term::NamedNodePattern;
use spargebra::Query;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const PREFIX: &str = "urn:oxilite:sql:";

struct Rewriter<'a> {
    stats: &'a Stats,
    caps: &'a Capabilities,
    options: &'a QueryOptions,
    dataset: Option<&'a QueryDataset>,
    base: Option<Iri<String>>,
    jobs: HashMap<String, CompiledQuery>,
}

impl Rewriter<'_> {
    fn try_compile(&mut self, p: &GraphPattern) -> Option<CompiledQuery> {
        let mut vars: Vec<Variable> = Vec::new();
        p.on_in_scope_variable(|v| {
            if !vars.contains(v) {
                vars.push(v.clone());
            }
        });
        let q = Query::Select {
            dataset: self.dataset.cloned(),
            pattern: GraphPattern::Project {
                inner: Box::new(p.clone()),
                variables: vars,
            },
            base_iri: self.base.clone(),
        };
        compile_query(&q, self.stats, self.caps, self.options).ok()
    }

    fn service(&mut self, p: &GraphPattern, c: CompiledQuery) -> GraphPattern {
        let name = format!("{PREFIX}{}", self.jobs.len());
        self.jobs.insert(name.clone(), c);
        GraphPattern::Service {
            name: NamedNodePattern::NamedNode(NamedNode::new_unchecked(name)),
            inner: Box::new(p.clone()),
            silent: false,
        }
    }

    /// Rewrites maximal compilable subtrees. `replace = false` below GRAPH (a SERVICE would
    /// lose the active graph).
    fn rewrite(&mut self, p: &GraphPattern, replace: bool) -> GraphPattern {
        if replace && !matches!(p, GraphPattern::Values { .. } | GraphPattern::Bgp { .. }) {
            if let Some(c) = self.try_compile(p) {
                return self.service(p, c);
            }
        }
        if replace {
            if let GraphPattern::Bgp { patterns } = p {
                if !patterns.is_empty() {
                    if let Some(c) = self.try_compile(p) {
                        return self.service(p, c);
                    }
                }
            }
        }
        let r = |me: &mut Self, x: &GraphPattern| Box::new(me.rewrite(x, replace));
        match p {
            GraphPattern::Join { left, right } => GraphPattern::Join {
                left: r(self, left),
                right: r(self, right),
            },
            GraphPattern::LeftJoin {
                left,
                right,
                expression,
            } => GraphPattern::LeftJoin {
                left: r(self, left),
                right: r(self, right),
                expression: expression.clone(),
            },
            GraphPattern::Union { left, right } => GraphPattern::Union {
                left: r(self, left),
                right: r(self, right),
            },
            GraphPattern::Minus { left, right } => GraphPattern::Minus {
                left: r(self, left),
                right: r(self, right),
            },
            GraphPattern::Filter { expr, inner } => GraphPattern::Filter {
                expr: expr.clone(),
                inner: r(self, inner),
            },
            GraphPattern::Extend {
                inner,
                variable,
                expression,
            } => GraphPattern::Extend {
                inner: r(self, inner),
                variable: variable.clone(),
                expression: expression.clone(),
            },
            GraphPattern::OrderBy { inner, expression } => GraphPattern::OrderBy {
                inner: r(self, inner),
                expression: expression.clone(),
            },
            GraphPattern::Project { inner, variables } => GraphPattern::Project {
                inner: r(self, inner),
                variables: variables.clone(),
            },
            GraphPattern::Distinct { inner } => GraphPattern::Distinct {
                inner: r(self, inner),
            },
            GraphPattern::Reduced { inner } => GraphPattern::Reduced {
                inner: r(self, inner),
            },
            GraphPattern::Slice {
                inner,
                start,
                length,
            } => GraphPattern::Slice {
                inner: r(self, inner),
                start: *start,
                length: *length,
            },
            GraphPattern::Group {
                inner,
                variables,
                aggregates,
            } => GraphPattern::Group {
                inner: r(self, inner),
                variables: variables.clone(),
                aggregates: aggregates.clone(),
            },
            GraphPattern::Graph { name, inner } => GraphPattern::Graph {
                name: name.clone(),
                inner: Box::new(self.rewrite(inner, false)),
            },
            other => other.clone(),
        }
    }
}

/// Runs precompiled SQL subqueries for the `urn:oxilite:sql:N` services (memoized: they do
/// not depend on outer bindings).
struct SqlServices<B> {
    backend: Arc<B>,
    caps: Capabilities,
    jobs: HashMap<String, CompiledQuery>,
    cache: Mutex<HashMap<String, Arc<(Vec<Variable>, Vec<Vec<Option<oxrdf::Term>>>)>>>,
}

impl<B: SyncBackend + Send + Sync + 'static> DefaultServiceHandler for SqlServices<B> {
    type Error = QueryEvaluationError;

    fn handle(
        &self,
        service_name: &NamedNode,
        _pattern: &GraphPattern,
        _base_iri: Option<&Iri<String>>,
    ) -> Result<QuerySolutionIter<'static>, QueryEvaluationError> {
        let name = service_name.as_str();
        let err = |e: Error| QueryEvaluationError::Service(Box::new(e));
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned();
        let result = match cached {
            Some(r) => r,
            None => {
                let job = self
                    .jobs
                    .get(name)
                    .ok_or_else(|| err(Error::Other(format!("unknown service {name}"))))?;
                let out = run_sync(
                    &*self.backend,
                    QueryJob::new(job.clone(), self.caps.clone()),
                )
                .map_err(err)?;
                let QueryOutput::Solutions { variables, rows } = out else {
                    return Err(err(Error::Other(
                        "SQL service did not return solutions".into(),
                    )));
                };
                let r = Arc::new((variables, rows));
                self.cache
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(name.to_string(), Arc::clone(&r));
                r
            }
        };
        let variables: Arc<[Variable]> = result.0.clone().into();
        Ok(QuerySolutionIter::from_tuples(
            variables,
            result.1.clone().into_iter().map(Ok),
        ))
    }
}

fn parts(q: &Query) -> (&GraphPattern, Option<&QueryDataset>, Option<&Iri<String>>) {
    match q {
        Query::Select {
            pattern,
            dataset,
            base_iri,
        }
        | Query::Construct {
            pattern,
            dataset,
            base_iri,
            ..
        }
        | Query::Describe {
            pattern,
            dataset,
            base_iri,
        }
        | Query::Ask {
            pattern,
            dataset,
            base_iri,
        } => (pattern, dataset.as_ref(), base_iri.as_ref()),
    }
}

fn with_pattern(q: &Query, pattern: GraphPattern) -> Query {
    let mut q = q.clone();
    match &mut q {
        Query::Select { pattern: p, .. }
        | Query::Construct { pattern: p, .. }
        | Query::Describe { pattern: p, .. }
        | Query::Ask { pattern: p, .. } => *p = pattern,
    }
    q
}

/// The query with compilable subtrees replaced by SQL services, and the number of them.
pub(crate) fn plan(
    query: &Query,
    stats: &Stats,
    caps: &Capabilities,
    options: &QueryOptions,
) -> (Query, HashMap<String, CompiledQuery>) {
    let (pattern, dataset, base) = parts(query);
    let mut rw = Rewriter {
        stats,
        caps,
        options,
        dataset,
        base: base.cloned(),
        jobs: HashMap::new(),
    };
    let rewritten = rw.rewrite(pattern, true);
    (with_pattern(query, rewritten), rw.jobs)
}

/// Evaluates a query with spareval, delegating compilable subtrees to SQL.
pub(crate) fn evaluate<B: SyncBackend + Send + Sync + 'static>(
    backend: Arc<B>,
    query: &Query,
    stats: &Stats,
    options: &QueryOptions,
) -> Result<QueryOutput> {
    let caps = backend.capabilities().clone();
    let (rewritten, jobs) = plan(query, stats, &caps, options);
    if jobs.is_empty() {
        return oxilite_core::fallback::evaluate(&*backend, query, options);
    }
    let handler = SqlServices {
        backend: Arc::clone(&backend),
        caps,
        jobs,
        cache: Mutex::new(HashMap::new()),
    };
    let evaluator = QueryEvaluator::new()
        .with_default_service_handler(handler)
        .with_custom_function(
            oxilite_core::text::text_match_name(),
            oxilite_core::text::text_match,
        );
    let mut prepared = evaluator.prepare(&rewritten);
    let dataset = oxilite_core::fallback::SqlDataset::new(&*backend).with_options(options)?;
    oxilite_core::fallback::apply_dataset_options(prepared.dataset_mut(), query, options, |id| {
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
