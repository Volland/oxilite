//! The schema registry: which named graphs hold schema rather than data.
//!
//! A schema graph is an ordinary named graph whose triples stay in `quads`; `schema_graphs`
//! only labels it with a [`SchemaRole`]. Registering an ontology narrows the TBox closure to
//! the active ontology graphs ([`crate::reason`]); registering a shapes graph narrows the
//! compiled shape index ([`crate::shapes`]). While nothing is registered for a role, every
//! graph may contribute to it, so a store that uses no registry behaves as it always did.
//!
// @lat: [[architecture#Schema registry]]

use crate::encoding::DEFAULT_GRAPH_ID;
use crate::error::Result;
use crate::sql::{col, sql_str, Capabilities, Request, Response, Statement};

/// What a registered graph holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[repr(i64)]
pub enum SchemaRole {
    /// An OWL / RDFS ontology: it feeds `tbox_closure`.
    Ontology = 1,
    /// A SHACL shapes graph: it feeds `shapes_index`.
    Shacl = 2,
    /// A ShEx schema. Recorded and hidden like the others; nothing compiles it yet.
    Shex = 3,
}

impl SchemaRole {
    pub fn from_i64(v: i64) -> Option<Self> {
        Some(match v {
            1 => Self::Ontology,
            2 => Self::Shacl,
            3 => Self::Shex,
            _ => return None,
        })
    }

    pub const fn as_i64(self) -> i64 {
        self as i64
    }
}

/// One row of `schema_graphs`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct SchemaGraph {
    /// Term id of the graph name (0 is the default graph).
    pub graph: i64,
    pub role: SchemaRole,
    /// The `owl:Ontology` IRI, when it differs from the graph name.
    pub iri: Option<String>,
    /// `owl:versionIRI`, a version string, or anything the caller wants to pin.
    pub version: Option<String>,
    /// Digest of the loaded document, for drift detection by the caller.
    pub sha256: Option<String>,
    /// `owl:imports` targets, as the caller recorded them.
    pub imports: Vec<String>,
    /// Inactive graphs stay registered (and hidden) but stop contributing.
    pub active: bool,
    /// Seconds since the Unix epoch, as the caller recorded them.
    pub loaded_at: f64,
}

impl SchemaGraph {
    /// A registration with only the required fields filled in.
    pub fn new(graph: i64, role: SchemaRole) -> Self {
        Self {
            graph,
            role,
            iri: None,
            version: None,
            sha256: None,
            imports: Vec::new(),
            active: true,
            loaded_at: 0.0,
        }
    }
}

fn opt_str(v: Option<&String>) -> String {
    v.map_or_else(|| "NULL".into(), |s| sql_str(s))
}

/// Registers (or replaces the registration of) one graph.
pub fn register_statements(entry: &SchemaGraph) -> Vec<Statement> {
    // `imports` is stored as newline-separated IRIs: an IRI cannot contain a newline, so no
    // escaping is needed and no JSON parser is pulled into the core.
    let imports = if entry.imports.is_empty() {
        "NULL".to_string()
    } else {
        sql_str(&entry.imports.join("\n"))
    };
    vec![Statement::new(format!(
        "INSERT OR REPLACE INTO schema_graphs(g, role, iri, version, sha256, imports, active, loaded_at) \
         VALUES ({g}, {role}, {iri}, {version}, {sha}, {imports}, {active}, {loaded_at})",
        g = entry.graph,
        role = entry.role.as_i64(),
        iri = opt_str(entry.iri.as_ref()),
        version = opt_str(entry.version.as_ref()),
        sha = opt_str(entry.sha256.as_ref()),
        active = i64::from(entry.active),
        loaded_at = entry.loaded_at,
    ))]
}

/// Removes a registration, leaving the graph's triples alone.
pub fn unregister_statements(graph: i64) -> Vec<Statement> {
    vec![Statement::new(format!(
        "DELETE FROM schema_graphs WHERE g = {graph}"
    ))]
}

/// Activates or deactivates a registration.
pub fn set_active_statements(graph: i64, active: bool) -> Vec<Statement> {
    vec![Statement::new(format!(
        "UPDATE schema_graphs SET active = {} WHERE g = {graph}",
        i64::from(active)
    ))]
}

/// Removes a registration together with every quad of its graph.
pub fn drop_statements(graph: i64) -> Vec<Statement> {
    let mut s = vec![Statement::new(format!(
        "DELETE FROM quads WHERE g = {graph}"
    ))];
    if graph != DEFAULT_GRAPH_ID {
        s.push(Statement::new(format!(
            "DELETE FROM graphs WHERE id = {graph}"
        )));
    }
    s.extend(unregister_statements(graph));
    s
}

/// Reads the whole registry.
pub fn load_request(caps: &Capabilities) -> Request {
    let g = if caps.int64_as_text {
        "CAST(g AS TEXT)"
    } else {
        "g"
    };
    Request::read(vec![Statement::new(format!(
        "SELECT {g}, role, iri, version, sha256, imports, active, loaded_at FROM schema_graphs ORDER BY role, g"
    ))])
}

/// Decodes the response of [`load_request`].
pub fn from_response(response: &Response) -> Result<Vec<SchemaGraph>> {
    let Some(rs) = response.first() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(rs.rows.len());
    for row in &rs.rows {
        let (Some(graph), Some(role)) = (col(row, 0)?.as_i64(), col(row, 1)?.as_i64()) else {
            continue;
        };
        let Some(role) = SchemaRole::from_i64(role) else {
            continue;
        };
        let text = |i: usize| -> Result<Option<String>> {
            Ok(col(row, i)?.clone().into_string().filter(|s| !s.is_empty()))
        };
        out.push(SchemaGraph {
            graph,
            role,
            iri: text(2)?,
            version: text(3)?,
            sha256: text(4)?,
            imports: text(5)?
                .map(|s| s.split('\n').map(str::to_owned).collect())
                .unwrap_or_default(),
            active: col(row, 6)?.as_i64().unwrap_or(1) != 0,
            loaded_at: col(row, 7)?.as_f64().unwrap_or(0.0),
        });
    }
    Ok(out)
}

/// SQL: the graphs that may contribute to `role` — the active registered ones, or every graph
/// while none is registered for it.
///
/// `column` is the graph column of the quad source being filtered (e.g. `"g"`, `"x.g"`).
pub fn scope(role: SchemaRole, column: &str) -> String {
    let r = role.as_i64();
    format!(
        "(NOT EXISTS (SELECT 1 FROM schema_graphs WHERE role = {r} AND active = 1) \
         OR {column} IN (SELECT g FROM schema_graphs WHERE role = {r} AND active = 1))"
    )
}

/// SQL: a quad source restricted to the graphs that may contribute to `role`.
pub fn scoped_quads(role: SchemaRole) -> String {
    format!(
        "(SELECT s, p, o, g FROM quads WHERE {})",
        scope(role, "g")
    )
}

/// SQL: a quad source with every registered schema graph removed, whatever its role and
/// whether or not it is active (see `QueryOptions::include_schema_graphs`).
pub fn quads_without_schema_graphs() -> &'static str {
    "(SELECT s, p, o, g FROM quads WHERE g NOT IN (SELECT g FROM schema_graphs))"
}
