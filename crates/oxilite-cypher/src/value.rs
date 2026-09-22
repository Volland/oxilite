//! Cypher values and their mapping to RDF terms.
//!
// @lat: [[architecture#Property graph frontend#Mapping]]

use crate::error::{CypherError, Result};

use oxrdf::{Literal, NamedNode, NamedOrBlankNode, Term};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

/// `rdf:JSON`, the datatype of list and map property values.
pub const RDF_JSON: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TemporalKind {
    Date,
    DateTime,
    LocalDateTime,
    Time,
    LocalTime,
    Duration,
}

impl TemporalKind {
    pub fn datatype(self) -> &'static str {
        match self {
            Self::Date => "http://www.w3.org/2001/XMLSchema#date",
            Self::DateTime | Self::LocalDateTime => "http://www.w3.org/2001/XMLSchema#dateTime",
            Self::Time | Self::LocalTime => "http://www.w3.org/2001/XMLSchema#time",
            Self::Duration => "http://www.w3.org/2001/XMLSchema#duration",
        }
    }
}

/// A node of the property graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: NamedOrBlankNode,
    pub labels: Vec<String>,
    pub properties: BTreeMap<String, Value>,
}

impl Node {
    pub fn element_id(&self) -> String {
        subject_str(&self.id)
    }
}

/// A relationship: an asserted triple, identified by its reifier when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Relationship {
    pub start: NamedOrBlankNode,
    pub predicate: NamedNode,
    pub end: NamedOrBlankNode,
    pub reifier: Option<NamedOrBlankNode>,
    pub rel_type: String,
    pub properties: BTreeMap<String, Value>,
}

impl Relationship {
    pub fn element_id(&self) -> String {
        match &self.reifier {
            Some(r) => subject_str(r),
            None => format!(
                "<<( {} <{}> {} )>>",
                subject_str(&self.start),
                self.predicate.as_str(),
                subject_str(&self.end)
            ),
        }
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        self.start == other.start
            && self.predicate == other.predicate
            && self.end == other.end
            && self.reifier == other.reifier
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub nodes: Vec<Node>,
    pub relationships: Vec<Relationship>,
}

/// A Cypher value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Temporal(TemporalKind, String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    Node(Node),
    Relationship(Relationship),
    Path(Path),
}

pub(crate) fn subject_str(s: &NamedOrBlankNode) -> String {
    match s {
        NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
    }
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "NULL",
            Self::Bool(_) => "BOOLEAN",
            Self::Int(_) => "INTEGER",
            Self::Float(_) => "FLOAT",
            Self::String(_) => "STRING",
            Self::Temporal(k, _) => match k {
                TemporalKind::Date => "DATE",
                TemporalKind::DateTime => "DATETIME",
                TemporalKind::LocalDateTime => "LOCALDATETIME",
                TemporalKind::Time => "TIME",
                TemporalKind::LocalTime => "LOCALTIME",
                TemporalKind::Duration => "DURATION",
            },
            Self::List(_) => "LIST",
            Self::Map(_) => "MAP",
            Self::Node(_) => "NODE",
            Self::Relationship(_) => "RELATIONSHIP",
            Self::Path(_) => "PATH",
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Converts a stored RDF literal to a Cypher value.
    pub fn from_literal(l: &Literal) -> Self {
        let dt = l.datatype().as_str();
        let lex = l.value();
        match dt {
            "http://www.w3.org/2001/XMLSchema#string"
            | "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"
            | "http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString" => {
                Self::String(lex.to_string())
            }
            "http://www.w3.org/2001/XMLSchema#boolean" => match lex {
                "true" | "1" => Self::Bool(true),
                "false" | "0" => Self::Bool(false),
                _ => Self::String(lex.to_string()),
            },
            "http://www.w3.org/2001/XMLSchema#integer"
            | "http://www.w3.org/2001/XMLSchema#long"
            | "http://www.w3.org/2001/XMLSchema#int"
            | "http://www.w3.org/2001/XMLSchema#short"
            | "http://www.w3.org/2001/XMLSchema#byte"
            | "http://www.w3.org/2001/XMLSchema#nonNegativeInteger"
            | "http://www.w3.org/2001/XMLSchema#positiveInteger"
            | "http://www.w3.org/2001/XMLSchema#nonPositiveInteger"
            | "http://www.w3.org/2001/XMLSchema#negativeInteger"
            | "http://www.w3.org/2001/XMLSchema#unsignedLong"
            | "http://www.w3.org/2001/XMLSchema#unsignedInt"
            | "http://www.w3.org/2001/XMLSchema#unsignedShort"
            | "http://www.w3.org/2001/XMLSchema#unsignedByte" => match lex.trim().parse::<i64>() {
                Ok(i) => Self::Int(i),
                Err(_) => lex
                    .trim()
                    .parse::<f64>()
                    .map_or_else(|_| Self::String(lex.to_string()), Self::Float),
            },
            "http://www.w3.org/2001/XMLSchema#decimal"
            | "http://www.w3.org/2001/XMLSchema#double"
            | "http://www.w3.org/2001/XMLSchema#float" => match lex.trim() {
                "INF" => Self::Float(f64::INFINITY),
                "-INF" => Self::Float(f64::NEG_INFINITY),
                "NaN" => Self::Float(f64::NAN),
                t => t
                    .parse::<f64>()
                    .map_or_else(|_| Self::String(lex.to_string()), Self::Float),
            },
            "http://www.w3.org/2001/XMLSchema#date"
            | "http://www.w3.org/2001/XMLSchema#time"
            | "http://www.w3.org/2001/XMLSchema#dateTime"
            | "http://www.w3.org/2001/XMLSchema#dateTimeStamp"
            | "http://www.w3.org/2001/XMLSchema#duration"
            | "http://www.w3.org/2001/XMLSchema#dayTimeDuration"
            | "http://www.w3.org/2001/XMLSchema#yearMonthDuration"
            | crate::temporal::ZONED_DATETIME => crate::temporal::from_literal(dt, lex)
                .unwrap_or_else(|| Self::String(lex.to_string())),
            RDF_JSON => serde_json::from_str::<serde_json::Value>(lex)
                .map_or_else(|_| Self::String(lex.to_string()), |j| Self::from_json(&j)),
            _ => Self::String(lex.to_string()),
        }
    }

    /// Converts a term returned by a SPARQL solution into a plain value (IRIs and blank nodes
    /// become their string form).
    pub fn from_term(t: &Term) -> Self {
        match t {
            Term::Literal(l) => Self::from_literal(l),
            Term::NamedNode(n) => Self::String(n.as_str().to_string()),
            Term::BlankNode(b) => Self::String(format!("_:{}", b.as_str())),
            Term::Triple(t) => Self::String(t.to_string()),
        }
    }

    /// The RDF literal storing this value as a property, or `None` for `null`.
    pub fn to_literal(&self) -> Result<Option<Literal>> {
        Ok(Some(match self {
            Self::Null => return Ok(None),
            Self::Bool(b) => Literal::from(*b),
            Self::Int(i) => Literal::from(*i),
            Self::Float(f) => Literal::from(*f),
            Self::String(s) => Literal::new_simple_literal(s.clone()),
            Self::Temporal(k, lex) => {
                let (l, dt) = crate::temporal::T::parse(*k, lex)?.lexical();
                Literal::new_typed_literal(l, NamedNode::new_unchecked(dt))
            }
            Self::List(items) if items.is_empty() => {
                Literal::new_typed_literal("[]", NamedNode::new_unchecked(RDF_JSON))
            }
            Self::List(_) | Self::Map(_) => Literal::new_typed_literal(
                self.to_json_storable()?.to_string(),
                NamedNode::new_unchecked(RDF_JSON),
            ),
            Self::Node(_) | Self::Relationship(_) | Self::Path(_) => {
                return Err(CypherError::runtime(format!(
                    "a {} cannot be stored as a property value",
                    self.type_name()
                )))
            }
        }))
    }

    fn to_json_storable(&self) -> Result<serde_json::Value> {
        Ok(match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(b) => (*b).into(),
            Self::Int(i) => (*i).into(),
            Self::Float(f) => serde_json::Number::from_f64(*f)
                .map_or(serde_json::Value::Null, serde_json::Value::Number),
            Self::String(s) | Self::Temporal(_, s) => s.clone().into(),
            Self::List(items) => serde_json::Value::Array(
                items
                    .iter()
                    .map(Self::to_json_storable)
                    .collect::<Result<_>>()?,
            ),
            Self::Map(m) => serde_json::Value::Object(
                m.iter()
                    .map(|(k, v)| Ok((k.clone(), v.to_json_storable()?)))
                    .collect::<Result<_>>()?,
            ),
            Self::Node(_) | Self::Relationship(_) | Self::Path(_) => {
                return Err(CypherError::runtime(format!(
                    "a {} cannot be stored in a property",
                    self.type_name()
                )))
            }
        })
    }

    pub fn from_json(j: &serde_json::Value) -> Self {
        match j {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Bool(*b),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => Self::Int(i),
                None => Self::Float(n.as_f64().unwrap_or(f64::NAN)),
            },
            serde_json::Value::String(s) => Self::String(s.clone()),
            serde_json::Value::Array(a) => Self::List(a.iter().map(Self::from_json).collect()),
            serde_json::Value::Object(o) => Self::Map(
                o.iter()
                    .map(|(k, v)| (k.clone(), Self::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// JSON form for bindings and result serialization.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        let props = |p: &BTreeMap<String, Value>| {
            serde_json::Value::Object(p.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
        };
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(b) => json!(b),
            Self::Int(i) => json!(i),
            Self::Float(f) => serde_json::Number::from_f64(*f)
                .map_or_else(|| json!(f.to_string()), serde_json::Value::Number),
            Self::String(s) => json!(s),
            Self::Temporal(k, s) => {
                json!({"type": self.type_name().to_lowercase(), "value": s, "kind": format!("{k:?}")})
            }
            Self::List(items) => {
                serde_json::Value::Array(items.iter().map(Self::to_json).collect())
            }
            Self::Map(m) => props(m),
            Self::Node(n) => json!({
                "type": "node",
                "id": n.element_id(),
                "labels": n.labels,
                "properties": props(&n.properties),
            }),
            Self::Relationship(r) => json!({
                "type": "relationship",
                "id": r.element_id(),
                "relType": r.rel_type,
                "start": subject_str(&r.start),
                "end": subject_str(&r.end),
                "properties": props(&r.properties),
            }),
            Self::Path(p) => json!({
                "type": "path",
                "nodes": p.nodes.iter().map(|n| Value::Node(n.clone()).to_json()).collect::<Vec<_>>(),
                "relationships": p.relationships.iter().map(|r| Value::Relationship(r.clone()).to_json()).collect::<Vec<_>>(),
            }),
        }
    }

    /// Cypher `=`: `None` is `null`.
    pub fn cypher_eq(&self, other: &Self) -> Option<bool> {
        match (self, other) {
            (Self::Null, _) | (_, Self::Null) => None,
            (Self::Int(a), Self::Int(b)) => Some(a == b),
            (Self::Int(_) | Self::Float(_), Self::Int(_) | Self::Float(_)) => {
                Some(self.as_f64() == other.as_f64())
            }
            (Self::Bool(a), Self::Bool(b)) => Some(a == b),
            (Self::String(a), Self::String(b)) => Some(a == b),
            (Self::Temporal(k1, a), Self::Temporal(k2, b)) => {
                match (
                    crate::temporal::T::parse(*k1, a),
                    crate::temporal::T::parse(*k2, b),
                ) {
                    (Ok(x), Ok(y)) => crate::temporal::equal(&x, &y),
                    _ => Some(k1 == k2 && a == b),
                }
            }
            (Self::List(a), Self::List(b)) => {
                if a.len() != b.len() {
                    return Some(false);
                }
                let mut unknown = false;
                for (x, y) in a.iter().zip(b) {
                    match x.cypher_eq(y) {
                        Some(false) => return Some(false),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown {
                    None
                } else {
                    Some(true)
                }
            }
            (Self::Map(a), Self::Map(b)) => {
                if a.len() != b.len() || a.keys().ne(b.keys()) {
                    return Some(false);
                }
                let mut unknown = false;
                for (x, y) in a.values().zip(b.values()) {
                    match x.cypher_eq(y) {
                        Some(false) => return Some(false),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown {
                    None
                } else {
                    Some(true)
                }
            }
            (Self::Node(a), Self::Node(b)) => Some(a.id == b.id),
            (Self::Relationship(a), Self::Relationship(b)) => Some(a.same(b)),
            (Self::Path(a), Self::Path(b)) => Some(
                a.nodes.len() == b.nodes.len()
                    && a.nodes.iter().zip(&b.nodes).all(|(x, y)| x.id == y.id)
                    && a.relationships.len() == b.relationships.len()
                    && a.relationships
                        .iter()
                        .zip(&b.relationships)
                        .all(|(x, y)| x.same(y)),
            ),
            _ => Some(false),
        }
    }

    /// Cypher `<` and friends: `None` when the values are not comparable.
    pub fn cypher_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => Some(a.cmp(b)),
            (Self::Int(_) | Self::Float(_), Self::Int(_) | Self::Float(_)) => {
                self.as_f64()?.partial_cmp(&other.as_f64()?)
            }
            (Self::String(a), Self::String(b)) => Some(a.cmp(b)),
            (Self::Bool(a), Self::Bool(b)) => Some(a.cmp(b)),
            (Self::Temporal(k1, a), Self::Temporal(k2, b)) => {
                let (x, y) = (
                    crate::temporal::T::parse(*k1, a).ok()?,
                    crate::temporal::T::parse(*k2, b).ok()?,
                );
                crate::temporal::compare(&x, &y)
            }
            (Self::List(a), Self::List(b)) => {
                for (x, y) in a.iter().zip(b) {
                    match x.cypher_cmp(y)? {
                        Ordering::Equal => {}
                        o => return Some(o),
                    }
                }
                Some(a.len().cmp(&b.len()))
            }
            _ => None,
        }
    }

    fn order_rank(&self) -> u8 {
        match self {
            Self::Map(_) => 0,
            Self::Node(_) => 1,
            Self::Relationship(_) => 2,
            Self::List(_) => 3,
            Self::Path(_) => 4,
            Self::Temporal(..) => 5,
            Self::String(_) => 6,
            Self::Bool(_) => 7,
            Self::Int(_) | Self::Float(_) => 8,
            Self::Null => 9,
        }
    }

    /// The total order of `ORDER BY` (nulls last when ascending).
    pub fn order(&self, other: &Self) -> Ordering {
        let (ra, rb) = (self.order_rank(), other.order_rank());
        if ra != rb {
            return ra.cmp(&rb);
        }
        match (self, other) {
            (Self::Int(_) | Self::Float(_), Self::Int(_) | Self::Float(_)) => {
                let (a, b) = (self.as_f64().unwrap_or(0.0), other.as_f64().unwrap_or(0.0));
                match (a.is_nan(), b.is_nan()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    _ => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
                }
            }
            (Self::List(a), Self::List(b)) => {
                for (x, y) in a.iter().zip(b) {
                    match x.order(y) {
                        Ordering::Equal => {}
                        o => return o,
                    }
                }
                a.len().cmp(&b.len())
            }
            (Self::Map(a), Self::Map(b)) => {
                let ka: Vec<_> = a.iter().collect();
                let kb: Vec<_> = b.iter().collect();
                for ((k1, v1), (k2, v2)) in ka.iter().zip(&kb) {
                    match k1.cmp(k2).then_with(|| v1.order(v2)) {
                        Ordering::Equal => {}
                        o => return o,
                    }
                }
                ka.len().cmp(&kb.len())
            }
            (Self::Node(a), Self::Node(b)) => subject_str(&a.id).cmp(&subject_str(&b.id)),
            (Self::Relationship(a), Self::Relationship(b)) => a.element_id().cmp(&b.element_id()),
            (Self::Path(a), Self::Path(b)) => a.relationships.len().cmp(&b.relationships.len()),
            _ => self.cypher_cmp(other).unwrap_or(Ordering::Equal),
        }
    }

    /// A key for grouping and `DISTINCT` (Cypher equivalence: `null` equals `null`, `1`
    /// equals `1.0`).
    pub fn group_key(&self) -> GroupKey {
        match self {
            Self::Null => GroupKey::Null,
            Self::Bool(b) => GroupKey::Bool(*b),
            Self::Int(i) => GroupKey::Int(*i),
            Self::Float(f) => {
                if f.fract() == 0.0 && f.abs() < 9.0e15 {
                    GroupKey::Int(*f as i64)
                } else if f.is_nan() {
                    GroupKey::NaN
                } else {
                    GroupKey::Float(f.to_bits())
                }
            }
            Self::String(s) => GroupKey::Str(s.clone()),
            Self::Temporal(k, s) => GroupKey::Temporal(*k, s.clone()),
            Self::List(items) => GroupKey::List(items.iter().map(Self::group_key).collect()),
            Self::Map(m) => {
                GroupKey::Map(m.iter().map(|(k, v)| (k.clone(), v.group_key())).collect())
            }
            Self::Node(n) => GroupKey::Entity(subject_str(&n.id)),
            Self::Relationship(r) => GroupKey::Entity(r.element_id()),
            Self::Path(p) => GroupKey::List(
                p.nodes
                    .iter()
                    .map(|n| GroupKey::Entity(subject_str(&n.id)))
                    .chain(
                        p.relationships
                            .iter()
                            .map(|r| GroupKey::Entity(r.element_id())),
                    )
                    .collect(),
            ),
        }
    }
}

/// See [`Value::group_key`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GroupKey {
    Null,
    Bool(bool),
    Int(i64),
    Float(u64),
    NaN,
    Str(String),
    Temporal(TemporalKind, String),
    List(Vec<GroupKey>),
    Map(Vec<(String, GroupKey)>),
    Entity(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "null"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(i) => write!(f, "{i}"),
            Self::Float(x) => {
                if x.fract() == 0.0 && x.is_finite() {
                    write!(f, "{x:.1}")
                } else {
                    write!(f, "{x}")
                }
            }
            Self::String(s) => write!(f, "'{s}'"),
            Self::Temporal(_, s) => write!(f, "'{s}'"),
            Self::List(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Self::Map(m) => {
                write!(f, "{{")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
            Self::Node(n) => {
                write!(f, "(")?;
                for l in &n.labels {
                    write!(f, ":{l}")?;
                }
                if !n.properties.is_empty() {
                    if !n.labels.is_empty() {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", Value::Map(n.properties.clone()))?;
                }
                write!(f, ")")
            }
            Self::Relationship(r) => {
                write!(f, "[:{}", r.rel_type)?;
                if !r.properties.is_empty() {
                    write!(f, " {}", Value::Map(r.properties.clone()))?;
                }
                write!(f, "]")
            }
            Self::Path(p) => {
                write!(f, "<")?;
                for (i, n) in p.nodes.iter().enumerate() {
                    if i > 0 {
                        let r = &p.relationships[i - 1];
                        if r.start == n.id {
                            write!(f, "<-{}-", Value::Relationship(r.clone()))?;
                        } else {
                            write!(f, "-{}->", Value::Relationship(r.clone()))?;
                        }
                    }
                    write!(f, "{}", Value::Node(n.clone()))?;
                }
                write!(f, ">")
            }
        }
    }
}

/// Parameters of a query, by name.
pub type Params = BTreeMap<String, Value>;

pub const REIFIES: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies";
