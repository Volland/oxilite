//! Vector indexes from Cypher, in Neo4j's syntax.
//!
//! `CALL db.index.vector.queryNodes(name, k, vector) YIELD node, score` is lowered like a
//! `MATCH`: the search is the SPARQL pattern `SERVICE <oxilite:vector/NAME> { … }`, `node` a node
//! binding and `score`/`distance` value bindings, so later clauses treat `node` like any matched
//! node and the whole statement stays one SQL query. `CREATE VECTOR INDEX`, `DROP INDEX` and
//! `SHOW VECTOR INDEXES` are schema commands ([`schema_command`]) that the store runs itself.
//!
// @lat: [[architecture#Vector indexes#Search]]

use crate::ast::Expr;
use crate::error::{CypherError, Result};
use crate::lexer::{tokenize, Tok};
use crate::lower::{Bind, Lowerer};
use crate::value::Value;
use oxilite_core::vector::{vocab, INDEX_PREFIX};
use oxrdf::{BlankNode, Literal, NamedNode, Variable};
use spargebra::algebra::GraphPattern;
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};

/// The procedure name (lower case, as the parser keeps it).
pub(crate) const QUERY_NODES: &str = "db.index.vector.querynodes";

fn triple(subject: TermPattern, predicate: NamedNodePattern, object: TermPattern) -> TriplePattern {
    TriplePattern {
        subject,
        predicate,
        object,
    }
}

/// The columns `db.index.vector.queryNodes` yields.
const COLUMNS: [&str; 3] = ["node", "score", "distance"];

/// The search pattern and the bindings of the yielded columns, by alias.
pub(crate) fn query_nodes(
    args: &[Expr],
    yields: &Option<Vec<(String, Option<String>)>>,
    lw: &mut Lowerer<'_>,
) -> Result<(GraphPattern, Vec<(String, Bind)>)> {
    let [name, k, query] = args else {
        return Err(CypherError::semantic(
            "db.index.vector.queryNodes(indexName, k, vector) takes three arguments",
        ));
    };
    let name = match lw.constant(name)? {
        Some(Value::String(s)) => s,
        _ => {
            return Err(CypherError::semantic(
                "the index name of db.index.vector.queryNodes must be a string",
            ))
        }
    };
    let k = match lw.constant(k)? {
        Some(Value::Int(k)) if k > 0 => k,
        _ => {
            return Err(CypherError::semantic(
                "k of db.index.vector.queryNodes must be a positive integer",
            ))
        }
    };
    let query = match lw.constant(query)? {
        Some(Value::List(items)) => {
            let mut nums = Vec::with_capacity(items.len());
            for i in items {
                nums.push(match i {
                    Value::Int(v) => v as f64,
                    Value::Float(v) => v,
                    _ => {
                        return Err(CypherError::semantic(
                            "the query vector must be a list of numbers",
                        ))
                    }
                });
            }
            TermPattern::Literal(Literal::new_simple_literal(format!(
                "[{}]",
                nums.iter()
                    .map(f64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )))
        }
        Some(Value::String(s)) if s.trim_start().starts_with('[') => {
            TermPattern::Literal(Literal::new_simple_literal(s))
        }
        // A node id or IRI: search near that node's own embedding.
        Some(Value::String(s)) => TermPattern::NamedNode(NamedNode::new(s.as_str()).map_err(|e| {
            CypherError::semantic(format!("{s} is neither a vector nor a node IRI: {e}"))
        })?),
        Some(Value::Node(n)) => match n.id {
            oxrdf::NamedOrBlankNode::NamedNode(x) => TermPattern::NamedNode(x),
            oxrdf::NamedOrBlankNode::BlankNode(_) => {
                return Err(CypherError::semantic(
                    "search near a node with an IRI (blank nodes have no stable id)",
                ))
            }
        },
        _ => {
            return Err(CypherError::semantic(
                "the query of db.index.vector.queryNodes must be a list of numbers, a parameter holding one, or a node IRI",
            ))
        }
    };
    let yields: Vec<(String, String)> = match yields {
        None => vec![
            ("node".into(), "node".into()),
            ("score".into(), "score".into()),
        ],
        Some(items) => {
            let mut out = Vec::new();
            for (col, alias) in items {
                if !COLUMNS.contains(&col.as_str()) {
                    return Err(CypherError::semantic(format!(
                        "procedure db.index.vector.queryNodes has no column `{col}` (node, score, distance)"
                    )));
                }
                out.push((col.clone(), alias.clone().unwrap_or_else(|| col.clone())));
            }
            out
        }
    };
    let n = |i: &str| NamedNodePattern::NamedNode(NamedNode::new_unchecked(i));
    let subject = TermPattern::BlankNode(BlankNode::default());
    let mut patterns = vec![
        triple(subject.clone(), n(vocab::QUERY), query),
        triple(
            subject.clone(),
            n(vocab::K),
            TermPattern::Literal(Literal::from(k)),
        ),
    ];
    let mut binds = Vec::new();
    for (col, alias) in yields {
        let var: Variable = lw.fresh(&col);
        let (pred, bind) = match col.as_str() {
            "node" => (
                vocab::NODE,
                Bind::Node {
                    var: var.clone(),
                    nullable: false,
                },
            ),
            "score" => (vocab::SCORE, value(var.clone())),
            _ => (vocab::DISTANCE, value(var.clone())),
        };
        patterns.push(triple(subject.clone(), n(pred), TermPattern::Variable(var)));
        binds.push((alias, bind));
    }
    Ok((
        GraphPattern::Service {
            name: NamedNodePattern::NamedNode(NamedNode::new_unchecked(format!(
                "{INDEX_PREFIX}{name}"
            ))),
            inner: Box::new(GraphPattern::Bgp { patterns }),
            silent: false,
        },
        binds,
    ))
}

fn value(var: Variable) -> Bind {
    Bind::Value {
        var,
        nullable: false,
        name: false,
    }
}

/// A Cypher schema command about vector indexes, run by the store rather than compiled.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaCommand {
    /// `CREATE VECTOR INDEX name [IF NOT EXISTS] FOR (n:Label) ON (n.prop) [OPTIONS {…}]`.
    CreateVectorIndex {
        name: String,
        if_not_exists: bool,
        /// The label (class) nodes must have, if any.
        label: Option<String>,
        /// The property key holding embeddings.
        property: String,
        /// `vector.dimensions`.
        dimensions: Option<u32>,
        /// `vector.similarity_function` (`cosine`, `euclidean`, `dot`, `jaccard`).
        similarity: Option<String>,
        /// `vector.element_type` (`float32`, `float64`, `int8`, `bit1`, `sparse`).
        element_type: Option<String>,
    },
    /// `DROP INDEX name [IF EXISTS]`.
    DropIndex { name: String, if_exists: bool },
    /// `SHOW VECTOR INDEX[ES]`.
    ShowVectorIndexes,
}

/// Recognizes a schema command; `None` for any other statement.
pub fn schema_command(src: &str) -> Result<Option<SchemaCommand>> {
    let toks: Vec<Tok> = match tokenize(src) {
        Ok(t) => t
            .into_iter()
            .map(|t| t.tok)
            .filter(|t| *t != Tok::Eof)
            .collect(),
        Err(_) => return Ok(None),
    };
    let mut p = P { toks, pos: 0 };
    if p.kw("SHOW") {
        if p.kw("VECTOR") && (p.kw("INDEXES") || p.kw("INDEX")) {
            p.end()?;
            return Ok(Some(SchemaCommand::ShowVectorIndexes));
        }
        return Ok(None);
    }
    if p.kw("DROP") {
        if !p.kw("INDEX") {
            return Ok(None);
        }
        let name = p.name("the index name")?;
        let if_exists = p.kw("IF") && p.expect_kw("EXISTS")?;
        p.end()?;
        return Ok(Some(SchemaCommand::DropIndex { name, if_exists }));
    }
    if !(p.kw("CREATE") && p.kw("VECTOR")) {
        return Ok(None);
    }
    p.expect_kw("INDEX")?;
    let name = p.name("the index name")?;
    let if_not_exists = p.kw("IF") && p.expect_kw("NOT")? && p.expect_kw("EXISTS")?;
    p.expect_kw("FOR")?;
    p.expect(&Tok::Sym("("), "`(`")?;
    let var = p.name("a node variable")?;
    let label = if p.eat(&Tok::Sym(":")) {
        Some(p.name("a label")?)
    } else {
        None
    };
    p.expect(&Tok::Sym(")"), "`)`")?;
    p.expect_kw("ON")?;
    p.expect(&Tok::Sym("("), "`(`")?;
    let v2 = p.name("the node variable")?;
    if v2 != var {
        return Err(CypherError::semantic(format!(
            "ON ({v2}.…) must use the variable of FOR ({var})"
        )));
    }
    p.expect(&Tok::Sym("."), "`.`")?;
    let property = p.name("a property key")?;
    p.expect(&Tok::Sym(")"), "`)`")?;
    let (mut dimensions, mut similarity, mut element_type) = (None, None, None);
    if p.kw("OPTIONS") {
        let options = p.map()?;
        let config = options
            .iter()
            .find(|(k, _)| k == "indexConfig")
            .map(|(_, v)| v.clone());
        let entries = match config {
            Some(Opt::Map(m)) => m,
            None => options,
            Some(_) => return Err(CypherError::semantic("indexConfig must be a map")),
        };
        for (k, v) in entries {
            match (k.as_str(), v) {
                ("vector.dimensions", Opt::Int(d)) if d > 0 => {
                    dimensions = Some(u32::try_from(d).map_err(|_| {
                        CypherError::semantic("vector.dimensions is too large")
                    })?)
                }
                ("vector.similarity_function", Opt::Str(s)) => similarity = Some(s),
                ("vector.element_type", Opt::Str(s)) => element_type = Some(s),
                (k, _) => {
                    return Err(CypherError::semantic(format!(
                        "unsupported vector index option `{k}` (vector.dimensions, vector.similarity_function, vector.element_type)"
                    )))
                }
            }
        }
    }
    p.end()?;
    Ok(Some(SchemaCommand::CreateVectorIndex {
        name,
        if_not_exists,
        label,
        property,
        dimensions,
        similarity,
        element_type,
    }))
}

#[derive(Debug, Clone)]
enum Opt {
    Int(i64),
    Str(String),
    Map(Vec<(String, Opt)>),
}

struct P {
    toks: Vec<Tok>,
    pos: usize,
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn word(&self) -> Option<&str> {
        match self.peek()? {
            Tok::Ident(s) | Tok::Quoted(s) => Some(s),
            _ => None,
        }
    }

    fn kw(&mut self, k: &str) -> bool {
        let hit = matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(k));
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn expect_kw(&mut self, k: &str) -> Result<bool> {
        if self.kw(k) {
            Ok(true)
        } else {
            Err(CypherError::semantic(format!("expected {k}")))
        }
    }

    fn expect(&mut self, t: &Tok, what: &str) -> Result<()> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(CypherError::semantic(format!("expected {what}")))
        }
    }

    fn name(&mut self, what: &str) -> Result<String> {
        let w = self
            .word()
            .map(str::to_owned)
            .ok_or_else(|| CypherError::semantic(format!("expected {what}")))?;
        self.pos += 1;
        Ok(w)
    }

    fn end(&mut self) -> Result<()> {
        self.eat(&Tok::Sym(";"));
        if self.peek().is_some() {
            return Err(CypherError::semantic(
                "unexpected text after the schema command",
            ));
        }
        Ok(())
    }

    fn map(&mut self) -> Result<Vec<(String, Opt)>> {
        self.expect(&Tok::Sym("{"), "`{`")?;
        let mut out = Vec::new();
        if self.eat(&Tok::Sym("}")) {
            return Ok(out);
        }
        loop {
            let key = match self.peek() {
                Some(Tok::Ident(s) | Tok::Quoted(s) | Tok::Str(s)) => s.clone(),
                _ => return Err(CypherError::semantic("expected an option name")),
            };
            self.pos += 1;
            self.expect(&Tok::Sym(":"), "`:`")?;
            let value = match self.peek().cloned() {
                Some(Tok::Sym("{")) => Opt::Map(self.map()?),
                Some(Tok::Int(i)) => {
                    self.pos += 1;
                    Opt::Int(i)
                }
                Some(Tok::Str(s)) => {
                    self.pos += 1;
                    Opt::Str(s)
                }
                _ => {
                    return Err(CypherError::semantic(format!(
                        "unsupported value for `{key}`"
                    )))
                }
            };
            out.push((key, value));
            if !self.eat(&Tok::Sym(",")) {
                break;
            }
        }
        self.expect(&Tok::Sym("}"), "`}`")?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_schema_commands() {
        assert_eq!(
            schema_command(
                "CREATE VECTOR INDEX docs IF NOT EXISTS FOR (d:Doc) ON (d.embedding) \
                 OPTIONS { indexConfig: { `vector.dimensions`: 3, `vector.similarity_function`: 'euclidean' } }"
            )
            .unwrap(),
            Some(SchemaCommand::CreateVectorIndex {
                name: "docs".into(),
                if_not_exists: true,
                label: Some("Doc".into()),
                property: "embedding".into(),
                dimensions: Some(3),
                similarity: Some("euclidean".into()),
                element_type: None,
            })
        );
        assert_eq!(
            schema_command("DROP INDEX docs IF EXISTS").unwrap(),
            Some(SchemaCommand::DropIndex {
                name: "docs".into(),
                if_exists: true
            })
        );
        assert_eq!(
            schema_command("SHOW VECTOR INDEXES").unwrap(),
            Some(SchemaCommand::ShowVectorIndexes)
        );
        assert_eq!(schema_command("MATCH (n) RETURN n").unwrap(), None);
        assert_eq!(schema_command("CREATE (n:Doc)").unwrap(), None);
    }
}
