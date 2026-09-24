//! The schema registry on the stores: declaring a graph to be an ontology or a shapes graph.
//!
//! The registry labels graphs; it never moves triples. Registering an ontology narrows the
//! TBox closure to the active ontology graphs, registering a shapes graph narrows the compiled
//! shape index, and both are rebuilt in the same atomic request as the registration itself.
//! While nothing is registered for a role, every graph contributes to it.
//!
// @lat: [[architecture#Schema registry]]

use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::job::run_async;
use oxilite_core::registry::{SchemaGraph, SchemaRole};
use oxilite_core::shapes::ShapeIndex;
use oxilite_core::{encoding, ops, AsyncBackend, Result, SyncBackend};
use oxrdf::{GraphName, GraphNameRef, NamedNode};

/// What is recorded about a registered graph, besides its name and role.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registration {
    /// The `owl:Ontology` IRI, when it differs from the graph name.
    pub iri: Option<NamedNode>,
    /// `owl:versionIRI`, a version string, or anything else worth pinning.
    pub version: Option<String>,
    /// Digest of the document the graph was loaded from, for drift detection.
    pub sha256: Option<String>,
    /// `owl:imports` targets, recorded but not resolved.
    pub imports: Vec<NamedNode>,
    /// An inactive graph stays registered (and hidden) but stops contributing.
    pub active: bool,
}

impl Registration {
    /// An active registration with nothing else recorded.
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

    /// Registers the graph inactive.
    pub fn inactive(mut self) -> Self {
        self.active = false;
        self
    }
}

/// A registry entry, with its graph name resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredGraph {
    pub graph: GraphName,
    pub role: SchemaRole,
    pub registration: Registration,
    /// Seconds since the Unix epoch, as recorded when the graph was registered.
    pub loaded_at: f64,
}

/// Seconds since the Unix epoch, or 0 where no clock is available (wasm).
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn to_row(graph: i64, role: SchemaRole, r: &Registration) -> SchemaGraph {
    SchemaGraph {
        graph,
        role,
        iri: r.iri.as_ref().map(|n| n.as_str().to_owned()),
        version: r.version.clone(),
        sha256: r.sha256.clone(),
        imports: r.imports.iter().map(|n| n.as_str().to_owned()).collect(),
        active: r.active,
        loaded_at: now(),
    }
}

fn from_row(row: SchemaGraph, graph: GraphName) -> RegisteredGraph {
    RegisteredGraph {
        graph,
        role: row.role,
        registration: Registration {
            iri: row.iri.and_then(|s| NamedNode::new(s).ok()),
            version: row.version,
            sha256: row.sha256,
            imports: row
                .imports
                .into_iter()
                .filter_map(|s| NamedNode::new(s).ok())
                .collect(),
            active: row.active,
        },
        loaded_at: row.loaded_at,
    }
}

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// Declares a graph to hold an ontology, SHACL shapes or a ShEx schema.
    ///
    /// The graph's triples are untouched and stay queryable. Registering the first ontology
    /// graph narrows reasoning to the registered ones; the same holds for shapes and the
    /// compiled shape index. Registering a graph again replaces its entry.
    pub fn register_schema_graph<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        role: SchemaRole,
        registration: &Registration,
    ) -> Result<()> {
        let graph = graph.into();
        let id = encoding::graph_id(graph);
        self.run(ops::register_schema_graph_job(
            &to_row(id, role, registration),
            graph,
            self.caps(),
        ))?;
        self.reload_stats()
    }

    /// The registry, ordered by role and graph.
    pub fn schema_graphs(&self) -> Result<Vec<RegisteredGraph>> {
        Ok(self
            .run(ops::schema_graphs_job(self.caps()))?
            .into_iter()
            .map(|(row, name)| from_row(row, name))
            .collect())
    }

    /// Activates or deactivates a registration; returns whether one was found.
    pub fn set_schema_graph_active<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        active: bool,
    ) -> Result<bool> {
        let id = encoding::graph_id(graph.into());
        let changed = self.run(ops::set_schema_graph_active_job(id, active))?;
        self.reload_stats()?;
        Ok(changed)
    }

    /// Removes a registration, leaving the graph's triples in place; returns whether one was
    /// found.
    pub fn unregister_schema_graph<'a>(&self, graph: impl Into<GraphNameRef<'a>>) -> Result<bool> {
        let id = encoding::graph_id(graph.into());
        let removed = self.run(ops::unregister_schema_graph_job(id))?;
        self.reload_stats()?;
        Ok(removed)
    }

    /// Removes a registration together with every quad of its graph, in one atomic request;
    /// returns how many quads were removed.
    pub fn drop_schema_graph<'a>(&self, graph: impl Into<GraphNameRef<'a>>) -> Result<u64> {
        let id = encoding::graph_id(graph.into());
        let removed = self.run(ops::drop_schema_graph_job(id))?;
        self.reload_stats()?;
        Ok(removed)
    }

    /// The compiled SHACL property shapes of the registered shapes graphs (of every graph
    /// while none is registered). One request, no SPARQL evaluation.
    pub fn shape_index(&self) -> Result<ShapeIndex> {
        self.run(ops::shape_index_job(self.caps()))
    }
}

impl<B: AsyncBackend> AsyncStore<B> {
    /// Declares a graph to hold an ontology, SHACL shapes or a ShEx schema.
    pub async fn register_schema_graph<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        role: SchemaRole,
        registration: &Registration,
    ) -> Result<()> {
        let graph = graph.into();
        let id = encoding::graph_id(graph);
        run_async(
            &self.backend,
            ops::register_schema_graph_job(&to_row(id, role, registration), graph, self.caps()),
        )
        .await?;
        self.reload_stats().await
    }

    /// The registry, ordered by role and graph.
    pub async fn schema_graphs(&self) -> Result<Vec<RegisteredGraph>> {
        Ok(
            run_async(&self.backend, ops::schema_graphs_job(self.caps()))
                .await?
                .into_iter()
                .map(|(row, name)| from_row(row, name))
                .collect(),
        )
    }

    /// Activates or deactivates a registration; returns whether one was found.
    pub async fn set_schema_graph_active<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
        active: bool,
    ) -> Result<bool> {
        let id = encoding::graph_id(graph.into());
        let changed =
            run_async(&self.backend, ops::set_schema_graph_active_job(id, active)).await?;
        self.reload_stats().await?;
        Ok(changed)
    }

    /// Removes a registration, leaving the graph's triples in place.
    pub async fn unregister_schema_graph<'a>(
        &self,
        graph: impl Into<GraphNameRef<'a>>,
    ) -> Result<bool> {
        let id = encoding::graph_id(graph.into());
        let removed = run_async(&self.backend, ops::unregister_schema_graph_job(id)).await?;
        self.reload_stats().await?;
        Ok(removed)
    }

    /// Removes a registration together with every quad of its graph, in one atomic request.
    pub async fn drop_schema_graph<'a>(&self, graph: impl Into<GraphNameRef<'a>>) -> Result<u64> {
        let id = encoding::graph_id(graph.into());
        let removed = run_async(&self.backend, ops::drop_schema_graph_job(id)).await?;
        self.reload_stats().await?;
        Ok(removed)
    }

    /// The compiled SHACL property shapes of the registered shapes graphs.
    pub async fn shape_index(&self) -> Result<ShapeIndex> {
        run_async(&self.backend, ops::shape_index_job(self.caps())).await
    }
}
