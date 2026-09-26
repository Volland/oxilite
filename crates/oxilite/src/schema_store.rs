//! The schema registry on the stores: declaring a graph to be an ontology or a shapes graph,
//! and which graphs it applies to.
//!
//! The registry is RDF in the system graph `<oxilite:schema>` (see
//! `oxilite_core::registry`). Every operation here is a SPARQL update or query built by the
//! core and run through the store's ordinary update path — atomic, versioned when versioning
//! is on, and rebuilding the reasoning closure and the shape index in the same request. The
//! same SPARQL runs on Oxigraph.
//!
// @lat: [[architecture#Schema registry]]

use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::registry::{self, SchemaGraph, SchemaRole};
use oxilite_core::shapes::ShapeIndex;
use oxilite_core::{AsyncBackend, Error, QueryOptions, QueryOutput, Result, SyncBackend};
use oxrdf::{GraphName, GraphNameRef, NamedNode, Term};

/// What is recorded about a registered graph, besides its name and role.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registration {
    /// The `owl:Ontology` IRI, when it differs from the graph name.
    pub iri: Option<NamedNode>,
    /// `owl:versionIRI`, a version string, or anything else worth pinning.
    pub version: Option<String>,
    /// Digest of the document the graph was loaded from, for drift detection.
    pub sha256: Option<String>,
    /// `owl:imports` targets. Imports naming another active registered ontology bring its
    /// axioms into this one's reasoning scopes.
    pub imports: Vec<NamedNode>,
    /// The graphs the schema applies to (`oxl:appliesTo`); empty means every graph.
    pub applies_to: Vec<GraphName>,
    /// An inactive graph stays registered (and hidden) but stops contributing.
    pub active: bool,
}

impl Registration {
    /// An active registration applying to every graph, with nothing else recorded.
    pub fn new() -> Self {
        Self {
            active: true,
            ..Default::default()
        }
    }

    /// Records the ontology IRI.
    pub fn with_iri(mut self, iri: NamedNode) -> Self {
        self.iri = Some(iri);
        self
    }

    /// Records the version.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Records the digest of the source document.
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }

    /// Records `owl:imports` targets.
    pub fn with_imports(mut self, imports: impl IntoIterator<Item = NamedNode>) -> Self {
        self.imports = imports.into_iter().collect();
        self
    }

    /// Applies the schema to these graphs only (instead of every graph).
    pub fn applies_to(mut self, graphs: impl IntoIterator<Item = GraphName>) -> Self {
        self.applies_to = graphs.into_iter().collect();
        self
    }

    /// Registers the graph inactive.
    pub fn inactive(mut self) -> Self {
        self.active = false;
        self
    }
}

/// A registry entry.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredGraph {
    pub graph: GraphName,
    pub role: SchemaRole,
    pub registration: Registration,
    /// When the graph was registered (`xsd:dateTime` lexical form).
    pub loaded_at: Option<String>,
}

impl RegisteredGraph {
    /// The entry in the core's form (e.g. for `oxilite_core::json`).
    pub fn to_entry(&self) -> SchemaGraph {
        let mut e = entry(self.graph.as_ref(), self.role, &self.registration);
        e.loaded_at = self.loaded_at.clone();
        e
    }

    /// Does this schema apply to `target`?
    pub fn applies(&self, target: GraphNameRef<'_>) -> bool {
        let a = &self.registration.applies_to;
        a.is_empty() || a.iter().any(|g| g.as_ref() == target)
    }
}

fn entry(graph: GraphNameRef<'_>, role: SchemaRole, r: &Registration) -> SchemaGraph {
    SchemaGraph {
        graph: graph.into_owned(),
        role,
        iri: r.iri.clone(),
        version: r.version.clone(),
        sha256: r.sha256.clone(),
        imports: r.imports.clone(),
        applies_to: r.applies_to.clone(),
        active: r.active,
        loaded_at: None,
    }
}

fn from_entry(e: SchemaGraph) -> RegisteredGraph {
    RegisteredGraph {
        graph: e.graph,
        role: e.role,
        registration: Registration {
            iri: e.iri,
            version: e.version,
            sha256: e.sha256,
            imports: e.imports,
            applies_to: e.applies_to,
            active: e.active,
        },
        loaded_at: e.loaded_at,
    }
}

fn entries(out: QueryOutput) -> Vec<RegisteredGraph> {
    match out {
        QueryOutput::Solutions { rows, .. } => registry::entries_from_rows(&rows)
            .into_iter()
            .map(from_entry)
            .collect(),
        _ => Vec::new(),
    }
}

fn problems(out: QueryOutput) -> Vec<String> {
    match out {
        QueryOutput::Solutions { rows, .. } => registry::problems(&rows),
        _ => Vec::new(),
    }
}

fn boolean(out: QueryOutput) -> bool {
    matches!(out, QueryOutput::Boolean(true))
}

fn count(out: QueryOutput) -> u64 {
    match out {
        QueryOutput::Solutions { rows, .. } => rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|t| match t {
                Term::Literal(l) => l.value().parse().ok(),
                _ => None,
            })
            .unwrap_or(0),
        _ => 0,
    }
}

fn applying(
    all: Vec<RegisteredGraph>,
    target: GraphNameRef<'_>,
    role: SchemaRole,
) -> Vec<GraphName> {
    all.into_iter()
        .filter(|e| e.role == role && e.registration.active && e.applies(target))
        .map(|e| e.graph)
        .collect()
}

fn parse_error(e: impl std::fmt::Display) -> Error {
    Error::Other(e.to_string())
}

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    fn registry_query(&self, sparql: &str) -> Result<QueryOutput> {
        let q = spargebra::SparqlParser::new()
            .parse_query(sparql)
            .map_err(parse_error)?;
        self.query_output(q, &QueryOptions::default())
    }

    fn registry_update(&self, sparql: &str) -> Result<()> {
        let u = spargebra::SparqlParser::new()
            .parse_update(sparql)
            .map_err(parse_error)?;
        self.update(u)
    }

    /// Declares a graph to hold an ontology, SHACL shapes or a ShEx schema, and which graphs
    /// it applies to.
    ///
    /// The graph's triples are untouched and stay queryable. Registering the first ontology
    /// graph narrows reasoning to the registered ones; the same holds for shapes and the
    /// compiled shape index. Registering a graph again replaces its description.
    pub fn register_schema_graph<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        role: SchemaRole,
        registration: &Registration,
    ) -> Result<()> {
        let e = entry(graph.into(), role, registration);
        self.registry_update(&registry::register_update(&e)?)
    }

    /// The registry, ordered by role and graph.
    pub fn schema_graphs(&self) -> Result<Vec<RegisteredGraph>> {
        Ok(entries(self.registry_query(&registry::entries_query())?))
    }

    /// The active schema graphs of `role` that apply to `target`.
    pub fn schema_graphs_for<'a>(
        &self,
        target: impl Into<GraphNameRef<'a>>,
        role: SchemaRole,
    ) -> Result<Vec<GraphName>> {
        Ok(applying(self.schema_graphs()?, target.into(), role))
    }

    fn is_registered(&self, graph: GraphNameRef<'_>) -> Result<bool> {
        Ok(boolean(
            self.registry_query(&registry::registered_query(graph)?)?,
        ))
    }

    /// Sets the graphs a registration applies to (empty: every graph), keeping the rest of its
    /// description; returns whether one was found.
    pub fn set_schema_graph_targets<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        applies_to: &[GraphName],
    ) -> Result<bool> {
        let graph = graph.into();
        if !self.is_registered(graph)? {
            return Ok(false);
        }
        self.registry_update(&registry::remap_update(graph, applies_to)?)?;
        Ok(true)
    }

    /// What is wrong with the registry: the violations of its SHACL shapes
    /// (`oxl:RegistrationShape`), one message each. Empty means valid.
    pub fn registry_problems(&self) -> Result<Vec<String>> {
        Ok(problems(self.registry_query(&registry::entries_query())?))
    }

    /// Activates or deactivates a registration; returns whether one was found.
    pub fn set_schema_graph_active<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        active: bool,
    ) -> Result<bool> {
        let graph = graph.into();
        if !self.is_registered(graph)? {
            return Ok(false);
        }
        self.registry_update(&registry::set_active_update(graph, active)?)?;
        Ok(true)
    }

    /// Removes a registration, leaving the graph's triples in place; returns whether one was
    /// found.
    pub fn unregister_schema_graph<'a>(&self, graph: impl Into<GraphNameRef<'a>>) -> Result<bool> {
        let graph = graph.into();
        if !self.is_registered(graph)? {
            return Ok(false);
        }
        self.registry_update(&registry::unregister_update(graph)?)?;
        Ok(true)
    }

    /// Removes a registration together with every quad of its graph, in one atomic update;
    /// returns how many quads the graph held.
    pub fn drop_schema_graph<'a>(&self, graph: impl Into<GraphNameRef<'a>>) -> Result<u64> {
        let graph = graph.into();
        let n = count(self.registry_query(&registry::size_query(graph))?);
        self.registry_update(&registry::drop_update(graph)?)?;
        Ok(n)
    }

    /// Are the system graphs (`<oxilite:vocabulary>` and the registry's own description)
    /// installed at the current vocabulary version?
    pub fn system_graphs_installed(&self) -> Result<bool> {
        Ok(boolean(
            self.registry_query(&registry::system_graphs_ready_query())?,
        ))
    }

    /// Installs or refreshes the system graphs (what `StoreOptions::system_graphs` does for a
    /// blank store); registrations are untouched. Returns `false` when they were already
    /// current.
    pub fn install_system_graphs(&self) -> Result<bool> {
        if self.system_graphs_installed()? {
            return Ok(false);
        }
        self.registry_update(&registry::system_graphs_update())?;
        Ok(true)
    }

    /// The compiled SHACL property shapes of the registered shapes graphs (of every graph
    /// while none is registered). One request, no SPARQL evaluation.
    pub fn shape_index(&self) -> Result<ShapeIndex> {
        self.run(oxilite_core::ops::shape_index_job(self.caps()))
    }
}

impl<B: AsyncBackend> AsyncStore<B> {
    async fn registry_query(&self, sparql: &str) -> Result<QueryOutput> {
        let q = spargebra::SparqlParser::new()
            .parse_query(sparql)
            .map_err(parse_error)?;
        self.query_output(q, &QueryOptions::default()).await
    }

    async fn registry_update(&self, sparql: &str) -> Result<()> {
        let u = spargebra::SparqlParser::new()
            .parse_update(sparql)
            .map_err(parse_error)?;
        self.update(u).await
    }

    /// Declares a graph to hold an ontology, SHACL shapes or a ShEx schema, and which graphs
    /// it applies to.
    pub async fn register_schema_graph<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        role: SchemaRole,
        registration: &Registration,
    ) -> Result<()> {
        let e = entry(graph.into(), role, registration);
        self.registry_update(&registry::register_update(&e)?).await
    }

    /// The registry, ordered by role and graph.
    pub async fn schema_graphs(&self) -> Result<Vec<RegisteredGraph>> {
        Ok(entries(
            self.registry_query(&registry::entries_query()).await?,
        ))
    }

    /// The active schema graphs of `role` that apply to `target`.
    pub async fn schema_graphs_for<'a>(
        &self,
        target: impl Into<GraphNameRef<'a>>,
        role: SchemaRole,
    ) -> Result<Vec<GraphName>> {
        let target = target.into().into_owned();
        Ok(applying(self.schema_graphs().await?, target.as_ref(), role))
    }

    async fn is_registered(&self, graph: GraphNameRef<'_>) -> Result<bool> {
        Ok(boolean(
            self.registry_query(&registry::registered_query(graph)?)
                .await?,
        ))
    }

    /// Sets the graphs a registration applies to (empty: every graph), keeping the rest of its
    /// description; returns whether one was found.
    pub async fn set_schema_graph_targets<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        applies_to: &[GraphName],
    ) -> Result<bool> {
        let graph = graph.into().into_owned();
        if !self.is_registered(graph.as_ref()).await? {
            return Ok(false);
        }
        self.registry_update(&registry::remap_update(graph.as_ref(), applies_to)?)
            .await?;
        Ok(true)
    }

    /// What is wrong with the registry: the violations of its SHACL shapes, one message each.
    pub async fn registry_problems(&self) -> Result<Vec<String>> {
        Ok(problems(
            self.registry_query(&registry::entries_query()).await?,
        ))
    }

    /// Activates or deactivates a registration; returns whether one was found.
    pub async fn set_schema_graph_active<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        active: bool,
    ) -> Result<bool> {
        let graph = graph.into().into_owned();
        if !self.is_registered(graph.as_ref()).await? {
            return Ok(false);
        }
        self.registry_update(&registry::set_active_update(graph.as_ref(), active)?)
            .await?;
        Ok(true)
    }

    /// Removes a registration, leaving the graph's triples in place.
    pub async fn unregister_schema_graph<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
    ) -> Result<bool> {
        let graph = graph.into().into_owned();
        if !self.is_registered(graph.as_ref()).await? {
            return Ok(false);
        }
        self.registry_update(&registry::unregister_update(graph.as_ref())?)
            .await?;
        Ok(true)
    }

    /// Removes a registration together with every quad of its graph, in one atomic update.
    pub async fn drop_schema_graph<'a>(&self, graph: impl Into<GraphNameRef<'a>>) -> Result<u64> {
        let graph = graph.into().into_owned();
        let n = count(
            self.registry_query(&registry::size_query(graph.as_ref()))
                .await?,
        );
        self.registry_update(&registry::drop_update(graph.as_ref())?)
            .await?;
        Ok(n)
    }

    /// Are the system graphs installed at the current vocabulary version?
    pub async fn system_graphs_installed(&self) -> Result<bool> {
        Ok(boolean(
            self.registry_query(&registry::system_graphs_ready_query())
                .await?,
        ))
    }

    /// Installs or refreshes the system graphs; returns `false` when they were already current.
    pub async fn install_system_graphs(&self) -> Result<bool> {
        if self.system_graphs_installed().await? {
            return Ok(false);
        }
        self.registry_update(&registry::system_graphs_update())
            .await?;
        Ok(true)
    }

    /// The compiled SHACL property shapes of the registered shapes graphs.
    pub async fn shape_index(&self) -> Result<ShapeIndex> {
        oxilite_core::job::run_async(
            &self.backend,
            oxilite_core::ops::shape_index_job(self.caps()),
        )
        .await
    }
}
