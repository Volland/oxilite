//! Term encoding: every RDF term maps to a tagged 64-bit integer id.
//!
//! Layout (the sign bit is always 0, so ids are positive SQLite INTEGERs):
//!
//! ```text
//!  63 | 62..59 | 58..0
//!   0 |  tag   | payload (hash, or inline value)
//! ```
//!
//! Hashed kinds store a 59-bit xxh3 hash of a canonical key and need a row in `terms`.
//! Inline kinds (canonical `xsd:integer` in ±2^58 and canonical `xsd:boolean`) carry their
//! value in the payload, need no `terms` row, and — because the integer payload is offset —
//! sort by value, so range filters on inline integers are id range scans.
//!
// @lat: [[architecture#Term encoding]]

use crate::error::{Error, Result};
use oxrdf::vocab::{rdf, xsd};
use oxrdf::{
    BaseDirection, BlankNode, GraphName, GraphNameRef, Literal, LiteralRef, NamedNode,
    NamedOrBlankNode, NamedOrBlankNodeRef, Quad, QuadRef, Term, TermRef, Triple, TripleRef,
};
use std::str::FromStr;
use xxhash_rust::xxh3::xxh3_64;

/// Number of bits used by the payload.
pub const PAYLOAD_BITS: u32 = 59;
/// Mask selecting the payload.
pub const PAYLOAD_MASK: i64 = (1_i64 << PAYLOAD_BITS) - 1;
/// Offset applied to inline integers so that ids sort by value.
pub const INT_OFFSET: i64 = 1_i64 << (PAYLOAD_BITS - 1);
/// Smallest inline integer.
pub const INT_MIN: i64 = -INT_OFFSET;
/// Largest inline integer.
pub const INT_MAX: i64 = INT_OFFSET - 1;

/// The id of the default graph.
pub const DEFAULT_GRAPH_ID: i64 = 0;

/// Term kinds, stored in the 4 tag bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Tag {
    /// Reserved: the default graph (id 0).
    Default = 0,
    /// IRI (hashed).
    Iri = 1,
    /// Blank node (hashed).
    BlankNode = 2,
    /// Simple literal / `xsd:string` (hashed).
    String = 3,
    /// Language-tagged string (hashed).
    LangString = 4,
    /// Any other typed literal, including non-canonical or out-of-range numbers (hashed).
    Typed = 5,
    /// Canonical `xsd:integer` in ±2^58 (inline).
    Integer = 6,
    /// Canonical `xsd:boolean` (inline).
    Boolean = 7,
    /// RDF 1.2 triple term (hashed, components in `triple_terms`).
    Triple = 9,
    /// RDF 1.2 directional language-tagged string (hashed).
    DirLangString = 10,
}

impl Tag {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Default,
            1 => Self::Iri,
            2 => Self::BlankNode,
            3 => Self::String,
            4 => Self::LangString,
            5 => Self::Typed,
            6 => Self::Integer,
            7 => Self::Boolean,
            9 => Self::Triple,
            10 => Self::DirLangString,
            _ => return None,
        })
    }

    /// The first id carrying this tag.
    pub const fn base(self) -> i64 {
        (self as i64) << PAYLOAD_BITS
    }

    /// Does this kind of term need a row in `terms`?
    pub const fn is_hashed(self) -> bool {
        matches!(
            self,
            Self::Iri
                | Self::BlankNode
                | Self::String
                | Self::LangString
                | Self::Typed
                | Self::DirLangString
        )
    }

    /// Is this kind of term a literal?
    pub const fn is_literal(self) -> bool {
        matches!(
            self,
            Self::String
                | Self::LangString
                | Self::Typed
                | Self::Integer
                | Self::Boolean
                | Self::DirLangString
        )
    }
}

/// Extracts the tag of an id.
pub fn tag_of(id: i64) -> Option<Tag> {
    Tag::from_u8(((id >> PAYLOAD_BITS) & 0xF) as u8)
}

fn make(tag: Tag, payload: i64) -> i64 {
    tag.base() | (payload & PAYLOAD_MASK)
}

fn hashed(tag: Tag, key: &[&[u8]]) -> i64 {
    let mut buf = Vec::with_capacity(key.iter().map(|k| k.len() + 1).sum::<usize>() + 1);
    buf.push(tag as u8);
    for (i, part) in key.iter().enumerate() {
        if i > 0 {
            buf.push(0);
        }
        buf.extend_from_slice(part);
    }
    make(tag, (xxh3_64(&buf) as i64) & PAYLOAD_MASK)
}

/// Builds the id of an inline integer, if it fits.
pub fn integer_id(value: i64) -> Option<i64> {
    (INT_MIN..=INT_MAX)
        .contains(&value)
        .then(|| make(Tag::Integer, value + INT_OFFSET))
}

/// Builds the id of a boolean.
pub fn boolean_id(value: bool) -> i64 {
    make(Tag::Boolean, i64::from(value))
}

/// Numeric type ranks used for SPARQL type promotion.
pub mod numeric_type {
    pub const INTEGER: i64 = 1;
    pub const DECIMAL: i64 = 2;
    pub const FLOAT: i64 = 3;
    pub const DOUBLE: i64 = 4;
}

/// The row stored in `terms` for a hashed term.
#[derive(Debug, Clone, PartialEq)]
pub struct TermRow {
    pub id: i64,
    /// IRI, blank node label, or literal lexical form.
    pub lex: String,
    /// Datatype IRI, only for [`Tag::Typed`].
    pub dt: Option<String>,
    /// Language tag, for [`Tag::LangString`] and [`Tag::DirLangString`].
    pub lang: Option<String>,
    /// Base direction (1 = ltr, 2 = rtl) for [`Tag::DirLangString`]; for date/time literals,
    /// whether the value has a timezone (1) or not (0).
    pub dir: Option<i64>,
    /// Numeric value for numeric datatypes, or 0/1 for non-canonical `xsd:boolean`.
    pub num: Option<f64>,
    /// Numeric type rank (see [`numeric_type`]).
    pub nt: Option<i64>,
    /// `xsd:dateTime` / `xsd:date` as seconds since the Unix epoch (timezone-normalized).
    pub ts: Option<f64>,
}

/// The row stored in `triple_terms` for an RDF 1.2 triple term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripleRow {
    pub id: i64,
    pub s: i64,
    pub p: i64,
    pub o: i64,
    /// Value key: equal for value-equal triple terms (`<<a b 1>> = <<a b 1.0>>`).
    pub vk: String,
    /// Sort key: orders triple terms like Oxigraph (subject, predicate, then object).
    pub sk: String,
}

/// Everything that must be written so that an encoded term can be decoded later.
#[derive(Debug, Default, Clone)]
pub struct EncodedRows {
    pub terms: Vec<TermRow>,
    pub triples: Vec<TripleRow>,
}

/// Encodes a named node.
pub fn named_node_id(iri: &str) -> i64 {
    hashed(Tag::Iri, &[iri.as_bytes()])
}

/// Encodes a blank node.
pub fn blank_node_id(label: &str) -> i64 {
    hashed(Tag::BlankNode, &[label.as_bytes()])
}

fn integer_family(dt: &str) -> bool {
    matches!(
        dt,
        "http://www.w3.org/2001/XMLSchema#integer"
            | "http://www.w3.org/2001/XMLSchema#int"
            | "http://www.w3.org/2001/XMLSchema#long"
            | "http://www.w3.org/2001/XMLSchema#short"
            | "http://www.w3.org/2001/XMLSchema#byte"
            | "http://www.w3.org/2001/XMLSchema#nonNegativeInteger"
            | "http://www.w3.org/2001/XMLSchema#positiveInteger"
            | "http://www.w3.org/2001/XMLSchema#nonPositiveInteger"
            | "http://www.w3.org/2001/XMLSchema#negativeInteger"
            | "http://www.w3.org/2001/XMLSchema#unsignedLong"
            | "http://www.w3.org/2001/XMLSchema#unsignedInt"
            | "http://www.w3.org/2001/XMLSchema#unsignedShort"
            | "http://www.w3.org/2001/XMLSchema#unsignedByte"
    )
}

/// Returns the numeric type rank of a datatype, if numeric.
pub fn numeric_rank(dt: &str) -> Option<i64> {
    if integer_family(dt) {
        Some(numeric_type::INTEGER)
    } else {
        match dt {
            "http://www.w3.org/2001/XMLSchema#decimal" => Some(numeric_type::DECIMAL),
            "http://www.w3.org/2001/XMLSchema#float" => Some(numeric_type::FLOAT),
            "http://www.w3.org/2001/XMLSchema#double" => Some(numeric_type::DOUBLE),
            _ => None,
        }
    }
}

fn parse_xsd_float(lex: &str) -> Option<f64> {
    match lex {
        "INF" | "+INF" => Some(f64::INFINITY),
        "-INF" => Some(f64::NEG_INFINITY),
        "NaN" => None, // SQLite stores NaN as NULL anyway
        _ => {
            let v = f64::from_str(lex.trim()).ok()?;
            v.is_finite().then_some(v)
        }
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn tz_seconds(tz: Option<oxsdatatypes::DayTimeDuration>) -> f64 {
    tz.map_or(0.0, |tz| (tz.hours() * 3600 + tz.minutes() * 60) as f64)
}

/// Seconds since the epoch of an `xsd:dateTime` or `xsd:date` lexical form (no timezone = UTC).
pub fn timestamp(lex: &str, dt: &str) -> Option<f64> {
    match dt {
        "http://www.w3.org/2001/XMLSchema#dateTime"
        | "http://www.w3.org/2001/XMLSchema#dateTimeStamp" => {
            let v = oxsdatatypes::DateTime::from_str(lex).ok()?;
            let days = days_from_civil(v.year(), v.month().into(), v.day().into());
            let secs: f64 = f64::from(oxsdatatypes::Double::from(v.second()));
            Some(
                days as f64 * 86_400.0
                    + f64::from(v.hour()) * 3600.0
                    + f64::from(v.minute()) * 60.0
                    + secs
                    - tz_seconds(v.timezone()),
            )
        }
        "http://www.w3.org/2001/XMLSchema#date" => {
            let v = oxsdatatypes::Date::from_str(lex).ok()?;
            let days = days_from_civil(v.year(), v.month().into(), v.day().into());
            Some(days as f64 * 86_400.0 - tz_seconds(v.timezone()))
        }
        _ => None,
    }
}

/// For `xsd:dateTime` / `xsd:date` values: 1 if the lexical form has a timezone, else 0.
pub fn timezone_flag(lex: &str, dt: &str) -> Option<i64> {
    timestamp(lex, dt)?;
    let tz = lex.ends_with('Z')
        || (lex.len() > 6 && {
            let b = lex.as_bytes();
            let n = b.len();
            (b[n - 6] == b'+' || b[n - 6] == b'-') && b[n - 3] == b':'
        });
    Some(i64::from(tz))
}

/// A string that is equal for value-equal terms (numbers by value, dates by instant): used to
/// compare triple terms with SPARQL `=` in one SQL comparison.
pub fn value_key(term: TermRef<'_>) -> String {
    match term {
        TermRef::NamedNode(n) => format!("i{}", named_node_id(n.as_str())),
        TermRef::BlankNode(b) => format!("b{}", blank_node_id(b.as_str())),
        TermRef::Literal(l) => {
            let (id, row) = encode_literal(l);
            match (tag_of(id), row) {
                (Some(Tag::Integer), _) => format!("n{}", (id & PAYLOAD_MASK) - INT_OFFSET),
                (Some(Tag::Boolean), _) => format!("B{}", id & 1),
                (_, Some(r)) => match (r.num, r.nt, r.ts) {
                    (Some(n), Some(_), _) if n.fract() == 0.0 && n.abs() < 9.0e15 => {
                        format!("n{}", n as i64)
                    }
                    (Some(n), Some(_), _) => format!("n{n:?}"),
                    (Some(b), None, _) if l.datatype() == xsd::BOOLEAN => format!("B{}", b as i64),
                    (_, _, Some(ts)) => {
                        format!("t{}|{}|{ts:?}", l.datatype().as_str(), r.dir.unwrap_or(0))
                    }
                    _ => format!("l{id}"),
                },
                _ => format!("l{id}"),
            }
        }
        TermRef::Triple(t) => format!(
            "({} {} {})",
            value_key(t.subject.as_ref().into()),
            value_key(t.predicate.as_ref().into()),
            value_key(t.object.as_ref())
        ),
    }
}

/// A string whose byte order is the ORDER BY order of terms (blank nodes, IRIs, numbers by
/// value, other literals by lexical form, triple terms by components).
pub fn sort_key(term: TermRef<'_>) -> String {
    fn number(x: f64) -> String {
        // Order-preserving bit transform: lexicographic order of the hex = numeric order.
        let bits = x.to_bits();
        let key = if x.is_sign_negative() {
            !bits
        } else {
            bits | (1 << 63)
        };
        format!("{key:016x}")
    }
    match term {
        TermRef::BlankNode(b) => format!("0{}", b.as_str()),
        TermRef::NamedNode(n) => format!("1{}", n.as_str()),
        TermRef::Literal(l) => {
            let (id, row) = encode_literal(l);
            let num = match (tag_of(id), &row) {
                (Some(Tag::Integer), _) => Some(((id & PAYLOAD_MASK) - INT_OFFSET) as f64),
                (_, Some(r)) if r.nt.is_some() => r.num,
                _ => None,
            };
            match num {
                Some(n) => format!("2{}", number(n)),
                None => format!("3{}\u{1}{}", l.value(), l.datatype().as_str()),
            }
        }
        TermRef::Triple(t) => format!(
            "4{}\u{2}{}\u{2}{}",
            sort_key(t.subject.as_ref().into()),
            sort_key(t.predicate.as_ref().into()),
            sort_key(t.object.as_ref())
        ),
    }
}

/// Encodes a literal, returning its id and (for hashed literals) the row to store.
pub fn encode_literal(literal: LiteralRef<'_>) -> (i64, Option<TermRow>) {
    let lex = literal.value();
    if let Some(lang) = literal.language() {
        if let Some(dir) = literal.direction() {
            let d = match dir {
                BaseDirection::Ltr => 1,
                BaseDirection::Rtl => 2,
            };
            let id = hashed(
                Tag::DirLangString,
                &[
                    lang.as_bytes(),
                    if d == 1 { b"ltr" } else { b"rtl" },
                    lex.as_bytes(),
                ],
            );
            return (
                id,
                Some(TermRow {
                    id,
                    lex: lex.into(),
                    dt: None,
                    lang: Some(lang.into()),
                    dir: Some(d),
                    num: None,
                    nt: None,
                    ts: None,
                }),
            );
        }
        let id = hashed(Tag::LangString, &[lang.as_bytes(), lex.as_bytes()]);
        return (
            id,
            Some(TermRow {
                id,
                lex: lex.into(),
                dt: None,
                lang: Some(lang.into()),
                dir: None,
                num: None,
                nt: None,
                ts: None,
            }),
        );
    }
    let dt = literal.datatype();
    if dt == xsd::STRING {
        let id = hashed(Tag::String, &[lex.as_bytes()]);
        return (
            id,
            Some(TermRow {
                id,
                lex: lex.into(),
                dt: None,
                lang: None,
                dir: None,
                num: None,
                nt: None,
                ts: None,
            }),
        );
    }
    if dt == xsd::INTEGER {
        if let Ok(v) = i64::from_str(lex) {
            if v.to_string() == lex {
                if let Some(id) = integer_id(v) {
                    return (id, None);
                }
            }
        }
    }
    if dt == xsd::BOOLEAN {
        match lex {
            "true" => return (boolean_id(true), None),
            "false" => return (boolean_id(false), None),
            _ => {}
        }
    }
    let dt_str = dt.as_str();
    let id = hashed(Tag::Typed, &[dt_str.as_bytes(), lex.as_bytes()]);
    let nt = numeric_rank(dt_str);
    let mut num = nt.and_then(|_| parse_xsd_float(lex));
    if dt == xsd::BOOLEAN {
        num = match lex.trim() {
            "1" | "true" => Some(1.0),
            "0" | "false" => Some(0.0),
            _ => None,
        };
    }
    (
        id,
        Some(TermRow {
            id,
            lex: lex.into(),
            dt: Some(dt_str.into()),
            lang: None,
            dir: timezone_flag(lex, dt_str),
            num,
            nt: if num.is_some() { nt } else { None },
            ts: timestamp(lex, dt_str),
        }),
    )
}

/// Encodes a term without collecting rows (only the id).
pub fn term_id(term: TermRef<'_>) -> i64 {
    match term {
        TermRef::NamedNode(n) => named_node_id(n.as_str()),
        TermRef::BlankNode(b) => blank_node_id(b.as_str()),
        TermRef::Literal(l) => encode_literal(l).0,
        TermRef::Triple(t) => triple_id(t.as_ref()),
    }
}

/// Id of a triple term (hash of its component ids).
pub fn triple_id(t: TripleRef<'_>) -> i64 {
    let s = subject_id(t.subject);
    let p = named_node_id(t.predicate.as_str());
    let o = term_id(t.object);
    hashed(
        Tag::Triple,
        &[&s.to_be_bytes(), &p.to_be_bytes(), &o.to_be_bytes()],
    )
}

pub fn subject_id(s: NamedOrBlankNodeRef<'_>) -> i64 {
    match s {
        NamedOrBlankNodeRef::NamedNode(n) => named_node_id(n.as_str()),
        NamedOrBlankNodeRef::BlankNode(b) => blank_node_id(b.as_str()),
    }
}

pub fn graph_id(g: GraphNameRef<'_>) -> i64 {
    match g {
        GraphNameRef::DefaultGraph => DEFAULT_GRAPH_ID,
        GraphNameRef::NamedNode(n) => named_node_id(n.as_str()),
        GraphNameRef::BlankNode(b) => blank_node_id(b.as_str()),
    }
}

impl EncodedRows {
    /// Encodes a term, recording the rows needed to decode it.
    pub fn term(&mut self, term: TermRef<'_>) -> i64 {
        match term {
            TermRef::NamedNode(n) => self.iri(n.as_str()),
            TermRef::BlankNode(b) => self.bnode(b.as_str()),
            TermRef::Literal(l) => {
                let (id, row) = encode_literal(l);
                if let Some(row) = row {
                    self.terms.push(row);
                }
                id
            }
            TermRef::Triple(t) => self.triple(t.as_ref()),
        }
    }

    pub fn iri(&mut self, iri: &str) -> i64 {
        let id = named_node_id(iri);
        self.terms.push(TermRow {
            id,
            lex: iri.into(),
            dt: None,
            lang: None,
            dir: None,
            num: None,
            nt: None,
            ts: None,
        });
        id
    }

    pub fn bnode(&mut self, label: &str) -> i64 {
        let id = blank_node_id(label);
        self.terms.push(TermRow {
            id,
            lex: label.into(),
            dt: None,
            lang: None,
            dir: None,
            num: None,
            nt: None,
            ts: None,
        });
        id
    }

    pub fn subject(&mut self, s: NamedOrBlankNodeRef<'_>) -> i64 {
        match s {
            NamedOrBlankNodeRef::NamedNode(n) => self.iri(n.as_str()),
            NamedOrBlankNodeRef::BlankNode(b) => self.bnode(b.as_str()),
        }
    }

    pub fn graph(&mut self, g: GraphNameRef<'_>) -> i64 {
        match g {
            GraphNameRef::DefaultGraph => DEFAULT_GRAPH_ID,
            GraphNameRef::NamedNode(n) => self.iri(n.as_str()),
            GraphNameRef::BlankNode(b) => self.bnode(b.as_str()),
        }
    }

    pub fn triple(&mut self, t: TripleRef<'_>) -> i64 {
        let s = self.subject(t.subject);
        let p = self.iri(t.predicate.as_str());
        let o = self.term(t.object);
        let id = hashed(
            Tag::Triple,
            &[&s.to_be_bytes(), &p.to_be_bytes(), &o.to_be_bytes()],
        );
        let owned = t.into_owned();
        let vk = value_key(TermRef::Triple(&owned));
        let sk = sort_key(TermRef::Triple(&owned));
        self.triples.push(TripleRow {
            id,
            s,
            p,
            o,
            vk,
            sk,
        });
        id
    }

    /// Encodes a quad, returning `[s, p, o, g]`.
    pub fn quad(&mut self, q: QuadRef<'_>) -> [i64; 4] {
        [
            self.subject(q.subject),
            self.iri(q.predicate.as_str()),
            self.term(q.object),
            self.graph(q.graph_name),
        ]
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.triples.is_empty()
    }

    /// Sorts and removes duplicate rows (by id).
    pub fn dedup(&mut self) {
        self.terms.sort_by_key(|r| r.id);
        self.terms.dedup_by_key(|r| r.id);
        self.triples.sort_by_key(|r| r.id);
        self.triples.dedup_by_key(|r| r.id);
    }
}

/// Decodes an inline id (integer, boolean) without any lookup.
pub fn decode_inline(id: i64) -> Option<Term> {
    match tag_of(id)? {
        Tag::Integer => Some(Literal::from((id & PAYLOAD_MASK) - INT_OFFSET).into()),
        Tag::Boolean => Some(Literal::from((id & PAYLOAD_MASK) != 0).into()),
        _ => None,
    }
}

/// Decodes a hashed id from its `terms` row.
pub fn decode_row(
    id: i64,
    lex: String,
    dt: Option<String>,
    lang: Option<String>,
    dir: Option<i64>,
) -> Result<Term> {
    let tag = tag_of(id).ok_or_else(|| Error::corrupted(format!("invalid term id {id}")))?;
    Ok(match tag {
        Tag::Iri => NamedNode::new_unchecked(lex).into(),
        Tag::BlankNode => BlankNode::new_unchecked(lex).into(),
        Tag::String => Literal::new_simple_literal(lex).into(),
        Tag::LangString => Literal::new_language_tagged_literal_unchecked(
            lex,
            lang.ok_or_else(|| Error::corrupted("language tag missing"))?,
        )
        .into(),
        Tag::DirLangString => Literal::new_directional_language_tagged_literal_unchecked(
            lex,
            lang.ok_or_else(|| Error::corrupted("language tag missing"))?,
            if dir == Some(2) {
                BaseDirection::Rtl
            } else {
                BaseDirection::Ltr
            },
        )
        .into(),
        Tag::Typed => Literal::new_typed_literal(
            lex,
            NamedNode::new_unchecked(dt.ok_or_else(|| Error::corrupted("datatype missing"))?),
        )
        .into(),
        Tag::Integer | Tag::Boolean => {
            decode_inline(id).ok_or_else(|| Error::corrupted("bad inline term"))?
        }
        Tag::Triple | Tag::Default => {
            return Err(Error::corrupted(format!("id {id} is not a plain term")))
        }
    })
}

/// Rebuilds a triple from its decoded components.
pub fn make_triple(s: Term, p: Term, o: Term) -> Result<Triple> {
    let s = match s {
        Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
        Term::BlankNode(b) => NamedOrBlankNode::BlankNode(b),
        _ => return Err(Error::corrupted("invalid triple term subject")),
    };
    let Term::NamedNode(p) = p else {
        return Err(Error::corrupted("invalid triple term predicate"));
    };
    Ok(Triple::new(s, p, o))
}

/// Converts a decoded term into a subject.
pub fn to_subject(t: Term) -> Result<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Ok(n.into()),
        Term::BlankNode(b) => Ok(b.into()),
        _ => Err(Error::corrupted("invalid subject")),
    }
}

/// Converts a decoded term into a graph name.
pub fn to_graph_name(id: i64, t: Option<Term>) -> Result<GraphName> {
    if id == DEFAULT_GRAPH_ID {
        return Ok(GraphName::DefaultGraph);
    }
    match t {
        Some(Term::NamedNode(n)) => Ok(n.into()),
        Some(Term::BlankNode(b)) => Ok(b.into()),
        _ => Err(Error::corrupted("invalid graph name")),
    }
}

/// Builds a quad from decoded parts.
pub fn make_quad(s: Term, p: Term, o: Term, g: GraphName) -> Result<Quad> {
    let Term::NamedNode(p) = p else {
        return Err(Error::corrupted("invalid predicate"));
    };
    Ok(Quad::new(to_subject(s)?, p, o, g))
}

/// Well-known ids used by the planner and reasoner.
pub fn rdf_type_id() -> i64 {
    named_node_id(rdf::TYPE.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[tests#Encoding#Inline integers sort by value]]
    #[test]
    fn inline_integers_sort_by_value() {
        let ids: Vec<i64> = [-5_i64, -1, 0, 1, 42, 1_000_000]
            .iter()
            .map(|v| integer_id(*v).unwrap())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        assert!(ids.iter().all(|id| *id > 0));
        for v in [-5_i64, 0, 7, INT_MAX, INT_MIN] {
            let id = integer_id(v).unwrap();
            assert_eq!(decode_inline(id), Some(Literal::from(v).into()));
        }
        assert!(integer_id(INT_MAX + 1).is_none());
    }

    // @lat: [[tests#Encoding#Non-canonical literals are hashed]]
    #[test]
    fn non_canonical_literals_are_hashed() {
        let canonical = Literal::new_typed_literal("12", xsd::INTEGER);
        let padded = Literal::new_typed_literal("012", xsd::INTEGER);
        let (a, row_a) = encode_literal(canonical.as_ref());
        let (b, row_b) = encode_literal(padded.as_ref());
        assert_eq!(tag_of(a), Some(Tag::Integer));
        assert!(row_a.is_none());
        assert_eq!(tag_of(b), Some(Tag::Typed));
        let row_b = row_b.unwrap();
        assert_eq!(row_b.num, Some(12.0));
        assert_eq!(row_b.nt, Some(numeric_type::INTEGER));
        assert_ne!(a, b);
    }

    // @lat: [[tests#Encoding#Simple literal equals xsd string]]
    #[test]
    fn simple_literal_equals_xsd_string() {
        let a = Literal::new_simple_literal("abc");
        let b = Literal::new_typed_literal("abc", xsd::STRING);
        assert_eq!(encode_literal(a.as_ref()).0, encode_literal(b.as_ref()).0);
        let c = Literal::new_language_tagged_literal("abc", "en").unwrap();
        assert_ne!(encode_literal(a.as_ref()).0, encode_literal(c.as_ref()).0);
    }

    // @lat: [[tests#Encoding#Tags partition the id space]]
    #[test]
    fn tags_partition_the_id_space() {
        let iri = named_node_id("http://example.com/a");
        let bnode = blank_node_id("http://example.com/a");
        assert_eq!(tag_of(iri), Some(Tag::Iri));
        assert_eq!(tag_of(bnode), Some(Tag::BlankNode));
        assert!(iri >= Tag::Iri.base() && iri < Tag::BlankNode.base());
        assert_ne!(iri, bnode);
    }

    #[test]
    fn timestamps() {
        assert_eq!(
            timestamp("1970-01-02T00:00:00Z", xsd::DATE_TIME.as_str()),
            Some(86_400.0)
        );
        assert_eq!(
            timestamp("1970-01-01T02:00:00+02:00", xsd::DATE_TIME.as_str()),
            Some(0.0)
        );
        assert_eq!(
            timestamp("2000-03-01", xsd::DATE.as_str()),
            Some(951_868_800.0)
        );
    }
}
