//! Bounded prefetch for stores that cannot block (Cloudflare D1).
//!
//! The shapes decide what is loaded: SHACL targets (target classes with their subclasses,
//! target nodes, subjects and objects of target predicates, implicit class targets), then a
//! breadth-first neighbourhood of those nodes (outgoing triples, and incoming ones for
//! predicates used in inverse paths) to a configured depth, plus the class hierarchy. Each
//! hop is one request per chunk of nodes, and the load fails with [`Error::TooLarge`] rather
//! than truncating when the subgraph grows beyond the limit.

use crate::{shacl_schema, shex_schema, validate_shacl_graph, validate_shex_graph, Error, Result};
use crate::{ResultShapeMap, ShaclValidationMode, ValidationReport};
use oxilite::sparql::QueryOptions;
use oxilite::AsyncStore;
use oxilite_core::{ops, run_async, AsyncBackend, QueryOutput};
use oxrdf::vocab::{rdf, rdfs};
use oxrdf::{GraphNameRef, NamedNode, NamedNodeRef, Term, TermRef, Triple};
use rudof_rdf::rdf_core::{Any, NeighsRDF, RDFFormat};
use rudof_rdf::rdf_impl::{OxigraphInMemory, ReaderMode};
use std::collections::{BTreeSet, HashSet};

const SH: &str = "http://www.w3.org/ns/shacl#";

/// Limits of a prefetch.
#[derive(Debug, Clone)]
pub struct PrefetchOptions {
    /// Maximum number of triples loaded (the load fails beyond it).
    pub max_triples: usize,
    /// Neighbourhood hops from the focus nodes (nested shapes and paths need one per level).
    pub max_depth: usize,
    /// Nodes per request.
    pub chunk: usize,
    /// Read every graph merged instead of the default graph.
    pub union: bool,
}

impl Default for PrefetchOptions {
    fn default() -> Self {
        Self {
            max_triples: 100_000,
            max_depth: 3,
            chunk: 400,
            union: false,
        }
    }
}

fn sh(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{SH}{local}"))
}

/// What a shapes graph needs from the data.
#[derive(Debug, Default)]
struct Needs {
    classes: BTreeSet<NamedNode>,
    nodes: Vec<Term>,
    subjects_of: BTreeSet<NamedNode>,
    objects_of: BTreeSet<NamedNode>,
    inverse: BTreeSet<NamedNode>,
}

fn analyze(shapes: &OxigraphInMemory) -> Result<Needs> {
    let objects = |p: NamedNode| -> Result<Vec<Term>> {
        Ok(shapes
            .triples_matching(&Any, &p, &Any)
            .map_err(|e| Error::Shapes(e.to_string()))?
            .map(|t| t.object)
            .collect())
    };
    let iri = |t: Term| match t {
        Term::NamedNode(n) => Some(n),
        _ => None,
    };
    let mut needs = Needs::default();
    needs
        .classes
        .extend(objects(sh("targetClass"))?.into_iter().filter_map(iri));
    needs.nodes.extend(objects(sh("targetNode"))?);
    needs
        .subjects_of
        .extend(objects(sh("targetSubjectsOf"))?.into_iter().filter_map(iri));
    needs
        .objects_of
        .extend(objects(sh("targetObjectsOf"))?.into_iter().filter_map(iri));
    needs
        .inverse
        .extend(objects(sh("inversePath"))?.into_iter().filter_map(iri));
    // Implicit class targets: shapes that are also classes.
    let ty: NamedNode = rdf::TYPE.into_owned();
    for class in [
        rdfs::CLASS.into_owned(),
        NamedNode::new_unchecked("http://www.w3.org/2002/07/owl#Class"),
    ] {
        for t in shapes
            .triples_matching(&Any, &ty, &Term::from(class))
            .map_err(|e| Error::Shapes(e.to_string()))?
        {
            if let oxrdf::NamedOrBlankNode::NamedNode(n) = t.subject {
                needs.classes.insert(n);
            }
        }
    }
    Ok(needs)
}

/// Loads the subgraph the shapes need into rudof's in-memory graph.
async fn load<B: AsyncBackend>(
    store: &AsyncStore<B>,
    seeds: Vec<Term>,
    needs: &Needs,
    opts: &PrefetchOptions,
) -> Result<OxigraphInMemory> {
    let caps = store.backend().capabilities().clone();
    let query_opts = QueryOptions {
        union_default_graph: opts.union,
        ..QueryOptions::default()
    };
    let mut triples: HashSet<Triple> = HashSet::new();
    let add = |ts: Vec<oxrdf::Quad>, triples: &mut HashSet<Triple>| -> Result<Vec<Term>> {
        let mut fresh = Vec::new();
        for q in ts {
            let t = Triple::from(q);
            if triples.insert(t.clone()) {
                fresh.push(t.subject.clone().into());
                fresh.push(t.object.clone());
            }
            if triples.len() > opts.max_triples {
                return Err(Error::TooLarge {
                    limit: opts.max_triples,
                    found: triples.len(),
                });
            }
        }
        Ok(fresh)
    };

    // Focus nodes.
    let mut focus: Vec<Term> = seeds;
    focus.extend(needs.nodes.iter().cloned());
    let query_opts = &query_opts;
    let select = |q: String| async move {
        match store.query_output(q.as_str(), query_opts).await? {
            QueryOutput::Solutions { rows, .. } => Ok::<_, Error>(
                rows.into_iter()
                    .filter_map(|r| r.into_iter().next().flatten())
                    .collect::<Vec<_>>(),
            ),
            _ => Ok(Vec::new()),
        }
    };
    for c in &needs.classes {
        focus.extend(
            select(format!(
                "SELECT DISTINCT ?x WHERE {{ ?x a/<{}>* {c} }}",
                rdfs::SUB_CLASS_OF.as_str()
            ))
            .await?,
        );
    }
    // Triples of target predicates: rudof finds those targets through them.
    let graph = (!opts.union).then_some(GraphNameRef::DefaultGraph);
    for (preds, subjects) in [(&needs.subjects_of, true), (&needs.objects_of, false)] {
        for p in preds {
            let quads = run_async(
                store.backend(),
                ops::scan_job(None, Some(p.as_ref()), None, graph, &caps),
            )
            .await?;
            for q in &quads {
                focus.push(if subjects {
                    q.subject.clone().into()
                } else {
                    q.object.clone()
                });
            }
            add(quads, &mut triples)?;
        }
    }

    // The class hierarchy (for sh:class through subclasses).
    let hierarchy = run_async(
        store.backend(),
        ops::scan_job(None, Some(rdfs::SUB_CLASS_OF), None, graph, &caps),
    )
    .await?;
    add(hierarchy, &mut triples)?;

    // Breadth-first neighbourhood.
    let inverse: Vec<NamedNodeRef<'_>> = needs.inverse.iter().map(NamedNode::as_ref).collect();
    let mut seen: HashSet<Term> = HashSet::new();
    let mut frontier: Vec<Term> = focus
        .into_iter()
        .filter(|t| !t.is_literal() && seen.insert(t.clone()))
        .collect();
    for _ in 0..=opts.max_depth {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for chunk in frontier.chunks(opts.chunk.max(1)) {
            let refs: Vec<TermRef<'_>> = chunk.iter().map(Term::as_ref).collect();
            let out = run_async(
                store.backend(),
                ops::neighbourhood_job(&refs, false, None, !opts.union, &caps),
            )
            .await?;
            next.extend(add(out, &mut triples)?);
            if !inverse.is_empty() {
                let inc = run_async(
                    store.backend(),
                    ops::neighbourhood_job(&refs, true, Some(&inverse), !opts.union, &caps),
                )
                .await?;
                next.extend(add(inc, &mut triples)?);
            }
        }
        frontier = next
            .into_iter()
            .filter(|t| !t.is_literal() && seen.insert(t.clone()))
            .collect();
    }

    let mut graph = OxigraphInMemory::new();
    let mut sorted: Vec<&Triple> = triples.iter().collect();
    sorted.sort_by_cached_key(|t| t.to_string());
    for t in sorted {
        graph
            .add_triple_ref(t.subject.as_ref(), t.predicate.as_ref(), t.object.as_ref())
            .map_err(|e| Error::Validation(e.to_string()))?;
    }
    Ok(graph)
}

/// Loads the subgraph a SHACL shapes graph (Turtle) needs, bounded by `opts`.
pub async fn prefetch_for_shacl<B: AsyncBackend>(
    store: &AsyncStore<B>,
    shapes_turtle: &str,
    opts: &PrefetchOptions,
) -> Result<OxigraphInMemory> {
    let shapes =
        OxigraphInMemory::from_str(shapes_turtle, &RDFFormat::Turtle, None, &ReaderMode::Strict)
            .map_err(|e| Error::Shapes(e.to_string()))?;
    let needs = analyze(&shapes)?;
    load(store, Vec::new(), &needs, opts).await
}

/// Loads the neighbourhood of `nodes` (for ShEx shape maps), bounded by `opts`.
pub async fn prefetch_nodes<B: AsyncBackend>(
    store: &AsyncStore<B>,
    nodes: &[Term],
    opts: &PrefetchOptions,
) -> Result<OxigraphInMemory> {
    load(store, nodes.to_vec(), &Needs::default(), opts).await
}

/// SHACL validation of an async store (D1): prefetch, then rudof over the in-memory graph.
pub async fn validate_shacl_async<B: AsyncBackend>(
    store: &AsyncStore<B>,
    shapes_turtle: &str,
    mode: &ShaclValidationMode,
    opts: &PrefetchOptions,
) -> Result<ValidationReport> {
    let schema = shacl_schema(shapes_turtle, &RDFFormat::Turtle, None)?;
    let mut graph = prefetch_for_shacl(store, shapes_turtle, opts).await?;
    graph
        .ensure_store()
        .map_err(|e| Error::Validation(e.to_string()))?;
    validate_shacl_graph(graph, &schema, mode)
}

/// ShEx validation of an async store (D1): the shape map's `focus` nodes and their
/// neighbourhood are prefetched, then validated in memory.
pub async fn validate_shex_async<B: AsyncBackend>(
    store: &AsyncStore<B>,
    shexc: &str,
    base: &str,
    shape_map: &str,
    focus: &[Term],
    opts: &PrefetchOptions,
) -> Result<ResultShapeMap> {
    let schema = shex_schema(shexc, base)?;
    let mut graph = prefetch_nodes(store, focus, opts).await?;
    graph
        .ensure_store()
        .map_err(|e| Error::Validation(e.to_string()))?;
    validate_shex_graph(&graph, &schema, shape_map)
}
