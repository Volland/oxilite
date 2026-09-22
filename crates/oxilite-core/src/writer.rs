//! Write path: turns quads into chunked, self-contained SQL statements.
//!
//! Term ids are computed in Rust, so an insert never needs to read anything back: terms and
//! quads go into the database in the same batch, with `INSERT OR IGNORE` making writes
//! idempotent. Statements are split to respect the backend's maximum SQL length.
//!
// @lat: [[architecture#Write path]]

use crate::encoding::{EncodedRows, DEFAULT_GRAPH_ID};
use crate::sql::{quote_str, sql_f64, Capabilities, Statement};
use oxrdf::QuadRef;
use std::fmt::Write;

/// Accumulates `VALUES` tuples and splits them into statements below a size limit.
struct Chunker<'a> {
    prefix: &'a str,
    suffix: &'a str,
    max: usize,
    current: String,
    out: Vec<Statement>,
}

impl<'a> Chunker<'a> {
    fn new(prefix: &'a str, suffix: &'a str, max: usize) -> Self {
        Self {
            prefix,
            suffix,
            max,
            current: String::new(),
            out: Vec::new(),
        }
    }

    fn push(&mut self, tuple: &str) {
        if !self.current.is_empty()
            && self.current.len() + tuple.len() + self.suffix.len() + 1 > self.max
        {
            self.flush();
        }
        if self.current.is_empty() {
            self.current.push_str(self.prefix);
        } else {
            self.current.push(',');
        }
        self.current.push_str(tuple);
    }

    fn flush(&mut self) {
        if !self.current.is_empty() {
            let mut sql = std::mem::take(&mut self.current);
            sql.push_str(self.suffix);
            self.out.push(Statement::new(sql));
        }
    }

    fn finish(mut self) -> Vec<Statement> {
        self.flush();
        self.out
    }
}

fn opt_str(out: &mut String, v: Option<&str>) {
    match v {
        Some(v) => quote_str(out, v),
        None => out.push_str("NULL"),
    }
}

/// Statements inserting the rows needed to decode encoded terms.
pub fn term_statements(rows: &EncodedRows, caps: &Capabilities) -> Vec<Statement> {
    let mut out = Vec::new();
    let mut terms = Chunker::new(
        "INSERT OR IGNORE INTO terms(id, lex, dt, lang, dir, num, nt, ts) VALUES ",
        "",
        caps.max_sql_len,
    );
    let mut tuple = String::new();
    for r in &rows.terms {
        tuple.clear();
        let _ = write!(tuple, "({},", r.id);
        quote_str(&mut tuple, &r.lex);
        tuple.push(',');
        opt_str(&mut tuple, r.dt.as_deref());
        tuple.push(',');
        opt_str(&mut tuple, r.lang.as_deref());
        let _ = write!(
            tuple,
            ",{},{},{},{})",
            r.dir.map_or_else(|| "NULL".into(), |v| v.to_string()),
            r.num.map_or_else(|| "NULL".into(), sql_f64),
            r.nt.map_or_else(|| "NULL".into(), |v| v.to_string()),
            r.ts.map_or_else(|| "NULL".into(), sql_f64),
        );
        terms.push(&tuple);
    }
    out.extend(terms.finish());
    let mut triples = Chunker::new(
        "INSERT OR IGNORE INTO triple_terms(id, s, p, o, vk, sk) VALUES ",
        "",
        caps.max_sql_len,
    );
    for t in &rows.triples {
        let mut tuple = format!("({},{},{},{},", t.id, t.s, t.p, t.o);
        quote_str(&mut tuple, &t.vk);
        tuple.push(',');
        quote_str(&mut tuple, &t.sk);
        tuple.push(')');
        triples.push(&tuple);
    }
    out.extend(triples.finish());
    out
}

/// Encoded quads ready to be written.
#[derive(Debug, Default)]
pub struct EncodedQuads {
    pub rows: EncodedRows,
    pub quads: Vec<[i64; 4]>,
}

impl EncodedQuads {
    pub fn new<'a>(quads: impl IntoIterator<Item = QuadRef<'a>>) -> Self {
        let mut me = Self::default();
        for q in quads {
            let ids = me.rows.quad(q);
            me.quads.push(ids);
        }
        me.rows.dedup();
        me
    }

    /// Statements inserting the quads (and their terms / graph names).
    pub fn insert_statements(&self, caps: &Capabilities) -> Vec<Statement> {
        let mut out = term_statements(&self.rows, caps);
        let mut graphs: Vec<i64> = self
            .quads
            .iter()
            .map(|q| q[3])
            .filter(|g| *g != DEFAULT_GRAPH_ID)
            .collect();
        graphs.sort_unstable();
        graphs.dedup();
        let mut g = Chunker::new(
            "INSERT OR IGNORE INTO graphs(id) VALUES ",
            "",
            caps.max_sql_len,
        );
        for id in graphs {
            g.push(&format!("({id})"));
        }
        out.extend(g.finish());
        out.extend(quad_insert_statements(&self.quads, caps));
        out
    }

    /// Statements deleting the quads.
    pub fn delete_statements(&self, caps: &Capabilities) -> Vec<Statement> {
        quad_delete_statements(&self.quads, caps)
    }
}

/// `INSERT OR IGNORE INTO quads` statements.
pub fn quad_insert_statements(quads: &[[i64; 4]], caps: &Capabilities) -> Vec<Statement> {
    insert_statements_into("quads", quads, caps)
}

/// `INSERT OR IGNORE INTO <table>(s, p, o, g)` statements (`quads` or `quads_inf`).
pub fn insert_statements_into(
    table: &str,
    quads: &[[i64; 4]],
    caps: &Capabilities,
) -> Vec<Statement> {
    let prefix = format!("INSERT OR IGNORE INTO {table}(s, p, o, g) VALUES ");
    let mut c = Chunker::new(&prefix, "", caps.max_sql_len);
    for [s, p, o, g] in quads {
        c.push(&format!("({s},{p},{o},{g})"));
    }
    c.finish()
}

/// `DELETE FROM quads` statements.
pub fn quad_delete_statements(quads: &[[i64; 4]], caps: &Capabilities) -> Vec<Statement> {
    if let [[s, p, o, g]] = quads {
        return vec![Statement::new(format!(
            "DELETE FROM quads WHERE s = {s} AND p = {p} AND o = {o} AND g = {g}"
        ))];
    }
    let mut c = Chunker::new(
        "DELETE FROM quads WHERE (s, p, o, g) IN (VALUES ",
        ")",
        caps.max_sql_len,
    );
    for [s, p, o, g] in quads {
        c.push(&format!("({s},{p},{o},{g})"));
    }
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{GraphName, Literal, NamedNode, Quad};

    // @lat: [[tests#Write path#Statements respect the size limit]]
    #[test]
    fn statements_respect_the_size_limit() {
        let quads: Vec<Quad> = (0..500)
            .map(|i| {
                Quad::new(
                    NamedNode::new_unchecked(format!("http://example.com/s{i}")),
                    NamedNode::new_unchecked("http://example.com/p"),
                    Literal::new_simple_literal(format!("value number {i} with 'quotes'")),
                    GraphName::DefaultGraph,
                )
            })
            .collect();
        let enc = EncodedQuads::new(quads.iter().map(Quad::as_ref));
        let caps = Capabilities {
            max_sql_len: 2_000,
            ..Capabilities::d1()
        };
        let stmts = enc.insert_statements(&caps);
        assert!(stmts.len() > 5);
        assert!(stmts.iter().all(|s| s.sql.len() <= 2_000));
        assert!(stmts.iter().all(|s| s.params.is_empty()));
    }
}
