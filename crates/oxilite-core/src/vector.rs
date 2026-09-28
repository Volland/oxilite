//! Vector indexes over embeddings stored as RDF literals (backends with vector functions: Turso).
//!
//! An index is described in the system graph `<oxilite:vectors>` by the resource
//! `<oxilite:vector/NAME>` (see [`VectorIndex::to_quads`]) — the definition is data, queryable
//! and versioned like any other. It is realised as a table `vec_name` of `(s, o, g, e)` rows,
//! one per embedding literal of the indexed property, kept current by triggers on `quads` in
//! the same transaction as every write, and back-filled when it is created. A fingerprint in
//! `oxilite_meta` (`vector:name`) records how the table was built, so [`sync_statements`] can
//! tell a missing, stale or orphaned table from a current one.
//!
//! [`knn_sql`] is the one nearest-neighbour statement: the SPARQL compiler
//! (`SERVICE <oxilite:vector/NAME>`), the Datalog built-in `nearest` and the store API all use it.
//!
// @lat: [[architecture#Vector indexes]]

use crate::encoding::{named_node_id, rdf_type_id, DEFAULT_GRAPH_ID};
use crate::error::{Error, Result};
use crate::registry::NS;
use crate::sql::{sql_str, Capabilities, Statement};
use oxrdf::vocab::{rdf, xsd};
use oxrdf::{GraphName, Literal, NamedNode, Quad, Term};
use std::collections::BTreeMap;

/// The system graph holding vector index definitions.
pub const VECTORS_GRAPH: &str = "oxilite:vectors";

/// The IRI prefix of an index: `<oxilite:vector/NAME>` names it, and is the `SERVICE` IRI that
/// searches it.
pub const INDEX_PREFIX: &str = "oxilite:vector/";

/// The key prefix of build fingerprints in `oxilite_meta`.
const META_PREFIX: &str = "vector:";

/// Aborts a back-fill that meets a value of the wrong dimensions ("CHECK constraint failed:
/// vector_dimensions_mismatch", see `Error::backend`).
const GUARD_TABLE: &str = "CREATE TABLE IF NOT EXISTS oxilite_vector_guard (\
    vector_dimensions_mismatch TEXT CHECK (vector_dimensions_mismatch IS NULL))";

/// Most results a search may ask for.
pub const MAX_K: u64 = 10_000;

/// Default number of results.
pub const DEFAULT_K: u64 = 10;

/// IRIs of the vector vocabulary (in the `oxl:` namespace).
pub mod vocab {
    pub const VECTOR_INDEX: &str = "https://oxilite.dev/ns#VectorIndex";
    pub const INDEX_NAME: &str = "https://oxilite.dev/ns#indexName";
    pub const PROPERTY: &str = "https://oxilite.dev/ns#property";
    pub const DIMENSIONS: &str = "https://oxilite.dev/ns#dimensions";
    pub const METRIC: &str = "https://oxilite.dev/ns#metric";
    pub const ELEMENT_TYPE: &str = "https://oxilite.dev/ns#elementType";
    pub const CLASS: &str = "https://oxilite.dev/ns#class";
    /// Search predicates, inside `SERVICE <oxilite:vector/NAME> { … }`.
    pub const QUERY: &str = "https://oxilite.dev/ns#query";
    pub const K: &str = "https://oxilite.dev/ns#k";
    pub const NODE: &str = "https://oxilite.dev/ns#node";
    pub const DISTANCE: &str = "https://oxilite.dev/ns#distance";
    pub const SCORE: &str = "https://oxilite.dev/ns#score";
}

/// How distance is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Metric {
    #[default]
    Cosine,
    Euclidean,
    DotProduct,
    /// Jaccard distance over sparse vectors (weighted: `1 - Σmin / Σmax`).
    Jaccard,
}

impl Metric {
    pub const ALL: [Self; 4] = [
        Self::Cosine,
        Self::Euclidean,
        Self::DotProduct,
        Self::Jaccard,
    ];

    /// The name used by Cypher, the shell and the studio.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cosine => "cosine",
            Self::Euclidean => "euclidean",
            Self::DotProduct => "dot",
            Self::Jaccard => "jaccard",
        }
    }

    pub fn iri(self) -> String {
        format!(
            "{NS}{}",
            match self {
                Self::Cosine => "Cosine",
                Self::Euclidean => "Euclidean",
                Self::DotProduct => "DotProduct",
                Self::Jaccard => "Jaccard",
            }
        )
    }

    /// Parses a name (`cosine`, `euclidean`/`l2`, `dot`/`dot_product`, `jaccard`).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "cosine" | "cos" => Self::Cosine,
            "euclidean" | "l2" => Self::Euclidean,
            "dot" | "dot_product" | "dotproduct" => Self::DotProduct,
            "jaccard" => Self::Jaccard,
            _ => return None,
        })
    }

    pub fn from_iri(iri: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.iri() == iri)
    }

    /// The SQL distance function.
    pub fn distance_fn(self) -> &'static str {
        match self {
            Self::Cosine => "vector_distance_cos",
            Self::Euclidean => "vector_distance_l2",
            Self::DotProduct => "vector_distance_dot",
            Self::Jaccard => "vector_distance_jaccard",
        }
    }

    /// The similarity score of a distance `d`, as Neo4j defines it: higher is nearer.
    pub fn score_sql(self, d: &str) -> String {
        match self {
            Self::Cosine => format!("(1.0 - ({d}) / 2.0)"),
            Self::Euclidean => format!("(1.0 / (1.0 + ({d}) * ({d})))"),
            // Turso's dot distance is the negated dot product.
            Self::DotProduct => format!("(-({d}))"),
            Self::Jaccard => format!("(1.0 - ({d}))"),
        }
    }

    /// [`Self::score_sql`] in Rust.
    pub fn score(self, d: f64) -> f64 {
        match self {
            Self::Cosine => 1.0 - d / 2.0,
            Self::Euclidean => 1.0 / (1.0 + d * d),
            Self::DotProduct => -d,
            Self::Jaccard => 1.0 - d,
        }
    }
}

/// How vector elements are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ElementType {
    #[default]
    Float32,
    Float64,
    /// Quantized to 8 bits.
    Int8,
    /// Quantized to one bit per dimension.
    Bit1,
    /// Sparse 32-bit floats (only the non-zero elements are stored).
    SparseFloat32,
}

impl ElementType {
    pub const ALL: [Self; 5] = [
        Self::Float32,
        Self::Float64,
        Self::Int8,
        Self::Bit1,
        Self::SparseFloat32,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Float32 => "float32",
            Self::Float64 => "float64",
            Self::Int8 => "int8",
            Self::Bit1 => "bit1",
            Self::SparseFloat32 => "sparse",
        }
    }

    pub fn iri(self) -> String {
        format!(
            "{NS}{}",
            match self {
                Self::Float32 => "Float32",
                Self::Float64 => "Float64",
                Self::Int8 => "Int8",
                Self::Bit1 => "Bit1",
                Self::SparseFloat32 => "SparseFloat32",
            }
        )
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "float32" | "f32" => Self::Float32,
            "float64" | "f64" => Self::Float64,
            "int8" | "i8" | "float8" => Self::Int8,
            "bit1" | "1bit" | "binary" => Self::Bit1,
            "sparse" | "sparse_float32" | "sparsefloat32" => Self::SparseFloat32,
            _ => return None,
        })
    }

    pub fn from_iri(iri: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.iri() == iri)
    }

    /// The SQL function turning a JSON array into a vector of this type.
    pub fn convert_fn(self) -> &'static str {
        match self {
            Self::Float32 => "vector32",
            Self::Float64 => "vector64",
            Self::Int8 => "vector8",
            Self::Bit1 => "vector1bit",
            Self::SparseFloat32 => "vector32_sparse",
        }
    }
}

/// A vector index definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorIndex {
    pub name: String,
    /// The property whose literal values are embeddings (`"[0.1, 0.2, …]"`).
    pub property: NamedNode,
    pub dimensions: u32,
    pub metric: Metric,
    pub element_type: ElementType,
    /// Only instances of this class are candidates.
    pub class: Option<NamedNode>,
}

impl VectorIndex {
    /// A cosine, 32-bit float index.
    pub fn new(name: impl Into<String>, property: NamedNode, dimensions: u32) -> Self {
        Self {
            name: name.into(),
            property,
            dimensions,
            metric: Metric::Cosine,
            element_type: ElementType::Float32,
            class: None,
        }
    }

    pub fn metric(mut self, metric: Metric) -> Self {
        self.metric = metric;
        self
    }

    pub fn element_type(mut self, element_type: ElementType) -> Self {
        self.element_type = element_type;
        self
    }

    pub fn class(mut self, class: NamedNode) -> Self {
        self.class = Some(class);
        self
    }

    /// Checks the name, the dimensions and the metric/element-type combination.
    pub fn validate(&self) -> Result<()> {
        let mut chars = self.name.chars();
        let ok = self.name.len() <= 64
            && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok {
            return Err(Error::Other(format!(
                "vector index name {:?} must match [A-Za-z][A-Za-z0-9_]{{0,63}}",
                self.name
            )));
        }
        if self.dimensions == 0 || self.dimensions > 65_536 {
            return Err(Error::Other(format!(
                "vector index {}: dimensions must be between 1 and 65536, not {}",
                self.name, self.dimensions
            )));
        }
        let sparse = self.element_type == ElementType::SparseFloat32;
        if (self.metric == Metric::Jaccard) != sparse {
            return Err(Error::Other(format!(
                "vector index {}: the Jaccard metric goes with sparse vectors, and sparse vectors with Jaccard",
                self.name
            )));
        }
        Ok(())
    }

    /// `<oxilite:vector/NAME>`.
    pub fn iri(&self) -> NamedNode {
        index_iri(&self.name)
    }

    /// The table holding the embeddings (lower case: SQLite names ignore case).
    pub fn table(&self) -> String {
        table_name(&self.name)
    }

    /// Everything the built table depends on.
    pub fn fingerprint(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.property.as_str(),
            self.dimensions,
            self.metric.name(),
            self.element_type.name(),
            self.class.as_ref().map_or("", NamedNode::as_str)
        )
    }

    /// The definition as quads of `<oxilite:vectors>`.
    pub fn to_quads(&self) -> Vec<Quad> {
        let g = GraphName::NamedNode(NamedNode::new_unchecked(VECTORS_GRAPH));
        let s = self.iri();
        let n = |i: &str| NamedNode::new_unchecked(i);
        let mut out = vec![
            Quad::new(s.clone(), rdf::TYPE, n(vocab::VECTOR_INDEX), g.clone()),
            Quad::new(
                s.clone(),
                n(vocab::INDEX_NAME),
                Literal::new_simple_literal(&self.name),
                g.clone(),
            ),
            Quad::new(
                s.clone(),
                n(vocab::PROPERTY),
                self.property.clone(),
                g.clone(),
            ),
            Quad::new(
                s.clone(),
                n(vocab::DIMENSIONS),
                Literal::from(i64::from(self.dimensions)),
                g.clone(),
            ),
            Quad::new(
                s.clone(),
                n(vocab::METRIC),
                n(&self.metric.iri()),
                g.clone(),
            ),
            Quad::new(
                s.clone(),
                n(vocab::ELEMENT_TYPE),
                n(&self.element_type.iri()),
                g.clone(),
            ),
        ];
        if let Some(c) = &self.class {
            out.push(Quad::new(s, n(vocab::CLASS), c.clone(), g));
        }
        out
    }

    /// Statements creating the table, its triggers (and IVF index), back-filling it and
    /// recording its fingerprint. Run them in one atomic request: a malformed existing value
    /// aborts the back-fill, and with it the whole creation.
    pub fn create_statements(&self, caps: &Capabilities) -> Result<Vec<Statement>> {
        require(caps)?;
        self.validate()?;
        let t = self.table();
        let pid = named_node_id(self.property.as_str());
        let dims = self.dimensions;
        let conv = self.element_type.convert_fn();
        let msg = sql_str(&format!(
            "oxilite: vector index {} expects {dims} dimensions",
            self.name
        ));
        let check = |o: &str| {
            format!(
                "COALESCE(json_array_length((SELECT lex FROM terms WHERE id = {o})), -1) <> {dims}"
            )
        };
        let mut s = vec![
            Statement::new(format!(
                "CREATE TABLE IF NOT EXISTS {t} (s INTEGER NOT NULL, o INTEGER NOT NULL, g INTEGER NOT NULL, \
                 e BLOB NOT NULL, PRIMARY KEY (s, o, g))"
            )),
            Statement::new(format!(
                "CREATE TRIGGER IF NOT EXISTS {t}_ins AFTER INSERT ON quads WHEN NEW.p = {pid} BEGIN \
                 SELECT RAISE(ABORT, {msg}) WHERE {}; \
                 INSERT OR IGNORE INTO {t}(s, o, g, e) SELECT NEW.s, NEW.o, NEW.g, {conv}(lex) FROM terms WHERE id = NEW.o; \
                 END",
                check("NEW.o")
            )),
            Statement::new(format!(
                "CREATE TRIGGER IF NOT EXISTS {t}_del AFTER DELETE ON quads WHEN OLD.p = {pid} BEGIN \
                 DELETE FROM {t} WHERE s = OLD.s AND o = OLD.o AND g = OLD.g; END"
            )),
            // Back-fill: the check first, so bad data aborts before anything is copied (RAISE
            // only exists in triggers; a CHECK constraint aborts a plain statement).
            Statement::new(GUARD_TABLE),
            Statement::new(format!(
                "INSERT INTO oxilite_vector_guard(vector_dimensions_mismatch) \
                 SELECT {} FROM quads q WHERE q.p = {pid} AND {} LIMIT 1",
                sql_str(&self.name),
                check("q.o")
            )),
            Statement::new(format!(
                "INSERT OR IGNORE INTO {t}(s, o, g, e) SELECT q.s, q.o, q.g, {conv}(t.lex) \
                 FROM quads q JOIN terms t ON t.id = q.o WHERE q.p = {pid}"
            )),
        ];
        if self.element_type == ElementType::SparseFloat32 && caps.vector_index_methods {
            s.push(Statement::new(format!(
                "CREATE INDEX IF NOT EXISTS {t}_ivf ON {t} USING toy_vector_sparse_ivf (e)"
            )));
        }
        s.push(Statement::new(format!(
            "INSERT OR REPLACE INTO oxilite_meta(key, value) VALUES ({}, {})",
            sql_str(&format!("{META_PREFIX}{}", self.name.to_lowercase())),
            sql_str(&self.fingerprint())
        )));
        Ok(s)
    }

    /// The nearest-neighbour statement for this index (see [`knn_sql`]).
    pub fn knn_sql(&self, query: &QueryVector, k: u64) -> Result<String> {
        knn_sql(self, query, k)
    }
}

/// `<oxilite:vector/NAME>`.
pub fn index_iri(name: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{INDEX_PREFIX}{name}"))
}

fn table_name(name: &str) -> String {
    format!("vec_{}", name.to_lowercase())
}

fn require(caps: &Capabilities) -> Result<()> {
    if caps.vectors {
        Ok(())
    } else {
        Err(Error::unsupported(format!(
            "vector indexes need a backend with vector functions, such as Turso (oxilite-turso); {} has none",
            caps.name
        )))
    }
}

/// Statements dropping an index's table, triggers and fingerprint (by lower-case name).
pub fn drop_statements(name: &str) -> Vec<Statement> {
    let t = table_name(name);
    vec![
        Statement::new(format!("DROP TRIGGER IF EXISTS {t}_ins")),
        Statement::new(format!("DROP TRIGGER IF EXISTS {t}_del")),
        Statement::new(format!("DROP INDEX IF EXISTS {t}_ivf")),
        Statement::new(format!("DROP TABLE IF EXISTS {t}")),
        Statement::new(format!(
            "DELETE FROM oxilite_meta WHERE key = {}",
            sql_str(&format!("{META_PREFIX}{}", name.to_lowercase()))
        )),
    ]
}

/// What a search is near.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryVector {
    /// A vector, as the JSON array text the index stores (`"[0.1, 0.2]"`).
    Vector(String),
    /// A node: its stored embedding is the query.
    Node(Term),
}

impl QueryVector {
    /// A vector of numbers.
    pub fn vector(values: &[f64]) -> Self {
        Self::Vector(format!(
            "[{}]",
            values
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ))
    }

    /// A node, whose stored embedding is used.
    pub fn node(node: impl Into<Term>) -> Self {
        Self::Node(node.into())
    }
}

/// Parses a JSON array of numbers, the lexical form of an embedding.
pub fn parse_vector(text: &str) -> Option<Vec<f64>> {
    let inner = text.trim().strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    inner
        .split(',')
        .map(|x| x.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect()
}

/// The nearest-neighbour statement: rows `(s, d)` — at most `k` distinct nodes (term ids), each
/// at its smallest distance, by increasing distance then id.
pub fn knn_sql(index: &VectorIndex, query: &QueryVector, k: u64) -> Result<String> {
    if k == 0 || k > MAX_K {
        return Err(Error::Other(format!(
            "vector search: k must be between 1 and {MAX_K}, not {k}"
        )));
    }
    let t = index.table();
    let q = match query {
        QueryVector::Vector(text) => {
            let v = parse_vector(text).ok_or_else(|| {
                Error::Other(format!(
                    "vector search on {}: the query {text:?} is not a JSON array of numbers",
                    index.name
                ))
            })?;
            if v.len() != index.dimensions as usize {
                return Err(Error::Other(format!(
                    "vector search on {}: the query has {} dimensions, the index {}",
                    index.name,
                    v.len(),
                    index.dimensions
                )));
            }
            format!("{}({})", index.element_type.convert_fn(), sql_str(text))
        }
        QueryVector::Node(node) => format!(
            "(SELECT e FROM {t} WHERE s = {} LIMIT 1)",
            crate::encoding::term_id(node.as_ref())
        ),
    };
    let dist = index.metric.distance_fn();
    let inner = match &index.class {
        None if index.element_type == ElementType::SparseFloat32 => {
            // The shape the IVF index method recognises.
            format!("SELECT s, {dist}(e, {q}) AS d FROM {t} ORDER BY d LIMIT {k}")
        }
        None => format!("SELECT v.s AS s, {dist}(v.e, {q}) AS d FROM {t} v"),
        Some(class) => format!(
            "SELECT v.s AS s, {dist}(v.e, {q}) AS d FROM {t} v WHERE EXISTS (SELECT 1 FROM quads c \
             WHERE c.s = v.s AND c.p = {} AND c.o = {})",
            rdf_type_id(),
            named_node_id(class.as_str())
        ),
    };
    Ok(format!(
        "SELECT s, MIN(d) AS d FROM ({inner}) GROUP BY s ORDER BY d, s LIMIT {k}"
    ))
}

/// [`knn_sql`] with a 1-based rank column `r` (ties broken by id), for relational frontends
/// that carry term ids rather than computed values.
pub fn knn_ranked_sql(index: &VectorIndex, query: &QueryVector, k: u64) -> Result<String> {
    let knn = knn_sql(index, query, k)?;
    Ok(format!(
        "WITH knn AS ({knn}) SELECT knn.s AS s, knn.d AS d, \
         (SELECT COUNT(*) FROM knn k2 WHERE k2.d < knn.d OR (k2.d = knn.d AND k2.s < knn.s)) + 1 AS r FROM knn"
    ))
}

/// The statement reading the definitions: every quad of `<oxilite:vectors>` with the text of its
/// terms (see [`definitions_from_rows`]).
pub fn definitions_statement(id_col: impl Fn(&str) -> String) -> Statement {
    Statement::new(format!(
        "SELECT {}, {}, {}, ts.lex, tp.lex, t.lex, t.dt, t.lang, t.dir FROM quads q \
         LEFT JOIN terms ts ON ts.id = q.s LEFT JOIN terms tp ON tp.id = q.p LEFT JOIN terms t ON t.id = q.o \
         WHERE q.g = {}",
        id_col("q.s"),
        id_col("q.p"),
        id_col("q.o"),
        named_node_id(VECTORS_GRAPH)
    ))
}

/// Definitions read from the rows of [`definitions_statement`]: the valid ones, and a message
/// for each description that is not one.
pub fn definitions_from_rows(
    rows: &[Vec<crate::sql::SqlValue>],
) -> (Vec<VectorIndex>, Vec<String>) {
    let mut by_subject: BTreeMap<String, Vec<(String, Term)>> = BTreeMap::new();
    for row in rows {
        let get = |i: usize| row.get(i).cloned().unwrap_or(crate::sql::SqlValue::Null);
        let (Some(_s), Some(_p), Some(o)) = (get(0).as_i64(), get(1).as_i64(), get(2).as_i64())
        else {
            continue;
        };
        let (Some(s), Some(p)) = (get(3).into_string(), get(4).into_string()) else {
            continue;
        };
        let term = match crate::encoding::decode_inline(o) {
            Some(t) => t,
            None => {
                let Some(lex) = get(5).into_string() else {
                    continue;
                };
                match crate::encoding::decode_row(
                    o,
                    lex,
                    get(6).into_string(),
                    get(7).into_string(),
                    get(8).as_i64(),
                ) {
                    Ok(t) => t,
                    Err(_) => continue,
                }
            }
        };
        by_subject.entry(s).or_default().push((p, term));
    }
    definitions_from_triples(by_subject)
}

/// Definitions read from quads of `<oxilite:vectors>` (other quads are ignored).
pub fn definitions_from_quads(quads: &[Quad]) -> (Vec<VectorIndex>, Vec<String>) {
    let mut by_subject: BTreeMap<String, Vec<(String, Term)>> = BTreeMap::new();
    for q in quads {
        if !matches!(&q.graph_name, GraphName::NamedNode(g) if g.as_str() == VECTORS_GRAPH) {
            continue;
        }
        if let oxrdf::NamedOrBlankNode::NamedNode(s) = &q.subject {
            by_subject
                .entry(s.as_str().to_owned())
                .or_default()
                .push((q.predicate.as_str().to_owned(), q.object.clone()));
        }
    }
    definitions_from_triples(by_subject)
}

fn definitions_from_triples(
    by_subject: BTreeMap<String, Vec<(String, Term)>>,
) -> (Vec<VectorIndex>, Vec<String>) {
    let mut defs: Vec<VectorIndex> = Vec::new();
    let mut problems = Vec::new();
    for (s, props) in by_subject {
        let is_index = props.iter().any(|(p, o)| {
            p == rdf::TYPE.as_str()
                && matches!(o, Term::NamedNode(n) if n.as_str() == vocab::VECTOR_INDEX)
        });
        if !is_index {
            continue;
        }
        match definition(&s, &props) {
            Ok(d) => {
                if let Some(other) = defs.iter().find(|x| x.table() == d.table()) {
                    problems.push(format!(
                        "<{s}>: index name {} clashes with {} (names are compared ignoring case)",
                        d.name, other.name
                    ));
                } else {
                    defs.push(d);
                }
            }
            Err(e) => problems.push(format!("<{s}>: {e}")),
        }
    }
    (defs, problems)
}

fn definition(s: &str, props: &[(String, Term)]) -> std::result::Result<VectorIndex, String> {
    let one = |p: &str| -> std::result::Result<Option<&Term>, String> {
        let mut it = props.iter().filter(|(k, _)| k == p).map(|(_, v)| v);
        let first = it.next();
        if it.next().is_some() {
            return Err(format!("more than one {}", crate::functions::local_name(p)));
        }
        Ok(first)
    };
    let from_iri = s
        .strip_prefix(INDEX_PREFIX)
        .ok_or_else(|| format!("an index must be named <{INDEX_PREFIX}NAME>"))?;
    let name = match one(vocab::INDEX_NAME)? {
        Some(Term::Literal(l)) => l.value().to_owned(),
        Some(_) => return Err("indexName must be a string".into()),
        None => from_iri.to_owned(),
    };
    if name != from_iri {
        return Err(format!(
            "indexName {name:?} differs from the name in the IRI ({from_iri:?})"
        ));
    }
    let property = match one(vocab::PROPERTY)? {
        Some(Term::NamedNode(n)) => n.clone(),
        Some(_) => return Err("property must be an IRI".into()),
        None => return Err("property is missing".into()),
    };
    let dimensions = match one(vocab::DIMENSIONS)? {
        Some(Term::Literal(l))
            if crate::encoding::numeric_rank(l.datatype().as_str()) == Some(1)
                || l.datatype() == xsd::INTEGER =>
        {
            l.value()
                .parse::<u32>()
                .map_err(|_| format!("dimensions {} is not a positive integer", l.value()))?
        }
        Some(_) => return Err("dimensions must be an integer".into()),
        None => return Err("dimensions is missing".into()),
    };
    let metric = match one(vocab::METRIC)? {
        Some(Term::NamedNode(n)) => {
            Metric::from_iri(n.as_str()).ok_or_else(|| format!("unknown metric <{n}>"))?
        }
        Some(_) => return Err("metric must be an IRI".into()),
        None => Metric::Cosine,
    };
    let element_type = match one(vocab::ELEMENT_TYPE)? {
        Some(Term::NamedNode(n)) => ElementType::from_iri(n.as_str())
            .ok_or_else(|| format!("unknown element type <{n}>"))?,
        Some(_) => return Err("elementType must be an IRI".into()),
        None if metric == Metric::Jaccard => ElementType::SparseFloat32,
        None => ElementType::Float32,
    };
    let class = match one(vocab::CLASS)? {
        Some(Term::NamedNode(n)) => Some(n.clone()),
        Some(_) => return Err("class must be an IRI".into()),
        None => None,
    };
    let d = VectorIndex {
        name,
        property,
        dimensions,
        metric,
        element_type,
        class,
    };
    d.validate().map_err(|e| e.to_string())?;
    Ok(d)
}

/// Built fingerprints by lower-case index name, from `oxilite_meta` rows `(key, value)`.
pub fn built_from_meta<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, String> {
    rows.into_iter()
        .filter_map(|(k, v)| Some((k.strip_prefix(META_PREFIX)?.to_owned(), v.to_owned())))
        .collect()
}

/// Is `name`'s table built from this exact definition?
pub fn is_built(index: &VectorIndex, built: &BTreeMap<String, String>) -> bool {
    built.get(&index.name.to_lowercase()) == Some(&index.fingerprint())
}

/// Statements making the built tables match the definitions: orphans and stale tables are
/// dropped, missing and stale ones created. Empty when everything is current.
pub fn sync_statements(
    defs: &[VectorIndex],
    built: &BTreeMap<String, String>,
    caps: &Capabilities,
) -> Result<Vec<Statement>> {
    let mut s = Vec::new();
    for name in built.keys() {
        if !defs.iter().any(|d| d.name.to_lowercase() == *name) {
            s.extend(drop_statements(name));
        }
    }
    for d in defs {
        if is_built(d, built) {
            continue;
        }
        if built.contains_key(&d.name.to_lowercase()) {
            s.extend(drop_statements(&d.name));
        }
        s.extend(d.create_statements(caps)?);
    }
    Ok(s)
}

/// Can this update change a vector index definition? Any triple written to `<oxilite:vectors>`
/// counts; with a variable graph, a triple counts when it could be a definition triple (a
/// variable predicate, an `oxl:` predicate, or `rdf:type` with a variable or `oxl:` class).
/// `LOAD`, `CLEAR` and `DROP` always count: re-checking is one read when nothing changed.
pub fn update_touches_vectors(update: &spargebra::Update) -> bool {
    use spargebra::term::{GraphName as G, GraphNamePattern, NamedNodePattern, TermPattern};
    use spargebra::GraphUpdateOperation as Op;
    let definition_triple = |p: &NamedNodePattern, o: &TermPattern| match p {
        NamedNodePattern::Variable(_) => true,
        NamedNodePattern::NamedNode(n) if n.as_ref() == rdf::TYPE => match o {
            TermPattern::NamedNode(c) => c.as_str().starts_with(NS),
            TermPattern::Variable(_) => true,
            _ => false,
        },
        NamedNodePattern::NamedNode(n) => n.as_str().starts_with(NS),
    };
    let counts = |g: &GraphNamePattern, p: &NamedNodePattern, o: &TermPattern| match g {
        GraphNamePattern::NamedNode(n) => n.as_str() == VECTORS_GRAPH,
        GraphNamePattern::DefaultGraph => false,
        GraphNamePattern::Variable(_) => definition_triple(p, o),
    };
    let data = |g: &G| matches!(g, G::NamedNode(n) if n.as_str() == VECTORS_GRAPH);
    update.operations.iter().any(|op| match op {
        Op::InsertData { data: d } => d.iter().any(|q| data(&q.graph_name)),
        Op::DeleteData { data: d } => d.iter().any(|q| {
            matches!(&q.graph_name, spargebra::term::GraphName::NamedNode(n) if n.as_str() == VECTORS_GRAPH)
        }),
        Op::DeleteInsert { delete, insert, .. } => {
            delete.iter().any(|q| {
                let o: TermPattern = q.object.clone().into();
                counts(&q.graph_name, &q.predicate, &o)
            }) || insert
                .iter()
                .any(|q| counts(&q.graph_name, &q.predicate, &q.object))
        }
        Op::Create { .. } => false,
        Op::Load { .. } | Op::Clear { .. } | Op::Drop { .. } => true,
    })
}

/// The index a `SERVICE` IRI names.
pub fn service_index(iri: &str) -> Option<&str> {
    iri.strip_prefix(INDEX_PREFIX)
}

/// Finds a definition by exact name, else ignoring case.
pub fn find<'a>(defs: &'a [VectorIndex], name: &str) -> Option<&'a VectorIndex> {
    defs.iter()
        .find(|d| d.name == name)
        .or_else(|| defs.iter().find(|d| d.name.eq_ignore_ascii_case(name)))
}

/// The id of the default graph, for callers building their own statements.
pub const DEFAULT_GRAPH: i64 = DEFAULT_GRAPH_ID;

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(l: &str) -> NamedNode {
        NamedNode::new_unchecked(format!("http://example.com/{l}"))
    }

    #[test]
    fn definitions_round_trip_through_rdf() {
        let d = VectorIndex::new("Docs", ex("embedding"), 3)
            .metric(Metric::Euclidean)
            .element_type(ElementType::Float64)
            .class(ex("Doc"));
        let (defs, problems) = definitions_from_quads(&d.to_quads());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(defs, vec![d]);
    }

    #[test]
    fn invalid_definitions() {
        assert!(VectorIndex::new("1x", ex("e"), 3).validate().is_err());
        assert!(VectorIndex::new("x", ex("e"), 0).validate().is_err());
        assert!(VectorIndex::new("x", ex("e"), 3)
            .metric(Metric::Jaccard)
            .validate()
            .is_err());
        let mut quads = VectorIndex::new("x", ex("e"), 3).to_quads();
        quads.retain(|q| q.predicate.as_str() != vocab::DIMENSIONS);
        let (defs, problems) = definitions_from_quads(&quads);
        assert!(defs.is_empty());
        assert!(
            problems[0].contains("dimensions is missing"),
            "{problems:?}"
        );
    }

    #[test]
    fn knn_checks_the_query() {
        let d = VectorIndex::new("x", ex("e"), 3);
        assert!(knn_sql(&d, &QueryVector::vector(&[1.0, 2.0]), 5)
            .unwrap_err()
            .to_string()
            .contains("2 dimensions"));
        assert!(knn_sql(&d, &QueryVector::Vector("nope".into()), 5).is_err());
        assert!(knn_sql(&d, &QueryVector::vector(&[1.0, 2.0, 3.0]), 0).is_err());
        let sql = knn_sql(&d, &QueryVector::vector(&[1.0, 2.0, 3.0]), 5).unwrap();
        assert!(
            sql.contains("vector_distance_cos(v.e, vector32('[1,2,3]'))"),
            "{sql}"
        );
    }

    #[test]
    fn sync_plans() {
        let caps = Capabilities {
            vectors: true,
            ..Capabilities::native()
        };
        let d = VectorIndex::new("x", ex("e"), 3);
        let mut built = BTreeMap::new();
        assert!(!sync_statements(std::slice::from_ref(&d), &built, &caps)
            .unwrap()
            .is_empty());
        built.insert("x".to_owned(), d.fingerprint());
        assert!(sync_statements(std::slice::from_ref(&d), &built, &caps)
            .unwrap()
            .is_empty());
        let drop = sync_statements(&[], &built, &caps).unwrap();
        assert!(drop.iter().any(|s| s.sql == "DROP TABLE IF EXISTS vec_x"));
        assert!(d.create_statements(&Capabilities::native()).is_err());
    }
}
