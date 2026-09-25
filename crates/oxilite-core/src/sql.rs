//! The sans-IO contract between oxilite and a SQLite-compatible engine.
//!
//! oxilite never talks to SQLite directly. Every operation produces [`Request`]s (lists of SQL
//! statements) and consumes [`Response`]s. A backend only has to run statements — natively
//! through rusqlite or a dlopen'ed `libsqlite3`, or remotely through Cloudflare D1's
//! `batch()` API.
//!
// @lat: [[architecture#Sans-IO core]]

use crate::error::{Error, Result};
use std::fmt::Write;

/// A SQL value, the subset of SQLite storage classes oxilite uses.
///
/// In JSON (JavaScript drivers, D1 over HTTP) it is `null`, a number or a string. The
/// deserializer is written by hand: an untagged derive breaks when another crate in the build
/// enables serde_json's `arbitrary_precision` (as the `ssi` crates behind `oxilite-vc` do).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for SqlValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = SqlValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("null, a number or a string")
            }
            fn visit_unit<E>(self) -> std::result::Result<SqlValue, E> {
                Ok(SqlValue::Null)
            }
            fn visit_none<E>(self) -> std::result::Result<SqlValue, E> {
                Ok(SqlValue::Null)
            }
            fn visit_some<D: serde::Deserializer<'de>>(
                self,
                d: D,
            ) -> std::result::Result<SqlValue, D::Error> {
                d.deserialize_any(self)
            }
            fn visit_bool<E>(self, v: bool) -> std::result::Result<SqlValue, E> {
                Ok(SqlValue::Integer(i64::from(v)))
            }
            fn visit_i64<E>(self, v: i64) -> std::result::Result<SqlValue, E> {
                Ok(SqlValue::Integer(v))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<SqlValue, E> {
                i64::try_from(v)
                    .map(SqlValue::Integer)
                    .or(Ok(SqlValue::Real(v as f64)))
            }
            fn visit_f64<E>(self, v: f64) -> std::result::Result<SqlValue, E> {
                Ok(SqlValue::Real(v))
            }
            fn visit_str<E>(self, v: &str) -> std::result::Result<SqlValue, E> {
                Ok(SqlValue::Text(v.to_owned()))
            }
            fn visit_string<E>(self, v: String) -> std::result::Result<SqlValue, E> {
                Ok(SqlValue::Text(v))
            }
            // serde_json with `arbitrary_precision` hands numbers over as a one-entry map.
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<SqlValue, A::Error> {
                let Some((_, n)) = map.next_entry::<String, String>()? else {
                    return Err(serde::de::Error::custom("empty map is not a SQL value"));
                };
                if let Ok(i) = n.parse::<i64>() {
                    return Ok(SqlValue::Integer(i));
                }
                n.parse::<f64>()
                    .map(SqlValue::Real)
                    .map_err(serde::de::Error::custom)
            }
        }
        d.deserialize_any(V)
    }
}

impl SqlValue {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(v) => Some(*v),
            // Backends that cannot transport 64-bit integers (D1 → JavaScript numbers) receive
            // ids as TEXT (see `Capabilities::int64_as_text`).
            Self::Text(v) => v.parse().ok(),
            Self::Real(v) if v.fract() == 0.0 && v.abs() < 9.0e15 => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(v) => Some(*v as f64),
            Self::Real(v) => Some(*v),
            Self::Text(v) => v.parse().ok(),
            Self::Null => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(v) => Some(v),
            _ => None,
        }
    }

    pub fn into_string(self) -> Option<String> {
        match self {
            Self::Text(v) => Some(v),
            Self::Integer(v) => Some(v.to_string()),
            Self::Real(v) => Some(v.to_string()),
            Self::Null => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

/// A single SQL statement. oxilite inlines all constants as SQL literals, so `params` is
/// almost always empty; this sidesteps D1's 100-bound-parameter limit.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Statement {
    pub sql: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub params: Vec<SqlValue>,
}

impl Statement {
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            params: Vec::new(),
        }
    }
}

impl From<String> for Statement {
    fn from(sql: String) -> Self {
        Self::new(sql)
    }
}

impl From<&str> for Statement {
    fn from(sql: &str) -> Self {
        Self::new(sql)
    }
}

/// How a request must be executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Mode {
    /// Read-only statements; may run outside a transaction.
    Read,
    /// All statements succeed or none does (one SQLite transaction, one D1 `batch()`).
    Atomic,
}

/// A group of statements sent to the backend in one round-trip.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Request {
    pub statements: Vec<Statement>,
    pub mode: Mode,
}

impl Request {
    pub fn read(statements: Vec<Statement>) -> Self {
        Self {
            statements,
            mode: Mode::Read,
        }
    }

    pub fn atomic(statements: Vec<Statement>) -> Self {
        Self {
            statements,
            mode: Mode::Atomic,
        }
    }
}

/// The result of one statement.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResultSet {
    #[cfg_attr(feature = "serde", serde(default))]
    pub rows: Vec<Vec<SqlValue>>,
    /// Rows modified by a write statement.
    #[cfg_attr(feature = "serde", serde(default))]
    pub changes: u64,
}

/// One [`ResultSet`] per statement of the [`Request`], in order.
pub type Response = Vec<ResultSet>;

/// What a backend can do. The compiler adapts SQL generation to it.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, rename_all = "camelCase"))]
pub struct Capabilities {
    /// Maximum length of one SQL statement in bytes (D1: 100 KB).
    pub max_sql_len: usize,
    /// Maximum number of statements in one request.
    pub max_statements: usize,
    /// The `oxilite_*` user-defined functions (regex, replace, hashes, Unicode case) exist.
    pub udf: bool,
    /// The backend supports interactive (read-then-write) transactions.
    pub interactive_transactions: bool,
    /// 64-bit integers must be returned as TEXT (JavaScript numbers lose precision above 2^53).
    pub int64_as_text: bool,
    /// Maximum number of terms in one compound SELECT (`UNION ALL` chain); D1 allows 5.
    pub max_compound_select: usize,
    /// A recursive CTE may have a compound recursive term (SQLite 3.34.0, 2020-12-01). A
    /// backend whose version cannot be established MUST leave this false: mutual recursion
    /// then takes a strategy that does not need it, instead of emitting SQL that would fail.
    pub compound_recursive_cte: bool,
    /// Name of the backend, for `explain()`.
    pub name: String,
    /// The versioning level of the store (set by the store at open, not by the backend):
    /// from `stamped` on, writers record the current tick in `quads.t`.
    pub versioning: crate::version::Versioning,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::native()
    }
}

impl Capabilities {
    /// A native SQLite linked in-process.
    pub fn native() -> Self {
        Self {
            max_sql_len: 1_000_000,
            max_statements: 10_000,
            udf: false,
            interactive_transactions: true,
            int64_as_text: false,
            max_compound_select: 500,
            // rusqlite bundles a current SQLite, and a dlopen'ed library is probed on open.
            compound_recursive_cte: true,
            name: "sqlite".into(),
            versioning: crate::version::Versioning::Off,
        }
    }

    /// Cloudflare D1 limits.
    pub fn d1() -> Self {
        Self {
            max_sql_len: 90_000,
            max_statements: 50,
            udf: false,
            interactive_transactions: false,
            int64_as_text: true,
            max_compound_select: 5,
            // D1's SQLite version is not ours to assume.
            compound_recursive_cte: false,
            name: "d1".into(),
            versioning: crate::version::Versioning::Off,
        }
    }
}

/// Writes a SQL string literal (single quotes doubled; NUL-containing strings as hex blobs).
pub fn quote_str(out: &mut String, s: &str) {
    if s.contains('\0') {
        out.push_str("CAST(X'");
        for b in s.as_bytes() {
            let _ = write!(out, "{b:02X}");
        }
        out.push_str("' AS TEXT)");
        return;
    }
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push('\'');
        }
        out.push(c);
    }
    out.push('\'');
}

/// Returns a SQL string literal.
pub fn sql_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    quote_str(&mut out, s);
    out
}

/// Formats an optional string as SQL.
pub fn sql_opt_str(s: Option<&str>) -> String {
    s.map_or_else(|| "NULL".into(), sql_str)
}

/// Formats a float as a SQL literal.
pub fn sql_f64(v: f64) -> String {
    if v.is_nan() {
        "NULL".into()
    } else if v == f64::INFINITY {
        "9e999".into()
    } else if v == f64::NEG_INFINITY {
        "-9e999".into()
    } else {
        let s = format!("{v:?}");
        if s.contains('.') || s.contains('e') || s.contains("inf") {
            s
        } else {
            format!("{s}.0")
        }
    }
}

/// `UNION ALL` of several SELECTs within the backend's compound-select limit: longer chains
/// are nested (`SELECT * FROM (a UNION ALL b …) UNION ALL …`).
pub fn union_all(mut parts: Vec<String>, max_terms: usize) -> String {
    let k = max_terms.max(2);
    while parts.len() > k {
        parts = parts
            .chunks(k)
            .map(|c| {
                if c.len() == 1 {
                    c[0].clone()
                } else {
                    format!("SELECT * FROM ({})", c.join(" UNION ALL "))
                }
            })
            .collect();
    }
    parts.join(" UNION ALL ")
}

/// Reads a column from a row.
pub fn col(row: &[SqlValue], i: usize) -> Result<&SqlValue> {
    row.get(i)
        .ok_or_else(|| Error::corrupted(format!("missing column {i} in result row")))
}

/// Checks that a response has the expected number of result sets.
pub fn expect_len(response: &Response, n: usize) -> Result<()> {
    if response.len() < n {
        return Err(Error::backend(format!(
            "backend returned {} result sets, expected {n}",
            response.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(sql_str("it's"), "'it''s'");
        assert_eq!(sql_str("a\0b"), "CAST(X'610062' AS TEXT)");
        assert_eq!(sql_f64(1.0), "1.0");
        assert_eq!(sql_f64(1e300), "1e300");
    }

    // @lat: [[tests#D1#SQL values survive arbitrary-precision JSON]]
    #[cfg(feature = "serde")]
    #[test]
    fn sql_values_survive_arbitrary_precision_json() {
        let plain: Vec<SqlValue> = serde_json::from_str(r#"[null, 7, 1.5, "x"]"#).unwrap();
        assert_eq!(
            plain,
            [
                SqlValue::Null,
                SqlValue::Integer(7),
                SqlValue::Real(1.5),
                SqlValue::Text("x".into())
            ]
        );
        // The shape serde_json hands numbers over in when `arbitrary_precision` is enabled.
        let token = r#"[{"$serde_json::private::Number": "1.5"}, {"$serde_json::private::Number": "9007199254740993"}]"#;
        let v: Vec<SqlValue> = serde_json::from_str(token).unwrap();
        assert_eq!(
            v,
            [
                SqlValue::Real(1.5),
                SqlValue::Integer(9_007_199_254_740_993)
            ]
        );
    }
}
