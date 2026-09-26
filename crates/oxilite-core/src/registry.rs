//! The schema registry: which named graphs hold schema rather than data, and which graphs each
//! one applies to — kept as RDF in the system graph `<oxilite:schema>`.
//!
//! Each registered graph is a resource of `<oxilite:schema>` with the graph's IRI as subject
//! (`oxl:DefaultGraph` for the default graph), typed with its role and described with the
//! `oxl:` vocabulary ([`VOCABULARY`]). Registry operations are plain SPARQL updates built here,
//! so they run unchanged on Oxigraph or any SPARQL store; the stores run them through their
//! update path. The SQL fragments below read the same triples to scope the derived caches
//! (`tbox_closure`, the shape index). While nothing is registered for a role, every graph but
//! the system graphs may contribute to it, so a store that uses no registry behaves as it
//! always did.
//!
//! The Rust reader ([`entries_from_rows`]), the SQL fragments and the SPARQL recipes of
//! `docs/schema-registry.md` agree on every edge case: a graph typed with several roles counts
//! for each, and only an `xsd:boolean` false (`false` or `0`) deactivates. [`problems`] checks
//! what the registry's own SHACL shapes (in [`VOCABULARY`]) check.
//!
// @lat: [[architecture#Schema registry]]

use crate::encoding::{named_node_id, term_id, DEFAULT_GRAPH_ID};
use crate::error::{Error, Result};
use crate::sql::Statement;
use oxrdf::vocab::{rdf, xsd};
use oxrdf::{GraphName, GraphNameRef, Literal, NamedNode, NamedNodeRef, QuadRef, Term, TermRef};
use spargebra::term::{GraphNamePattern, NamedNodePattern, TermPattern};
use spargebra::{GraphUpdateOperation, Update};
use std::collections::BTreeMap;

/// The system graph holding the registry.
pub const SCHEMA_GRAPH: &str = "oxilite:schema";

/// The system graph holding the oxilite vocabulary (installed by [`system_quads`]).
pub const VOCABULARY_GRAPH: &str = "oxilite:vocabulary";

/// The version of the vocabulary that [`system_quads`] installs (`oxl:version` of
/// `<oxilite:vocabulary>` in the registry).
pub const VOCABULARY_VERSION: &str = "2";

/// The oxilite namespace (`oxl:`).
pub const NS: &str = "https://oxilite.dev/ns#";

/// The oxilite vocabulary as Turtle: the registry's classes and properties, the SHACL shapes
/// the registry must conform to, and the terms of the history graph.
pub const VOCABULARY: &str = include_str!("../vocab/oxl.ttl");

/// IRIs of the registry vocabulary.
pub mod vocab {
    pub const SCHEMA_GRAPH: &str = "https://oxilite.dev/ns#SchemaGraph";
    pub const SYSTEM_GRAPH: &str = "https://oxilite.dev/ns#SystemGraph";
    pub const ONTOLOGY_GRAPH: &str = "https://oxilite.dev/ns#OntologyGraph";
    pub const SHAPES_GRAPH: &str = "https://oxilite.dev/ns#ShapesGraph";
    pub const SHEX_GRAPH: &str = "https://oxilite.dev/ns#ShExGraph";
    pub const APPLIES_TO: &str = "https://oxilite.dev/ns#appliesTo";
    pub const ACTIVE: &str = "https://oxilite.dev/ns#active";
    pub const ONTOLOGY_IRI: &str = "https://oxilite.dev/ns#ontologyIri";
    pub const VERSION: &str = "https://oxilite.dev/ns#version";
    pub const SHA256: &str = "https://oxilite.dev/ns#sha256";
    pub const LOADED_AT: &str = "https://oxilite.dev/ns#loadedAt";
    pub const DEFAULT_GRAPH: &str = "https://oxilite.dev/ns#DefaultGraph";
    pub const ALL_GRAPHS: &str = "https://oxilite.dev/ns#AllGraphs";
    pub const IMPORTS: &str = "http://www.w3.org/2002/07/owl#imports";
}

/// The graphs oxilite maintains itself. They never contribute axioms or shapes, whatever the
/// registry says about them.
pub const SYSTEM_GRAPHS: [&str; 2] = [SCHEMA_GRAPH, VOCABULARY_GRAPH];

/// What a registered graph holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum SchemaRole {
    /// An OWL / RDFS ontology: it feeds `tbox_closure`.
    Ontology,
    /// A SHACL shapes graph: it feeds the shape index.
    Shacl,
    /// A ShEx schema. Recorded and hidden like the others; nothing compiles it yet.
    Shex,
}

impl SchemaRole {
    pub const ALL: [Self; 3] = [Self::Ontology, Self::Shacl, Self::Shex];

    /// The role's name: `ontology`, `shacl` or `shex`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ontology => "ontology",
            Self::Shacl => "shacl",
            Self::Shex => "shex",
        }
    }

    /// The class typing a registered graph of this role.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Ontology => vocab::ONTOLOGY_GRAPH,
            Self::Shacl => vocab::SHAPES_GRAPH,
            Self::Shex => vocab::SHEX_GRAPH,
        }
    }

    /// The role a class names.
    pub fn from_class(iri: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.class() == iri)
    }
}

impl std::str::FromStr for SchemaRole {
    type Err = Error;

    /// Parses a role name (`shapes` is accepted for `shacl`), ignoring case.
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "ontology" => Self::Ontology,
            "shacl" | "shapes" => Self::Shacl,
            "shex" => Self::Shex,
            _ => {
                return Err(Error::Other(format!(
                    "unknown schema role {s}: one of ontology, shacl, shex"
                )))
            }
        })
    }
}

/// One registered graph, as `<oxilite:schema>` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaGraph {
    pub graph: GraphName,
    pub role: SchemaRole,
    /// The `owl:Ontology` IRI, when it differs from the graph name.
    pub iri: Option<NamedNode>,
    /// A version to pin (`owl:versionIRI`, a tag…).
    pub version: Option<String>,
    /// Digest of the document the graph was loaded from, for drift detection.
    pub sha256: Option<String>,
    /// `owl:imports` targets. An import naming another active registered ontology (by graph
    /// name or `oxl:ontologyIri`) brings its axioms into this ontology's scopes.
    pub imports: Vec<NamedNode>,
    /// The graphs the schema applies to; empty means every graph (written as
    /// `oxl:appliesTo oxl:AllGraphs`).
    pub applies_to: Vec<GraphName>,
    /// Inactive graphs stay registered (and hidden) but stop contributing.
    pub active: bool,
    /// When the graph was registered (`xsd:dateTime` lexical form).
    pub loaded_at: Option<String>,
}

impl SchemaGraph {
    /// An active registration applying to every graph, with nothing else recorded.
    pub fn new(graph: GraphName, role: SchemaRole) -> Self {
        Self {
            graph,
            role,
            iri: None,
            version: None,
            sha256: None,
            imports: Vec::new(),
            applies_to: Vec::new(),
            active: true,
            loaded_at: None,
        }
    }

    /// Does this schema apply to `target`?
    pub fn applies(&self, target: GraphNameRef<'_>) -> bool {
        self.applies_to.is_empty() || self.applies_to.iter().any(|g| g.as_ref() == target)
    }
}

// ---------------------------------------------------------------------------------- SPARQL

/// The IRI naming a graph in the registry (`oxl:DefaultGraph` for the default graph).
pub fn graph_node(g: GraphNameRef<'_>) -> Result<NamedNode> {
    match g {
        GraphNameRef::NamedNode(n) => Ok(n.into_owned()),
        GraphNameRef::DefaultGraph => Ok(NamedNode::new_unchecked(vocab::DEFAULT_GRAPH)),
        GraphNameRef::BlankNode(_) => Err(Error::Other(
            "a graph named by a blank node cannot be registered as schema".into(),
        )),
    }
}

/// The graph a registry IRI names (`oxl:DefaultGraph` is the default graph).
pub fn node_graph(n: &NamedNode) -> GraphName {
    if n.as_str() == vocab::DEFAULT_GRAPH {
        GraphName::DefaultGraph
    } else {
        n.clone().into()
    }
}

fn iri(s: &str) -> String {
    format!("<{s}>")
}

fn describe_delete(node: &NamedNode) -> String {
    format!(
        "DELETE WHERE {{ GRAPH {} {{ {node} ?p ?o }} }}",
        iri(SCHEMA_GRAPH)
    )
}

/// Is this literal an `xsd:boolean` false? (`"0"^^xsd:boolean` too; a plain `"false"` is not.)
fn is_false(l: &Literal) -> bool {
    l.datatype() == xsd::BOOLEAN && matches!(l.value(), "false" | "0")
}

/// The `oxl:appliesTo` objects of a registration: its targets, or `oxl:AllGraphs`.
fn targets_of(applies_to: &[GraphName]) -> Result<Vec<NamedNode>> {
    if applies_to.is_empty() {
        return Ok(vec![NamedNode::new_unchecked(vocab::ALL_GRAPHS)]);
    }
    applies_to.iter().map(|g| graph_node(g.as_ref())).collect()
}

/// The `xsd:dateTime` of now, where a clock is available.
fn now() -> Option<String> {
    Some(oxsdatatypes::DateTime::now().to_string())
}

/// SPARQL registering a graph: its old description, if any, is replaced. A named graph is
/// created (`CREATE SILENT GRAPH`) so the registry never names a graph the store lacks.
/// `loaded_at` is set to now when absent.
pub fn register_update(entry: &SchemaGraph) -> Result<String> {
    let node = graph_node(entry.graph.as_ref())?;
    let mut out = String::new();
    if let GraphName::NamedNode(n) = &entry.graph {
        out.push_str(&format!("CREATE SILENT GRAPH {n} ;\n"));
    }
    out.push_str(&describe_delete(&node));
    out.push_str(" ;\nINSERT DATA { GRAPH ");
    out.push_str(&iri(SCHEMA_GRAPH));
    out.push_str(" {\n");
    let mut triple = |p: &str, o: String| out.push_str(&format!("  {node} {} {o} .\n", iri(p)));
    triple(rdf::TYPE.as_str(), iri(entry.role.class()));
    triple(vocab::ACTIVE, Literal::from(entry.active).to_string());
    for t in targets_of(&entry.applies_to)? {
        triple(vocab::APPLIES_TO, t.to_string());
    }
    if let Some(i) = &entry.iri {
        triple(vocab::ONTOLOGY_IRI, i.to_string());
    }
    if let Some(v) = &entry.version {
        triple(vocab::VERSION, Literal::new_simple_literal(v).to_string());
    }
    if let Some(h) = &entry.sha256 {
        triple(vocab::SHA256, Literal::new_simple_literal(h).to_string());
    }
    for i in &entry.imports {
        triple(vocab::IMPORTS, i.to_string());
    }
    if let Some(t) = entry.loaded_at.clone().or_else(now) {
        triple(
            vocab::LOADED_AT,
            Literal::new_typed_literal(t, xsd::DATE_TIME).to_string(),
        );
    }
    out.push_str("} }");
    Ok(out)
}

/// SPARQL setting the graphs a registration applies to (empty: every graph), keeping the rest
/// of its description. Nothing happens to an unregistered graph.
pub fn remap_update(graph: GraphNameRef<'_>, applies_to: &[GraphName]) -> Result<String> {
    let node = graph_node(graph)?;
    let (g, a) = (iri(SCHEMA_GRAPH), iri(vocab::APPLIES_TO));
    let insert = targets_of(applies_to)?
        .iter()
        .map(|t| format!("{node} {a} {t} ."))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!(
        "DELETE {{ GRAPH {g} {{ {node} {a} ?t }} }} INSERT {{ GRAPH {g} {{ {insert} }} }} \
         WHERE {{ GRAPH {g} {{ {node} a ?role OPTIONAL {{ {node} {a} ?t }} }} }}"
    ))
}

/// SPARQL removing a registration (the graph's triples stay).
pub fn unregister_update(graph: GraphNameRef<'_>) -> Result<String> {
    Ok(describe_delete(&graph_node(graph)?))
}

/// SPARQL activating or deactivating a registration (nothing happens to an unregistered graph).
pub fn set_active_update(graph: GraphNameRef<'_>, active: bool) -> Result<String> {
    let node = graph_node(graph)?;
    let (g, a) = (iri(SCHEMA_GRAPH), iri(vocab::ACTIVE));
    Ok(format!(
        "DELETE {{ GRAPH {g} {{ {node} {a} ?a }} }} INSERT {{ GRAPH {g} {{ {node} {a} {} }} }} \
         WHERE {{ GRAPH {g} {{ {node} a ?role OPTIONAL {{ {node} {a} ?a }} }} }}",
        Literal::from(active)
    ))
}

/// SPARQL removing a registration and every triple of its graph.
pub fn drop_update(graph: GraphNameRef<'_>) -> Result<String> {
    let node = graph_node(graph)?;
    let drop = match graph {
        GraphNameRef::NamedNode(n) => format!("DROP SILENT GRAPH {n}"),
        _ => "CLEAR SILENT DEFAULT".to_owned(),
    };
    Ok(format!("{} ;\n{drop}", describe_delete(&node)))
}

/// SPARQL asking whether a graph is registered.
pub fn registered_query(graph: GraphNameRef<'_>) -> Result<String> {
    Ok(format!(
        "ASK {{ GRAPH {} {{ {} a ?role }} }}",
        iri(SCHEMA_GRAPH),
        graph_node(graph)?
    ))
}

/// SPARQL counting the triples of a graph (`?n`).
pub fn size_query(graph: GraphNameRef<'_>) -> String {
    match graph {
        GraphNameRef::NamedNode(n) => {
            format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH {n} {{ ?s ?p ?o }} }}")
        }
        _ => "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }".to_owned(),
    }
}

/// SPARQL reading the registry: `?g ?p ?o` rows, parsed by [`entries_from_rows`].
pub fn entries_query() -> String {
    format!(
        "SELECT ?g ?p ?o WHERE {{ GRAPH {} {{ ?g ?p ?o }} }}",
        iri(SCHEMA_GRAPH)
    )
}

/// The registrations described by `?g ?p ?o` rows, ordered by role and graph. Subjects
/// without a role class are ignored.
pub fn entries_from_rows(rows: &[Vec<Option<Term>>]) -> Vec<SchemaGraph> {
    let mut by: BTreeMap<String, (NamedNode, Vec<(String, Term)>)> = BTreeMap::new();
    for row in rows {
        let (Some(Term::NamedNode(s)), Some(Term::NamedNode(p)), Some(o)) = (
            row.first().cloned().flatten(),
            row.get(1).cloned().flatten(),
            row.get(2).cloned().flatten(),
        ) else {
            continue;
        };
        by.entry(s.as_str().to_owned())
            .or_insert_with(|| (s.clone(), Vec::new()))
            .1
            .push((p.into_string(), o));
    }
    let mut out = Vec::new();
    for (_, (node, props)) in by {
        // A graph may hold several roles (an ontology with its SHACL shapes): one entry each,
        // as the SQL scopes count it once per role.
        let mut roles: Vec<SchemaRole> = props
            .iter()
            .filter_map(|(p, o)| match o {
                Term::NamedNode(c) if p == rdf::TYPE.as_str() => SchemaRole::from_class(c.as_str()),
                _ => None,
            })
            .collect();
        roles.sort();
        roles.dedup();
        let Some(&first) = roles.first() else {
            continue;
        };
        let mut e = SchemaGraph::new(node_graph(&node), first);
        let mut all = false;
        for (p, o) in props {
            match (p.as_str(), o) {
                (vocab::ACTIVE, Term::Literal(l)) if is_false(&l) => {
                    e.active = false;
                }
                (vocab::APPLIES_TO, Term::NamedNode(n)) if n.as_str() == vocab::ALL_GRAPHS => {
                    all = true;
                }
                (vocab::APPLIES_TO, Term::NamedNode(n)) => e.applies_to.push(node_graph(&n)),
                (vocab::ONTOLOGY_IRI, Term::NamedNode(n)) => e.iri = Some(n),
                (vocab::VERSION, Term::Literal(l)) => e.version = Some(l.value().to_owned()),
                (vocab::SHA256, Term::Literal(l)) => e.sha256 = Some(l.value().to_owned()),
                (vocab::LOADED_AT, Term::Literal(l)) => e.loaded_at = Some(l.value().to_owned()),
                (vocab::IMPORTS, Term::NamedNode(n)) => e.imports.push(n),
                _ => {}
            }
        }
        if all {
            e.applies_to.clear();
        }
        e.applies_to.sort_by_key(ToString::to_string);
        e.imports.sort();
        for role in roles {
            out.push(SchemaGraph { role, ..e.clone() });
        }
    }
    out.sort_by_key(|e| (e.role, e.graph.to_string()));
    out
}

// ------------------------------------------------------------------------ change detection

fn registry_graph(g: GraphNameRef<'_>) -> bool {
    matches!(g, GraphNameRef::NamedNode(n) if n.as_str() == SCHEMA_GRAPH)
}

/// Does writing this quad change what the registry says? (Any quad of `<oxilite:schema>`.)
pub fn is_registry_quad(q: QuadRef<'_>) -> bool {
    registry_graph(q.graph_name)
}

/// Can this update change the registry? Any triple written to `<oxilite:schema>` counts. With
/// a variable graph (conservative), a triple counts when it could be a registry triple: a
/// variable predicate, an `oxl:` predicate, `owl:imports`, or `rdf:type` with a variable or
/// `oxl:` class — so a bulk `INSERT { GRAPH ?g { ?s a ex:Person } }` rebuilds nothing.
pub fn update_touches_registry(update: &Update) -> bool {
    let registry_triple = |p: &NamedNodePattern, o: &TermPattern| match p {
        NamedNodePattern::Variable(_) => true,
        NamedNodePattern::NamedNode(n) if n.as_ref() == rdf::TYPE => match o {
            TermPattern::NamedNode(c) => c.as_str().starts_with(NS),
            TermPattern::Variable(_) => true,
            _ => false,
        },
        NamedNodePattern::NamedNode(n) => {
            n.as_str().starts_with(NS) || n.as_str() == vocab::IMPORTS
        }
    };
    let counts = |g: &GraphNamePattern, p: &NamedNodePattern, o: &TermPattern| match g {
        GraphNamePattern::NamedNode(n) => n.as_str() == SCHEMA_GRAPH,
        GraphNamePattern::DefaultGraph => false,
        GraphNamePattern::Variable(_) => registry_triple(p, o),
    };
    update.operations.iter().any(|op| match op {
        GraphUpdateOperation::InsertData { data } => data.iter().any(|q| {
            matches!(&q.graph_name, spargebra::term::GraphName::NamedNode(n) if n.as_str() == SCHEMA_GRAPH)
        }),
        GraphUpdateOperation::DeleteData { data } => data.iter().any(|q| {
            matches!(&q.graph_name, spargebra::term::GraphName::NamedNode(n) if n.as_str() == SCHEMA_GRAPH)
        }),
        GraphUpdateOperation::DeleteInsert { delete, insert, .. } => {
            delete.iter().any(|q| {
                let o: TermPattern = q.object.clone().into();
                counts(&q.graph_name, &q.predicate, &o)
            }) || insert
                .iter()
                .any(|q| counts(&q.graph_name, &q.predicate, &q.object))
        }
        GraphUpdateOperation::Create { .. } => false,
        GraphUpdateOperation::Load { .. }
        | GraphUpdateOperation::Clear { .. }
        | GraphUpdateOperation::Drop { .. } => true,
    })
}

// -------------------------------------------------------------------------------------- SQL

/// Term ids the SQL fragments need.
struct Ids {
    reg: i64,
    ty: i64,
    applies: i64,
    active: i64,
    /// `false` and `"0"^^xsd:boolean`, the two lexical forms of an `xsd:boolean` false.
    falses: [i64; 2],
    dflt: i64,
    all: i64,
    system: i64,
    imports: i64,
    ontology_iri: i64,
}

fn ids() -> Ids {
    Ids {
        reg: named_node_id(SCHEMA_GRAPH),
        ty: named_node_id(rdf::TYPE.as_str()),
        applies: named_node_id(vocab::APPLIES_TO),
        active: named_node_id(vocab::ACTIVE),
        falses: [
            term_id(TermRef::Literal(Literal::from(false).as_ref())),
            term_id(TermRef::Literal(
                Literal::new_typed_literal("0", xsd::BOOLEAN).as_ref(),
            )),
        ],
        dflt: named_node_id(vocab::DEFAULT_GRAPH),
        all: named_node_id(vocab::ALL_GRAPHS),
        system: named_node_id(vocab::SYSTEM_GRAPH),
        imports: named_node_id(vocab::IMPORTS),
        ontology_iri: named_node_id(vocab::ONTOLOGY_IRI),
    }
}

/// The scope of the closure that applies to every graph (the id of `oxl:AllGraphs`).
pub fn all_scope() -> i64 {
    named_node_id(vocab::ALL_GRAPHS)
}

/// The graph id of the registry graph.
pub fn registry_graph_id() -> i64 {
    named_node_id(SCHEMA_GRAPH)
}

/// SQL: the graph id a registry node names (`oxl:DefaultGraph` is 0).
fn graph_of(i: &Ids, col: &str) -> String {
    format!(
        "(CASE {col} WHEN {} THEN {DEFAULT_GRAPH_ID} ELSE {col} END)",
        i.dflt
    )
}

/// SQL: the registry nodes (column `s`) of the active graphs of these roles.
/// SQL: the registry nodes (column `s`) of the graphs registered with this role, active or
/// not: the "every graph" fallback holds only while there are none, so deactivating the last
/// ontology silences it instead of letting every graph back in.
fn role_nodes(i: &Ids, role: SchemaRole) -> String {
    format!(
        "SELECT r.s AS s FROM quads r WHERE r.g = {} AND r.p = {} AND r.o = {}",
        i.reg,
        i.ty,
        named_node_id(role.class())
    )
}

fn active_nodes(i: &Ids, roles: &[SchemaRole]) -> String {
    let classes = roles
        .iter()
        .map(|r| named_node_id(r.class()).to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT r.s AS s FROM quads r WHERE r.g = {reg} AND r.p = {ty} AND r.o IN ({classes}) \
         AND NOT EXISTS (SELECT 1 FROM quads z WHERE z.g = {reg} AND z.s = r.s AND z.p = {active} AND z.o IN ({f0}, {f1}))",
        reg = i.reg,
        ty = i.ty,
        active = i.active,
        f0 = i.falses[0],
        f1 = i.falses[1]
    )
}

/// SQL: the ids of the system graphs, a fixed list. Typing another graph `oxl:SystemGraph`
/// hides it, but never takes it out of the "every graph" fallback.
fn system_graphs() -> String {
    SYSTEM_GRAPHS
        .iter()
        .map(|g| named_node_id(g).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// SQL: the graphs that may contribute to `role` — the active registered ones, or every graph
/// but the system graphs while none is registered for it (active or not).
///
/// `column` is the graph column of the quad source being filtered (e.g. `"g"`, `"x.g"`).
pub fn scope(role: SchemaRole, column: &str) -> String {
    let i = ids();
    let nodes = active_nodes(&i, &[role]);
    format!(
        "((NOT EXISTS ({}) AND {column} NOT IN ({})) OR {column} IN (SELECT {} FROM ({nodes}) an))",
        role_nodes(&i, role),
        system_graphs(),
        graph_of(&i, "an.s")
    )
}

/// SQL: a quad source restricted to the graphs that may contribute to `role`.
pub fn scoped_quads(role: SchemaRole) -> String {
    format!("(SELECT s, p, o, g FROM quads WHERE {})", scope(role, "g"))
}

/// SQL: the ids of every registered schema graph, whatever its role and whether or not it is
/// active.
fn registered_graphs(i: &Ids) -> String {
    let classes = SchemaRole::ALL
        .iter()
        .map(|r| named_node_id(r.class()))
        .chain([i.system])
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT {} FROM quads r WHERE r.g = {} AND r.p = {} AND r.o IN ({classes})",
        graph_of(i, "r.s"),
        i.reg,
        i.ty
    )
}

/// SQL: a quad source without the registry graph and the graphs it registers (see
/// `QueryOptions::include_schema_graphs`).
pub fn quads_without_schema_graphs() -> String {
    without_schema_graphs("quads")
}

/// SQL: `source` (a quad table) without the registry graph and the graphs it registers.
pub fn without_schema_graphs(source: &str) -> String {
    let i = ids();
    format!(
        "(SELECT s, p, o, g FROM {source} WHERE g NOT IN ({}) AND g NOT IN ({}))",
        system_graphs(),
        registered_graphs(&i)
    )
}

/// SQL: the ontology axioms as `(s, p, o, scope)`, restricted by `cond` (on alias `q`).
///
/// The axioms of the active ontology graphs that apply to every graph come with the scope
/// [`all_scope`] and again with every specific scope; those of an ontology mapped to graph G
/// with scope G. An ontology's `owl:imports` (recorded in the registry or asserted in its own
/// graph) that name another active ontology — by graph name or `oxl:ontologyIri` — bring that
/// ontology's axioms into the importer's scopes, transitively. While no ontology is
/// registered, every graph's triples but the system graphs' count, with scope [`all_scope`].
pub fn ontology_axioms(cond: &str) -> String {
    let i = ids();
    let act = active_nodes(&i, &[SchemaRole::Ontology]);
    // Specific targets of registry nodes (not oxl:AllGraphs).
    let targets = format!(
        "SELECT a.s AS n, {} AS t FROM quads a WHERE a.g = {} AND a.p = {} AND a.o <> {}",
        graph_of(&i, "a.o"),
        i.reg,
        i.applies,
        i.all
    );
    let to_all = format!(
        "SELECT a.s FROM quads a WHERE a.g = {} AND a.p = {} AND a.o = {}",
        i.reg, i.applies, i.all
    );
    // Active ontologies applying to every graph: no specific target, or oxl:AllGraphs.
    let global = format!(
        "SELECT ac.s AS s FROM ({act}) ac WHERE ac.s NOT IN (SELECT n FROM ({targets})) OR ac.s IN ({to_all})"
    );
    let specific = format!("SELECT st.n AS n, st.t AS t FROM ({targets}) st WHERE st.n IN ({act})");
    let direct = format!(
        "SELECT {g} AS g, {all} AS scope FROM ({global}) gl \
         UNION SELECT {g}, sp.t FROM ({global}) gl JOIN ({specific}) sp \
         UNION SELECT {n}, sp.t FROM ({specific}) sp",
        g = graph_of(&i, "gl.s"),
        n = graph_of(&i, "sp.n"),
        all = i.all
    );
    // (importer graph, imported IRI): recorded in the registry, or asserted in the ontology.
    let imports = format!(
        "SELECT {gx} AS f, x.o AS iri FROM quads x WHERE x.g = {reg} AND x.p = {imp} \
         UNION SELECT {gi}, y.o FROM ({act}) ia JOIN quads y ON y.p = {imp} AND y.g = {gi}",
        gx = graph_of(&i, "x.s"),
        gi = graph_of(&i, "ia.s"),
        reg = i.reg,
        imp = i.imports
    );
    // (IRI, graph) naming each active ontology: its graph name and its oxl:ontologyIri.
    let named = format!(
        "SELECT nb.s AS iri, {gn} AS t FROM ({act}) nb \
         UNION SELECT oi.o, {go} FROM quads oi WHERE oi.g = {reg} AND oi.p = {oiri} AND oi.s IN ({act})",
        gn = graph_of(&i, "nb.s"),
        go = graph_of(&i, "oi.s"),
        reg = i.reg,
        oiri = i.ontology_iri
    );
    let edges = format!(
        "SELECT io.f AS f, nm.t AS t FROM ({imports}) io JOIN ({named}) nm ON nm.iri = io.iri"
    );
    // UNION (not UNION ALL) ends the recursion on import cycles.
    let map = format!(
        "WITH RECURSIVE reg_direct(g, scope) AS ({direct}), \
         reg_map(g, scope) AS (SELECT g, scope FROM reg_direct \
           UNION SELECT ed.t, reg_map.scope FROM reg_map JOIN ({edges}) ed ON ed.f = reg_map.g) \
         SELECT g, scope FROM reg_map"
    );
    format!(
        "(SELECT q.s AS s, q.p AS p, q.o AS o, m.scope AS scope FROM quads q JOIN ({map}) m ON m.g = q.g WHERE {cond} \
         UNION ALL SELECT q.s, q.p, q.o, {all} FROM quads q WHERE {cond} AND NOT EXISTS ({registered}) AND q.g NOT IN ({sys}))",
        registered = role_nodes(&i, SchemaRole::Ontology),
        all = i.all,
        sys = system_graphs()
    )
}

/// Loads the specific closure scopes (graphs with ontologies of their own), kept in memory with
/// the planner statistics.
pub fn scopes_statement(id_col: impl Fn(&str) -> String) -> Statement {
    Statement::new(format!(
        "SELECT DISTINCT {} FROM tbox_closure WHERE scope <> {}",
        id_col("scope"),
        all_scope()
    ))
}

// ---------------------------------------------------------------------------- system graphs

/// The system graphs a new store starts with: the vocabulary in `<oxilite:vocabulary>`, and in
/// `<oxilite:schema>` a description of both system graphs (`oxl:SystemGraph`, with the
/// vocabulary's version). System graphs never narrow reasoning or the shape index.
pub fn system_quads() -> Vec<oxrdf::Quad> {
    let vocabulary = NamedNode::new_unchecked(VOCABULARY_GRAPH);
    let registry = NamedNode::new_unchecked(SCHEMA_GRAPH);
    let mut out: Vec<oxrdf::Quad> = oxrdfio::RdfParser::from_format(oxrdfio::RdfFormat::Turtle)
        .for_slice(VOCABULARY.as_bytes())
        .map(|q| {
            let q = q.expect("the bundled vocabulary is valid Turtle");
            oxrdf::Quad::new(q.subject, q.predicate, q.object, vocabulary.clone())
        })
        .collect();
    let t = |s: &NamedNode, p: &str, o: Term| {
        oxrdf::Quad::new(s.clone(), NamedNode::new_unchecked(p), o, registry.clone())
    };
    let system: Term = NamedNode::new_unchecked(vocab::SYSTEM_GRAPH).into();
    let label = "http://www.w3.org/2000/01/rdf-schema#label";
    out.extend([
        t(&registry, rdf::TYPE.as_str(), system.clone()),
        t(
            &registry,
            label,
            Literal::new_simple_literal("schema registry").into(),
        ),
        t(&vocabulary, rdf::TYPE.as_str(), system),
        t(
            &vocabulary,
            label,
            Literal::new_simple_literal("oxilite vocabulary").into(),
        ),
        t(&vocabulary, vocab::APPLIES_TO, registry.clone().into()),
        t(
            &vocabulary,
            vocab::ONTOLOGY_IRI,
            NamedNode::new_unchecked(NS).into(),
        ),
        t(
            &vocabulary,
            vocab::VERSION,
            Literal::new_simple_literal(VOCABULARY_VERSION).into(),
        ),
    ]);
    out
}

/// SPARQL installing (or refreshing) the system graphs on any store: the vocabulary graph is
/// replaced and the system graphs' descriptions rewritten; registrations are untouched.
pub fn system_graphs_update() -> String {
    let (reg, voc) = (iri(SCHEMA_GRAPH), iri(VOCABULARY_GRAPH));
    let mut out = format!(
        "CREATE SILENT GRAPH {reg} ;\nCREATE SILENT GRAPH {voc} ;\nCLEAR SILENT GRAPH {voc} ;\n\
         DELETE WHERE {{ GRAPH {reg} {{ {reg} ?p ?o }} }} ;\n\
         DELETE WHERE {{ GRAPH {reg} {{ {voc} ?p ?o }} }} ;\nINSERT DATA {{\n"
    );
    for q in system_quads() {
        out.push_str(&format!(
            "  GRAPH {} {{ {} {} {} . }}\n",
            q.graph_name, q.subject, q.predicate, q.object
        ));
    }
    out.push('}');
    out
}

/// SPARQL `ASK`: are the system graphs installed at the current vocabulary version?
pub fn system_graphs_ready_query() -> String {
    format!(
        "ASK {{ GRAPH {} {{ {} {} {} }} }}",
        iri(SCHEMA_GRAPH),
        iri(VOCABULARY_GRAPH),
        iri(vocab::VERSION),
        Literal::new_simple_literal(VOCABULARY_VERSION)
    )
}

// ------------------------------------------------------------------------------ validation

/// What is wrong with the registry described by `?g ?p ?o` rows (see [`entries_query`]): the
/// violations of the registry's SHACL shapes (`oxl:RegistrationShape` in [`VOCABULARY`]),
/// found without a SHACL engine, one message per violation. Empty means valid.
///
/// A node is checked when it has a role class, `oxl:SystemGraph`, or a registration property.
pub fn problems(rows: &[Vec<Option<Term>>]) -> Vec<String> {
    const PROPS: [&str; 7] = [
        vocab::APPLIES_TO,
        vocab::ACTIVE,
        vocab::ONTOLOGY_IRI,
        vocab::VERSION,
        vocab::SHA256,
        vocab::LOADED_AT,
        vocab::IMPORTS,
    ];
    let mut by: BTreeMap<String, Vec<(String, Term)>> = BTreeMap::new();
    for row in rows {
        let (Some(s), Some(Term::NamedNode(p)), Some(o)) = (
            row.first().cloned().flatten(),
            row.get(1).cloned().flatten(),
            row.get(2).cloned().flatten(),
        ) else {
            continue;
        };
        by.entry(s.to_string())
            .or_default()
            .push((p.into_string(), o));
    }
    let mut out = Vec::new();
    for (node, props) in by {
        let values = |p: &'static str| props.iter().filter(move |(q, _)| q == p).map(|(_, o)| o);
        let typed = values(rdf::TYPE.as_str()).any(|o| {
            matches!(o, Term::NamedNode(c)
                if SchemaRole::from_class(c.as_str()).is_some() || c.as_str() == vocab::SYSTEM_GRAPH)
        });
        let described = props.iter().any(|(p, _)| PROPS.contains(&p.as_str()));
        if !typed && !described {
            continue;
        }
        let mut bad = |m: String| out.push(format!("{node}: {m}"));
        if node.starts_with("_:") {
            bad("a blank node cannot name a graph".into());
        }
        if !typed {
            bad("has registration properties but no role class (oxl:OntologyGraph, oxl:ShapesGraph, oxl:ShExGraph)".into());
        }
        for p in [
            vocab::ACTIVE,
            vocab::ONTOLOGY_IRI,
            vocab::VERSION,
            vocab::SHA256,
            vocab::LOADED_AT,
        ] {
            let n = values(p).count();
            if n > 1 {
                bad(format!("<{p}> has {n} values, at most one is allowed"));
            }
        }
        for p in [vocab::APPLIES_TO, vocab::ONTOLOGY_IRI, vocab::IMPORTS] {
            for o in values(p).filter(|o| !matches!(o, Term::NamedNode(_))) {
                bad(format!("<{p}> {o} is not an IRI"));
            }
        }
        let literal = |p: &'static str, dt: NamedNodeRef<'static>| {
            values(p)
                .filter(move |o| !matches!(o, Term::Literal(l) if l.datatype() == dt))
                .map(move |o| format!("<{p}> {o} is not an <{}>", dt.as_str()))
                .collect::<Vec<_>>()
        };
        for m in literal(vocab::ACTIVE, xsd::BOOLEAN)
            .into_iter()
            .chain(literal(vocab::VERSION, xsd::STRING))
            .chain(literal(vocab::SHA256, xsd::STRING))
            .chain(literal(vocab::LOADED_AT, xsd::DATE_TIME))
        {
            bad(m);
        }
        for o in values(vocab::ACTIVE) {
            if let Term::Literal(l) = o {
                if l.datatype() == xsd::BOOLEAN
                    && !matches!(l.value(), "true" | "false" | "1" | "0")
                {
                    bad(format!(
                        "<{}> {o} is not a valid xsd:boolean",
                        vocab::ACTIVE
                    ));
                }
            }
        }
        for o in values(vocab::SHA256) {
            if let Term::Literal(l) = o {
                let v = l.value();
                if v.len() != 64 || !v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                    bad(format!(
                        "<{}> {o} is not 64 lowercase hex digits",
                        vocab::SHA256
                    ));
                }
            }
        }
    }
    out
}

// -------------------------------------------------------------------------------- migration

/// Schema version 1 kept registrations in a `schema_graphs` table: reads its rows with the
/// graph IRIs (`g`, `lex`, `role`, `iri`, `version`, `sha256`, `imports`, `active`).
pub fn legacy_rows_statement() -> Statement {
    Statement::new(
        "SELECT sg.g, t.lex, sg.role, sg.iri, sg.version, sg.sha256, sg.imports, sg.active \
         FROM schema_graphs sg LEFT JOIN terms t ON t.id = sg.g",
    )
}

/// The registrations of legacy rows (see [`legacy_rows_statement`]). Graphs named by blank
/// nodes, which the registry graph cannot name, are skipped.
pub fn legacy_entries(response: &crate::sql::Response) -> Result<Vec<SchemaGraph>> {
    use crate::sql::col;
    let Some(rs) = response.first() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for row in &rs.rows {
        let g = col(row, 0)?.as_i64();
        let text = |i: usize| -> Result<Option<String>> {
            Ok(col(row, i)?.clone().into_string().filter(|s| !s.is_empty()))
        };
        let graph = if g == Some(DEFAULT_GRAPH_ID) {
            GraphName::DefaultGraph
        } else {
            match text(1)?.and_then(|l| NamedNode::new(l).ok()) {
                Some(n) if g == Some(named_node_id(n.as_str())) => n.into(),
                _ => continue,
            }
        };
        let role = match col(row, 2)?.as_i64() {
            Some(1) => SchemaRole::Ontology,
            Some(2) => SchemaRole::Shacl,
            Some(3) => SchemaRole::Shex,
            _ => continue,
        };
        let mut e = SchemaGraph::new(graph, role);
        e.iri = text(3)?.and_then(|s| NamedNode::new(s).ok());
        e.version = text(4)?;
        e.sha256 = text(5)?;
        e.imports = text(6)?
            .map(|s| {
                s.split('\n')
                    .filter_map(|i| NamedNode::new(i).ok())
                    .collect()
            })
            .unwrap_or_default();
        e.active = col(row, 7)?.as_i64().unwrap_or(1) != 0;
        out.push(e);
    }
    Ok(out)
}

/// The registry triples of a registration, as quads of `<oxilite:schema>` (for writers that
/// cannot run SPARQL, like the migration).
pub fn entry_quads(entry: &SchemaGraph) -> Result<Vec<oxrdf::Quad>> {
    let node = graph_node(entry.graph.as_ref())?;
    let g = NamedNode::new_unchecked(SCHEMA_GRAPH);
    let q = |p: &str, o: Term| {
        oxrdf::Quad::new(node.clone(), NamedNode::new_unchecked(p), o, g.clone())
    };
    let mut out = vec![
        q(
            rdf::TYPE.as_str(),
            NamedNode::new_unchecked(entry.role.class()).into(),
        ),
        q(vocab::ACTIVE, Literal::from(entry.active).into()),
    ];
    for t in targets_of(&entry.applies_to)? {
        out.push(q(vocab::APPLIES_TO, t.into()));
    }
    if let Some(i) = &entry.iri {
        out.push(q(vocab::ONTOLOGY_IRI, i.clone().into()));
    }
    if let Some(v) = &entry.version {
        out.push(q(vocab::VERSION, Literal::new_simple_literal(v).into()));
    }
    if let Some(h) = &entry.sha256 {
        out.push(q(vocab::SHA256, Literal::new_simple_literal(h).into()));
    }
    for i in &entry.imports {
        out.push(q(vocab::IMPORTS, i.clone().into()));
    }
    if let Some(t) = &entry.loaded_at {
        out.push(q(
            vocab::LOADED_AT,
            Literal::new_typed_literal(t, xsd::DATE_TIME).into(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_update_parses_and_reads_back() {
        let mut e = SchemaGraph::new(
            NamedNode::new_unchecked("http://ex.org/onto").into(),
            SchemaRole::Ontology,
        );
        e.version = Some("v \"1\"".into());
        e.applies_to = vec![
            NamedNode::new_unchecked("http://ex.org/data").into(),
            GraphName::DefaultGraph,
        ];
        e.imports = vec![NamedNode::new_unchecked("http://ex.org/base")];
        e.loaded_at = Some("2026-09-26T10:00:00Z".into());
        let u = register_update(&e).unwrap();
        spargebra::SparqlParser::new().parse_update(&u).unwrap();
        for s in [
            unregister_update(e.graph.as_ref()).unwrap(),
            set_active_update(e.graph.as_ref(), false).unwrap(),
            drop_update(e.graph.as_ref()).unwrap(),
            drop_update(GraphNameRef::DefaultGraph).unwrap(),
        ] {
            spargebra::SparqlParser::new().parse_update(&s).unwrap();
        }
        let rows: Vec<Vec<Option<Term>>> = entry_quads(&e)
            .unwrap()
            .into_iter()
            .map(|q| {
                vec![
                    Some(q.subject.into()),
                    Some(q.predicate.into()),
                    Some(q.object),
                ]
            })
            .collect();
        let back = entries_from_rows(&rows);
        let mut want = e.clone();
        want.applies_to.sort_by_key(ToString::to_string);
        assert_eq!(back, vec![want]);
        assert_eq!(problems(&rows), Vec::<String>::new());
        spargebra::SparqlParser::new()
            .parse_update(&remap_update(e.graph.as_ref(), &[]).unwrap())
            .unwrap();
    }

    fn row(s: &str, p: &str, o: Term) -> Vec<Option<Term>> {
        vec![
            Some(NamedNode::new_unchecked(s).into()),
            Some(NamedNode::new_unchecked(p).into()),
            Some(o),
        ]
    }

    fn class(c: &str) -> Term {
        NamedNode::new_unchecked(c).into()
    }

    // @lat: [[tests#Schema registry#Readers agree on edge cases]]
    #[test]
    fn readers_agree_on_booleans_and_roles() {
        let g = "http://ex.org/g";
        let ty = rdf::TYPE.as_str();
        // A graph with two roles is listed once per role.
        let rows = vec![
            row(g, ty, class(vocab::ONTOLOGY_GRAPH)),
            row(g, ty, class(vocab::SHAPES_GRAPH)),
        ];
        let roles: Vec<_> = entries_from_rows(&rows).iter().map(|e| e.role).collect();
        assert_eq!(roles, vec![SchemaRole::Ontology, SchemaRole::Shacl]);
        // Only an xsd:boolean false deactivates, in either lexical form.
        for (o, active) in [
            (Literal::from(false), false),
            (Literal::new_typed_literal("0", xsd::BOOLEAN), false),
            (Literal::new_simple_literal("false"), true),
            (Literal::from(true), true),
        ] {
            let rows = vec![
                row(g, ty, class(vocab::ONTOLOGY_GRAPH)),
                row(g, vocab::ACTIVE, o.clone().into()),
            ];
            assert_eq!(entries_from_rows(&rows)[0].active, active, "{o}");
        }
        // The SQL reads both lexical forms of false.
        let sql = active_nodes(&ids(), &[SchemaRole::Ontology]);
        assert!(sql.contains(&ids().falses[1].to_string()));
    }

    // @lat: [[tests#Schema registry#Registry writes are detected narrowly]]
    #[test]
    fn registry_writes_are_detected_narrowly() {
        let touches = |u: &str| {
            let u =
                format!("PREFIX oxl: <https://oxilite.dev/ns#> PREFIX ex: <http://ex.org/> {u}");
            update_touches_registry(&spargebra::SparqlParser::new().parse_update(&u).unwrap())
        };
        // Bulk typing through a variable graph is not a registry write.
        assert!(!touches(
            "INSERT { GRAPH ?g { ?s a ex:Person } } WHERE { GRAPH ?g { ?s ex:p ?o } }"
        ));
        assert!(!touches(
            "DELETE { GRAPH ?g { ?s ex:p ?o } } WHERE { GRAPH ?g { ?s ex:p ?o } }"
        ));
        // Anything that could be one is.
        assert!(touches(
            "INSERT { GRAPH ?g { ?s a oxl:OntologyGraph } } WHERE { GRAPH ?g { ?s ex:p ?o } }"
        ));
        assert!(touches(
            "INSERT { GRAPH ?g { ?s a ?c } } WHERE { GRAPH ?g { ?s ex:p ?c } }"
        ));
        assert!(touches("DELETE { GRAPH ?g { ?s <http://www.w3.org/2002/07/owl#imports> ?o } } WHERE { GRAPH ?g { ?s ?p ?o } }"));
        assert!(touches("DELETE { GRAPH <oxilite:schema> { ?s ex:p ?o } } WHERE { GRAPH <oxilite:schema> { ?s ex:p ?o } }"));
        assert!(touches(
            "INSERT DATA { GRAPH <oxilite:schema> { ex:a ex:b ex:c } }"
        ));
    }

    // @lat: [[tests#Schema registry#Registry problems]]
    #[test]
    fn problems_mirror_the_registry_shapes() {
        let g = "http://ex.org/g";
        let ty = rdf::TYPE.as_str();
        let rows = vec![
            row(g, ty, class(vocab::ONTOLOGY_GRAPH)),
            row(
                g,
                vocab::ACTIVE,
                Literal::new_simple_literal("false").into(),
            ),
            row(g, vocab::ACTIVE, Literal::from(true).into()),
            row(
                g,
                vocab::APPLIES_TO,
                Literal::new_simple_literal("x").into(),
            ),
            row(g, vocab::SHA256, Literal::new_simple_literal("ABC").into()),
            row(
                "http://ex.org/h",
                vocab::APPLIES_TO,
                class(vocab::ALL_GRAPHS),
            ),
        ];
        let p = problems(&rows);
        assert_eq!(p.len(), 5, "{p:#?}");
        assert!(p
            .iter()
            .any(|m| m.starts_with("<http://ex.org/h>") && m.contains("no role class")));
    }
}
