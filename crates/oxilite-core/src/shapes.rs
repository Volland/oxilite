//! The compiled SHACL shape index.
//!
//! `shapes_index` and `shapes_in` hold the property shapes of the registered shapes graphs
//! (every graph while none is registered), pre-resolved to target class, path, `sh:datatype`,
//! `sh:minCount`, `sh:maxCount`, `sh:pattern`, `sh:in` values and whether the shape is
//! relationship-valued. They are caches: rebuilt from `quads` by `optimize()` and, inside the
//! same atomic request, by any write that touches a SHACL predicate — the discipline
//! `tbox_closure` already follows. Reading them costs one request and no SPARQL evaluation.
//!
// @lat: [[architecture#Schema registry#Compiled shape index]]

use crate::encoding::{decode_inline, decode_row, named_node_id, Tag, INT_OFFSET, PAYLOAD_BITS};
use crate::error::Result;
use crate::registry::{scoped_quads, SchemaRole};
use crate::sql::{col, Capabilities, Request, Response, Statement};
use oxrdf::vocab::rdf;
use oxrdf::{NamedNode, QuadRef, Term};
use spargebra::term::NamedNodePattern;
use spargebra::{GraphUpdateOperation, Update};
use std::collections::BTreeMap;

const SH: &str = "http://www.w3.org/ns/shacl#";

/// SHACL predicates whose triples change the compiled index.
///
/// `rdf:first` / `rdf:rest` are deliberately absent: they would make every write touching any
/// RDF list refresh the index. A shape and its `sh:in` list are written together in practice,
/// and the `sh:in` triple itself is a trigger; appending to an existing list without touching a
/// `sh:` predicate leaves the index stale until the next `optimize()`.
const SHAPE_LOCALS: [&str; 10] = [
    "targetClass",
    "property",
    "path",
    "datatype",
    "minCount",
    "maxCount",
    "pattern",
    "class",
    "node",
    "in",
];

fn sh(local: &str) -> i64 {
    named_node_id(&format!("{SH}{local}"))
}

/// Does writing this quad invalidate the shape index?
pub fn is_shape_quad(q: QuadRef<'_>) -> bool {
    is_shape_iri(q.predicate.as_str())
}

fn is_shape_iri(p: &str) -> bool {
    p.strip_prefix(SH)
        .is_some_and(|l| SHAPE_LOCALS.contains(&l))
}

/// Can this update invalidate the shape index? (Conservative: variables count as shapes.)
pub fn update_touches_shapes(update: &Update) -> bool {
    let pattern = |p: &NamedNodePattern| match p {
        NamedNodePattern::Variable(_) => true,
        NamedNodePattern::NamedNode(n) => is_shape_iri(n.as_str()),
    };
    update.operations.iter().any(|op| match op {
        GraphUpdateOperation::InsertData { data } => {
            data.iter().any(|q| is_shape_iri(q.predicate.as_str()))
        }
        GraphUpdateOperation::DeleteData { data } => {
            data.iter().any(|q| is_shape_iri(q.predicate.as_str()))
        }
        GraphUpdateOperation::DeleteInsert { delete, insert, .. } => {
            delete.iter().any(|q| pattern(&q.predicate))
                || insert.iter().any(|q| pattern(&q.predicate))
        }
        GraphUpdateOperation::Create { .. } => false,
        GraphUpdateOperation::Load { .. }
        | GraphUpdateOperation::Clear { .. }
        | GraphUpdateOperation::Drop { .. } => true,
    })
}

/// SQL: the integer value of a term id that should be an `xsd:integer` literal. Canonical
/// integers are inline (`Tag::Integer`); anything else falls back to `terms.num`, and a
/// non-numeric term decodes to NULL, i.e. "not declared".
fn int_value(x: &str) -> String {
    let int_base = Tag::Integer.base() + INT_OFFSET;
    let k_int = Tag::Integer as i64;
    format!(
        "CASE WHEN (({x}) >> {PAYLOAD_BITS}) = {k_int} THEN ({x}) - {int_base} \
         ELSE (SELECT CAST(n.num AS INTEGER) FROM terms n WHERE n.id = ({x}) AND n.nt IS NOT NULL) END"
    )
}

/// Statements rebuilding `shapes_index` and `shapes_in` from the asserted quads.
pub fn refresh_statements() -> Vec<Statement> {
    let sq = scoped_quads(SchemaRole::Shacl);
    // Every (shape, target class, property shape, path) the shapes graphs declare.
    let ps = format!(
        "SELECT t.o AS target, pr.o AS pshape, pa.o AS path \
         FROM {sq} t JOIN {sq} pr ON pr.s = t.s AND pr.p = {property} \
         JOIN {sq} pa ON pa.s = pr.o AND pa.p = {path} WHERE t.p = {target_class}",
        property = sh("property"),
        path = sh("path"),
        target_class = sh("targetClass"),
    );
    // A property shape's single-valued constraint, as the lexical form of its object.
    let lex_of = |local: &str| {
        format!(
            "(SELECT v.lex FROM {sq} x JOIN terms v ON v.id = x.o WHERE x.s = ps.pshape AND x.p = {})",
            sh(local)
        )
    };
    let int_of = |local: &str| {
        format!(
            "(SELECT {} FROM {sq} x WHERE x.s = ps.pshape AND x.p = {})",
            int_value("x.o"),
            sh(local)
        )
    };
    // `sh:class` / `sh:node` make a property relationship-valued.
    let rel = format!(
        "(SELECT EXISTS (SELECT 1 FROM {sq} x WHERE x.s = ps.pshape AND x.p IN ({}, {})))",
        sh("class"),
        sh("node")
    );
    // Several shapes may target the same (target, path). MAX ignores NULLs, so grouping
    // reproduces a field-by-field merge and is deterministic.
    let index = format!(
        "INSERT OR REPLACE INTO shapes_index(target, path, datatype, min_count, max_count, pattern, relationship) \
         SELECT tt.lex, pt.lex, MAX({datatype}), MAX({min}), MAX({max}), MAX({pattern}), MAX({rel}) \
         FROM ({ps}) ps JOIN terms tt ON tt.id = ps.target JOIN terms pt ON pt.id = ps.path \
         GROUP BY tt.lex, pt.lex",
        datatype = lex_of("datatype"),
        min = int_of("minCount"),
        max = int_of("maxCount"),
        pattern = lex_of("pattern"),
    );
    // `sh:in` is an RDF list: walk rdf:rest* from the list head, then take each rdf:first.
    let values = format!(
        "WITH RECURSIVE cells(pshape, node) AS (\
            SELECT x.s, x.o FROM {sq} x WHERE x.p = {sh_in} \
            UNION SELECT c.pshape, r.o FROM cells c JOIN {sq} r ON r.s = c.node AND r.p = {rest}) \
         INSERT OR REPLACE INTO shapes_in(target, path, id, lex, dt, lang, dir) \
         SELECT tt.lex, pt.lex, f.o, v.lex, v.dt, v.lang, v.dir \
         FROM ({ps}) ps JOIN cells c ON c.pshape = ps.pshape \
         JOIN {sq} f ON f.s = c.node AND f.p = {first} \
         JOIN terms tt ON tt.id = ps.target JOIN terms pt ON pt.id = ps.path \
         LEFT JOIN terms v ON v.id = f.o",
        sh_in = sh("in"),
        rest = named_node_id(rdf::REST.as_str()),
        first = named_node_id(rdf::FIRST.as_str()),
    );
    vec![
        Statement::new("DELETE FROM shapes_index"),
        Statement::new("DELETE FROM shapes_in"),
        Statement::new(index),
        Statement::new(values),
    ]
}

/// One property shape, merged across every shape declaring it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PropertyShape {
    pub datatype: Option<NamedNode>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub values_in: Vec<Term>,
    pub pattern: Option<String>,
    /// Relationship-valued (`sh:class` / `sh:node`).
    pub relationship: bool,
}

/// The compiled property shapes, by target class and path.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapeIndex {
    pub by_class: BTreeMap<NamedNode, BTreeMap<NamedNode, PropertyShape>>,
}

impl ShapeIndex {
    pub fn is_empty(&self) -> bool {
        self.by_class.is_empty()
    }

    /// The shapes of a path for a node with these classes.
    pub fn get(&self, class: &NamedNode, path: &NamedNode) -> Option<&PropertyShape> {
        self.by_class.get(class).and_then(|m| m.get(path))
    }

    /// Statements loading both tables.
    pub fn load_request(caps: &Capabilities) -> Request {
        let id = if caps.int64_as_text {
            "CAST(id AS TEXT)"
        } else {
            "id"
        };
        Request::read(vec![
            Statement::new(
                "SELECT target, path, datatype, min_count, max_count, pattern, relationship FROM shapes_index",
            ),
            Statement::new(format!(
                "SELECT target, path, {id}, lex, dt, lang, dir FROM shapes_in"
            )),
        ])
    }

    /// Decodes the response of [`Self::load_request`].
    pub fn from_response(response: &Response) -> Result<Self> {
        let mut me = Self::default();
        let Some(index) = response.first() else {
            return Ok(me);
        };
        for row in &index.rows {
            let (Some(target), Some(path)) = (named(col(row, 0)?), named(col(row, 1)?)) else {
                continue;
            };
            let shape = me.by_class.entry(target).or_default().entry(path).or_default();
            shape.datatype = named(col(row, 2)?);
            shape.min = col(row, 3)?.as_i64();
            shape.max = col(row, 4)?.as_i64();
            shape.pattern = col(row, 5)?.clone().into_string();
            shape.relationship = col(row, 6)?.as_i64().unwrap_or(0) != 0;
        }
        let Some(values) = response.get(1) else {
            return Ok(me);
        };
        for row in &values.rows {
            let (Some(target), Some(path), Some(id)) =
                (named(col(row, 0)?), named(col(row, 1)?), col(row, 2)?.as_i64())
            else {
                continue;
            };
            let term = match col(row, 3)?.clone().into_string() {
                // A hashed term: rebuild it from its `terms` row.
                Some(lex) => decode_row(
                    id,
                    lex,
                    col(row, 4)?.clone().into_string(),
                    col(row, 5)?.clone().into_string(),
                    col(row, 6)?.as_i64(),
                )?,
                // An inline value (integer or boolean) has no `terms` row.
                None => match decode_inline(id) {
                    Some(t) => t,
                    None => continue,
                },
            };
            let shape = me.by_class.entry(target).or_default().entry(path).or_default();
            if !shape.values_in.contains(&term) {
                shape.values_in.push(term);
            }
        }
        Ok(me)
    }
}

fn named(v: &crate::sql::SqlValue) -> Option<NamedNode> {
    NamedNode::new(v.as_str()?).ok()
}
