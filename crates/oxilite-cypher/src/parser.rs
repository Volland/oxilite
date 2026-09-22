//! Recursive-descent parser for the supported openCypher subset.
//!
// @lat: [[architecture#Property graph frontend]]

use crate::ast::*;
use crate::error::{CypherError, Result};
use crate::lexer::{tokenize, Tok, Token};

/// Parses a Cypher query.
pub fn parse(src: &str) -> Result<Query> {
    let mut p = Parser {
        toks: tokenize(src)?,
        i: 0,
        src: src.chars().collect(),
    };
    let q = p.query()?;
    p.eat_sym(";");
    if !matches!(p.peek(), Tok::Eof) {
        return Err(p.err("unexpected input after the end of the query"));
    }
    Ok(q)
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
    src: Vec<char>,
}

const RESERVED: &[&str] = &[
    "MATCH", "OPTIONAL", "WHERE", "RETURN", "WITH", "UNWIND", "CREATE", "MERGE", "SET", "REMOVE",
    "DELETE", "DETACH", "ORDER", "SKIP", "LIMIT", "UNION", "CALL", "YIELD", "AS", "AND", "OR",
    "XOR", "NOT", "IN", "IS", "NULL", "TRUE", "FALSE", "CASE", "WHEN", "THEN", "ELSE", "END",
    "DISTINCT", "ON", "STARTS", "ENDS", "CONTAINS",
];

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.i + n).min(self.toks.len() - 1)].tok
    }

    fn pos(&self) -> usize {
        self.toks[self.i].pos
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.i].tok.clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn err(&self, message: impl Into<String>) -> CypherError {
        let found = match self.peek() {
            Tok::Eof => "end of input".to_string(),
            Tok::Ident(s) | Tok::Quoted(s) => format!("'{s}'"),
            Tok::Sym(s) => format!("'{s}'"),
            Tok::Str(s) => format!("string '{s}'"),
            Tok::Int(i) => i.to_string(),
            Tok::Float(f) => f.to_string(),
            Tok::Param(p) => format!("${p}"),
        };
        CypherError::syntax(self.pos(), format!("{} (found {found})", message.into()))
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s.eq_ignore_ascii_case(kw))
    }

    fn is_kw_at(&self, n: usize, kw: &str) -> bool {
        matches!(self.peek_at(n), Tok::Ident(s) if s.eq_ignore_ascii_case(kw))
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> Result<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(self.err(format!("expected {kw}")))
        }
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if *x == s)
    }

    fn is_sym_at(&self, n: usize, s: &str) -> bool {
        matches!(self.peek_at(n), Tok::Sym(x) if *x == s)
    }

    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_sym(&mut self, s: &str) -> Result<()> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            Err(self.err(format!("expected '{s}'")))
        }
    }

    /// A symbolic name (variable, label, type, key); keywords are allowed where unambiguous.
    fn name(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Ident(s) | Tok::Quoted(s) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.err("expected a name")),
        }
    }

    /// A variable name: not a reserved word.
    fn variable(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Quoted(s) => {
                self.bump();
                Ok(s)
            }
            Tok::Ident(s) if !RESERVED.iter().any(|k| k.eq_ignore_ascii_case(&s)) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.err("expected a variable")),
        }
    }

    fn at_variable(&self) -> bool {
        match self.peek() {
            Tok::Quoted(_) => true,
            Tok::Ident(s) => !RESERVED.iter().any(|k| k.eq_ignore_ascii_case(s)),
            _ => false,
        }
    }

    fn text(&self, from: usize, to: usize) -> String {
        self.src[from..to.min(self.src.len())]
            .iter()
            .collect::<String>()
            .trim()
            .to_string()
    }

    // ----- queries and clauses -----

    fn query(&mut self) -> Result<Query> {
        let mut parts = vec![self.single_query()?];
        let mut union_all = Vec::new();
        while self.eat_kw("UNION") {
            union_all.push(self.eat_kw("ALL"));
            parts.push(self.single_query()?);
        }
        Ok(Query { parts, union_all })
    }

    fn single_query(&mut self) -> Result<SingleQuery> {
        let mut clauses = Vec::new();
        loop {
            if self.is_kw("UNION") || matches!(self.peek(), Tok::Eof) || self.is_sym(";") {
                break;
            }
            if self.is_sym("}") {
                break;
            }
            clauses.push(self.clause()?);
        }
        if clauses.is_empty() {
            return Err(self.err("expected a clause"));
        }
        Ok(SingleQuery { clauses })
    }

    fn clause(&mut self) -> Result<Clause> {
        if self.eat_kw("OPTIONAL") {
            self.expect_kw("MATCH")?;
            return self.match_clause(true);
        }
        if self.eat_kw("MATCH") {
            return self.match_clause(false);
        }
        if self.eat_kw("UNWIND") {
            let expr = self.expr()?;
            self.expect_kw("AS")?;
            let alias = self.variable()?;
            return Ok(Clause::Unwind { expr, alias });
        }
        if self.eat_kw("WITH") {
            return Ok(Clause::With(self.projection(true)?));
        }
        if self.eat_kw("RETURN") {
            return Ok(Clause::Return(self.projection(false)?));
        }
        if self.eat_kw("CREATE") {
            return Ok(Clause::Create(self.pattern_list()?));
        }
        if self.eat_kw("MERGE") {
            let pattern = self.pattern_part()?;
            let (mut on_create, mut on_match) = (Vec::new(), Vec::new());
            while self.is_kw("ON") {
                self.bump();
                if self.eat_kw("CREATE") {
                    self.expect_kw("SET")?;
                    on_create.extend(self.set_items()?);
                } else if self.eat_kw("MATCH") {
                    self.expect_kw("SET")?;
                    on_match.extend(self.set_items()?);
                } else {
                    return Err(self.err("expected CREATE or MATCH after ON"));
                }
            }
            return Ok(Clause::Merge {
                pattern,
                on_create,
                on_match,
            });
        }
        if self.eat_kw("SET") {
            return Ok(Clause::Set(self.set_items()?));
        }
        if self.eat_kw("REMOVE") {
            let mut items = Vec::new();
            loop {
                let var = self.variable()?;
                if self.eat_sym(".") {
                    let key = self.name()?;
                    items.push(RemoveItem::Property { var, key });
                } else if self.is_sym(":") {
                    items.push(RemoveItem::Labels {
                        var,
                        labels: self.labels()?,
                    });
                } else {
                    return Err(self.err("expected .property or :Label"));
                }
                if !self.eat_sym(",") {
                    break;
                }
            }
            return Ok(Clause::Remove(items));
        }
        let detach = self.eat_kw("DETACH");
        if self.eat_kw("DELETE") {
            let mut exprs = vec![self.expr()?];
            while self.eat_sym(",") {
                exprs.push(self.expr()?);
            }
            return Ok(Clause::Delete { detach, exprs });
        }
        if detach {
            return Err(self.err("expected DELETE after DETACH"));
        }
        if self.eat_kw("CALL") {
            let mut procedure = self.name()?;
            while self.eat_sym(".") {
                procedure.push('.');
                procedure.push_str(&self.name()?);
            }
            let mut args = Vec::new();
            if self.eat_sym("(") {
                if !self.is_sym(")") {
                    args.push(self.expr()?);
                    while self.eat_sym(",") {
                        args.push(self.expr()?);
                    }
                }
                self.expect_sym(")")?;
            }
            let mut yields = None;
            let mut where_ = None;
            if self.eat_kw("YIELD") {
                let mut items = Vec::new();
                if !self.eat_sym("*") {
                    loop {
                        let col = self.name()?;
                        let alias = if self.eat_kw("AS") {
                            Some(self.variable()?)
                        } else {
                            None
                        };
                        items.push((col, alias));
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                    yields = Some(items);
                }
                if self.eat_kw("WHERE") {
                    where_ = Some(self.expr()?);
                }
            }
            return Ok(Clause::Call {
                procedure: procedure.to_lowercase(),
                args,
                yields,
                where_,
            });
        }
        Err(self.err("expected a clause (MATCH, RETURN, CREATE…)"))
    }

    fn match_clause(&mut self, optional: bool) -> Result<Clause> {
        let patterns = self.pattern_list()?;
        let where_ = if self.eat_kw("WHERE") {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(Clause::Match {
            optional,
            patterns,
            where_,
        })
    }

    fn projection(&mut self, with: bool) -> Result<Projection> {
        let distinct = self.eat_kw("DISTINCT");
        let mut star = false;
        let mut items = Vec::new();
        if self.eat_sym("*") {
            star = true;
            if !self.eat_sym(",") {
                return self.projection_tail(distinct, star, items, with);
            }
        }
        loop {
            let from = self.pos();
            let expr = self.expr()?;
            let to = self.pos();
            let alias = if self.eat_kw("AS") {
                Some(self.variable()?)
            } else {
                None
            };
            items.push(ProjectionItem {
                expr,
                alias,
                text: self.text(from, to),
            });
            if !self.eat_sym(",") {
                break;
            }
        }
        self.projection_tail(distinct, star, items, with)
    }

    fn projection_tail(
        &mut self,
        distinct: bool,
        star: bool,
        items: Vec<ProjectionItem>,
        with: bool,
    ) -> Result<Projection> {
        let mut order = Vec::new();
        if self.is_kw("ORDER") {
            self.bump();
            self.expect_kw("BY")?;
            loop {
                let e = self.expr()?;
                let asc = if self.eat_kw("DESC") || self.eat_kw("DESCENDING") {
                    false
                } else {
                    let _ = self.eat_kw("ASC") || self.eat_kw("ASCENDING");
                    true
                };
                order.push((e, asc));
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        let skip = if self.eat_kw("SKIP") {
            Some(self.expr()?)
        } else {
            None
        };
        let limit = if self.eat_kw("LIMIT") {
            Some(self.expr()?)
        } else {
            None
        };
        let where_ = if with && self.eat_kw("WHERE") {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(Projection {
            distinct,
            star,
            items,
            order,
            skip,
            limit,
            where_,
        })
    }

    fn set_items(&mut self) -> Result<Vec<SetItem>> {
        let mut items = Vec::new();
        loop {
            // `SET (n).name = …`
            let var = if self.is_sym("(") && self.is_sym_at(2, ")") {
                self.bump();
                let v = self.variable()?;
                self.expect_sym(")")?;
                v
            } else {
                self.variable()?
            };
            if self.eat_sym(".") {
                let key = self.name()?;
                self.expect_sym("=")?;
                let value = self.expr()?;
                items.push(SetItem::Property { var, key, value });
            } else if self.eat_sym("+=") {
                items.push(SetItem::Merge {
                    var,
                    value: self.expr()?,
                });
            } else if self.eat_sym("=") {
                items.push(SetItem::Replace {
                    var,
                    value: self.expr()?,
                });
            } else if self.is_sym(":") {
                items.push(SetItem::Labels {
                    var,
                    labels: self.labels()?,
                });
            } else {
                return Err(self.err("expected .property, =, += or :Label in SET"));
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(items)
    }

    fn labels(&mut self) -> Result<Vec<String>> {
        let mut labels = Vec::new();
        while self.eat_sym(":") {
            labels.push(self.name()?);
        }
        Ok(labels)
    }

    // ----- patterns -----

    fn pattern_list(&mut self) -> Result<Vec<PatternPart>> {
        let mut out = vec![self.pattern_part()?];
        while self.eat_sym(",") {
            out.push(self.pattern_part()?);
        }
        Ok(out)
    }

    fn pattern_part(&mut self) -> Result<PatternPart> {
        let var = if self.at_variable() && self.is_sym_at(1, "=") {
            let v = self.variable()?;
            self.bump();
            Some(v)
        } else {
            None
        };
        let shortest = if self.is_kw("shortestPath") && self.is_sym_at(1, "(") {
            Some(Shortest::One)
        } else if self.is_kw("allShortestPaths") && self.is_sym_at(1, "(") {
            Some(Shortest::All)
        } else {
            None
        };
        let element = if shortest.is_some() {
            self.bump();
            self.expect_sym("(")?;
            let e = self.pattern_element()?;
            self.expect_sym(")")?;
            e
        } else {
            self.pattern_element()?
        };
        Ok(PatternPart {
            var,
            shortest,
            element,
        })
    }

    fn pattern_element(&mut self) -> Result<PatternElement> {
        // Parenthesised pattern elements: `((a)-->(b))`.
        if self.is_sym("(") && self.is_sym_at(1, "(") {
            self.bump();
            let e = self.pattern_element()?;
            self.expect_sym(")")?;
            return Ok(e);
        }
        let start = self.node_pattern()?;
        let mut chain = Vec::new();
        while self.is_sym("-") || (self.is_sym("<") && self.is_sym_at(1, "-")) {
            let rel = self.rel_pattern()?;
            let node = self.node_pattern()?;
            chain.push((rel, node));
        }
        Ok(PatternElement { start, chain })
    }

    fn node_pattern(&mut self) -> Result<NodePattern> {
        self.expect_sym("(")?;
        let var = if self.at_variable() {
            Some(self.variable()?)
        } else {
            None
        };
        let labels = self.labels()?;
        let props = if self.is_sym("{") {
            Some(self.map_literal()?)
        } else if let Tok::Param(p) = self.peek().clone() {
            self.bump();
            Some(Expr::Param(p))
        } else {
            None
        };
        self.expect_sym(")")?;
        Ok(NodePattern { var, labels, props })
    }

    fn rel_pattern(&mut self) -> Result<RelPattern> {
        let left = self.eat_sym("<");
        self.expect_sym("-")?;
        let mut var = None;
        let mut types = Vec::new();
        let mut props = None;
        let mut length = None;
        if self.eat_sym("[") {
            if self.at_variable() {
                var = Some(self.variable()?);
            }
            if self.eat_sym(":") {
                types.push(self.name()?);
                while self.eat_sym("|") {
                    self.eat_sym(":");
                    types.push(self.name()?);
                }
            }
            if self.eat_sym("*") {
                let min = match self.peek().clone() {
                    Tok::Int(n) => {
                        self.bump();
                        Some(u32::try_from(n).map_err(|_| self.err("invalid length"))?)
                    }
                    _ => None,
                };
                if self.eat_sym("..") {
                    let max = match self.peek().clone() {
                        Tok::Int(n) => {
                            self.bump();
                            Some(u32::try_from(n).map_err(|_| self.err("invalid length"))?)
                        }
                        _ => None,
                    };
                    length = Some((min, max));
                } else {
                    // `*n` is exactly n hops; `*` alone is 1..unbounded.
                    length = Some((min, min));
                }
            }
            if self.is_sym("{") {
                props = Some(self.map_literal()?);
            } else if let Tok::Param(p) = self.peek().clone() {
                self.bump();
                props = Some(Expr::Param(p));
            }
            self.expect_sym("]")?;
        }
        self.expect_sym("-")?;
        let right = self.eat_sym(">");
        let dir = match (left, right) {
            (true, false) => Direction::Left,
            (false, true) => Direction::Right,
            (false, false) => Direction::Both,
            (true, true) => Direction::Both,
        };
        Ok(RelPattern {
            var,
            types,
            props,
            dir,
            length,
        })
    }

    fn map_literal(&mut self) -> Result<Expr> {
        self.expect_sym("{")?;
        let mut items = Vec::new();
        if !self.is_sym("}") {
            loop {
                let k = self.name()?;
                self.expect_sym(":")?;
                items.push((k, self.expr()?));
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        self.expect_sym("}")?;
        Ok(Expr::Map(items))
    }

    // ----- expressions -----

    pub(crate) fn expr(&mut self) -> Result<Expr> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr> {
        let mut e = self.xor_expr()?;
        while self.eat_kw("OR") {
            e = Expr::Binary(BinOp::Or, Box::new(e), Box::new(self.xor_expr()?));
        }
        Ok(e)
    }

    fn xor_expr(&mut self) -> Result<Expr> {
        let mut e = self.and_expr()?;
        while self.eat_kw("XOR") {
            e = Expr::Binary(BinOp::Xor, Box::new(e), Box::new(self.and_expr()?));
        }
        Ok(e)
    }

    fn and_expr(&mut self) -> Result<Expr> {
        let mut e = self.not_expr()?;
        while self.eat_kw("AND") {
            e = Expr::Binary(BinOp::And, Box::new(e), Box::new(self.not_expr()?));
        }
        Ok(e)
    }

    fn not_expr(&mut self) -> Result<Expr> {
        if self.eat_kw("NOT") {
            return Ok(Expr::Unary(UnOp::Not, Box::new(self.not_expr()?)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Expr> {
        let first = self.predicate_expr()?;
        let mut parts: Vec<Expr> = Vec::new();
        let mut prev = first.clone();
        let mut result = first;
        let mut chained = false;
        loop {
            let op = match self.peek() {
                Tok::Sym("=") => BinOp::Eq,
                Tok::Sym("<>") | Tok::Sym("!=") => BinOp::Ne,
                Tok::Sym("<") => BinOp::Lt,
                Tok::Sym("<=") => BinOp::Le,
                Tok::Sym(">") => BinOp::Gt,
                Tok::Sym(">=") => BinOp::Ge,
                Tok::Sym("=~") => BinOp::Regex,
                _ => break,
            };
            self.bump();
            let rhs = self.predicate_expr()?;
            let cmp = Expr::Binary(op, Box::new(prev.clone()), Box::new(rhs.clone()));
            if chained {
                parts.push(cmp);
            } else {
                parts = vec![cmp];
                chained = true;
            }
            prev = rhs;
        }
        if chained {
            // `a < b < c` means `a < b AND b < c`.
            let mut it = parts.into_iter();
            result = it.next().expect("one comparison");
            for p in it {
                result = Expr::Binary(BinOp::And, Box::new(result), Box::new(p));
            }
        }
        Ok(result)
    }

    fn predicate_expr(&mut self) -> Result<Expr> {
        let mut e = self.additive()?;
        loop {
            if self.is_kw("STARTS") && self.is_kw_at(1, "WITH") {
                self.bump();
                self.bump();
                e = Expr::Binary(BinOp::StartsWith, Box::new(e), Box::new(self.additive()?));
            } else if self.is_kw("ENDS") && self.is_kw_at(1, "WITH") {
                self.bump();
                self.bump();
                e = Expr::Binary(BinOp::EndsWith, Box::new(e), Box::new(self.additive()?));
            } else if self.eat_kw("CONTAINS") {
                e = Expr::Binary(BinOp::Contains, Box::new(e), Box::new(self.additive()?));
            } else if self.eat_kw("IN") {
                e = Expr::Binary(BinOp::In, Box::new(e), Box::new(self.additive()?));
            } else if self.is_kw("IS") {
                self.bump();
                let negated = self.eat_kw("NOT");
                self.expect_kw("NULL")?;
                e = Expr::IsNull(Box::new(e), negated);
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn additive(&mut self) -> Result<Expr> {
        let mut e = self.multiplicative()?;
        loop {
            let op = if self.is_sym("+") {
                BinOp::Add
            } else if self.is_sym("-") {
                BinOp::Sub
            } else {
                break;
            };
            self.bump();
            e = Expr::Binary(op, Box::new(e), Box::new(self.multiplicative()?));
        }
        Ok(e)
    }

    fn multiplicative(&mut self) -> Result<Expr> {
        let mut e = self.power()?;
        loop {
            let op = if self.is_sym("*") {
                BinOp::Mul
            } else if self.is_sym("/") {
                BinOp::Div
            } else if self.is_sym("%") {
                BinOp::Mod
            } else {
                break;
            };
            self.bump();
            e = Expr::Binary(op, Box::new(e), Box::new(self.power()?));
        }
        Ok(e)
    }

    fn power(&mut self) -> Result<Expr> {
        let mut e = self.unary()?;
        while self.eat_sym("^") {
            e = Expr::Binary(BinOp::Pow, Box::new(e), Box::new(self.unary()?));
        }
        Ok(e)
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.eat_sym("-") {
            return Ok(match self.unary()? {
                Expr::Int(i) => Expr::Int(-i),
                Expr::Float(f) => Expr::Float(-f),
                e => Expr::Unary(UnOp::Neg, Box::new(e)),
            });
        }
        if self.eat_sym("+") {
            return Ok(Expr::Unary(UnOp::Plus, Box::new(self.unary()?)));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.atom()?;
        loop {
            if self.is_sym(".") {
                self.bump();
                let key = self.name()?;
                e = Expr::Prop(Box::new(e), key);
            } else if self.is_sym("[") {
                self.bump();
                if self.eat_sym("..") {
                    let to = if self.is_sym("]") {
                        None
                    } else {
                        Some(Box::new(self.expr()?))
                    };
                    self.expect_sym("]")?;
                    e = Expr::Slice(Box::new(e), None, to);
                    continue;
                }
                let idx = self.expr()?;
                if self.eat_sym("..") {
                    let to = if self.is_sym("]") {
                        None
                    } else {
                        Some(Box::new(self.expr()?))
                    };
                    self.expect_sym("]")?;
                    e = Expr::Slice(Box::new(e), Some(Box::new(idx)), to);
                } else {
                    self.expect_sym("]")?;
                    e = Expr::Index(Box::new(e), Box::new(idx));
                }
            } else if self.is_sym(":") && matches!(e, Expr::Var(_) | Expr::Prop(..)) {
                e = Expr::HasLabels(Box::new(e), self.labels()?);
            } else if self.is_sym("{") {
                if let Expr::Var(v) = &e {
                    let var = v.clone();
                    e = self.map_projection(var)?;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn map_projection(&mut self, var: String) -> Result<Expr> {
        self.expect_sym("{")?;
        let mut items = Vec::new();
        if !self.is_sym("}") {
            loop {
                if self.eat_sym(".") {
                    if self.eat_sym("*") {
                        items.push(MapProjItem::AllProps);
                    } else {
                        items.push(MapProjItem::Prop(self.name()?));
                    }
                } else if self.is_sym_at(1, ":") {
                    let k = self.name()?;
                    self.bump();
                    items.push(MapProjItem::Entry(k, self.expr()?));
                } else {
                    items.push(MapProjItem::Var(self.variable()?));
                }
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        self.expect_sym("}")?;
        Ok(Expr::MapProjection { var, items })
    }

    fn try_pattern(&mut self) -> Option<PatternElement> {
        let save = self.i;
        match self.pattern_element() {
            Ok(p) if !p.chain.is_empty() => Some(p),
            _ => {
                self.i = save;
                None
            }
        }
    }

    fn atom(&mut self) -> Result<Expr> {
        match self.peek().clone() {
            Tok::Int(i) => {
                self.bump();
                Ok(Expr::Int(i))
            }
            Tok::Float(f) => {
                self.bump();
                Ok(Expr::Float(f))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::Param(p) => {
                self.bump();
                Ok(Expr::Param(p))
            }
            Tok::Sym("(") => {
                if let Some(p) = self.try_pattern() {
                    return Ok(Expr::Pattern(Box::new(p)));
                }
                self.bump();
                let e = self.expr()?;
                self.expect_sym(")")?;
                Ok(e)
            }
            Tok::Sym("[") => self.list_atom(),
            Tok::Sym("{") => self.map_literal(),
            Tok::Quoted(v) => {
                self.bump();
                Ok(Expr::Var(v))
            }
            Tok::Ident(word) => self.ident_atom(word),
            _ => Err(self.err("expected an expression")),
        }
    }

    fn list_atom(&mut self) -> Result<Expr> {
        self.expect_sym("[")?;
        // List comprehension: `[x IN list …]`.
        if self.at_variable() && self.is_kw_at(1, "IN") {
            let var = self.variable()?;
            self.bump();
            let list = self.expr()?;
            let filter = if self.eat_kw("WHERE") {
                Some(Box::new(self.expr()?))
            } else {
                None
            };
            let map = if self.eat_sym("|") {
                Some(Box::new(self.expr()?))
            } else {
                None
            };
            self.expect_sym("]")?;
            return Ok(Expr::ListComp {
                var,
                list: Box::new(list),
                filter,
                map,
            });
        }
        // Pattern comprehension: `[(a)-->(b) WHERE … | expr]`.
        if self.is_sym("(") || (self.at_variable() && self.is_sym_at(1, "=")) {
            let save = self.i;
            let mut var = None;
            if self.at_variable() && self.is_sym_at(1, "=") {
                var = Some(self.variable()?);
                self.bump();
            }
            if let Some(pattern) = self.try_pattern() {
                let filter = if self.eat_kw("WHERE") {
                    Some(Box::new(self.expr()?))
                } else {
                    None
                };
                if self.eat_sym("|") {
                    let map = self.expr()?;
                    self.expect_sym("]")?;
                    return Ok(Expr::PatternComp {
                        var,
                        pattern: Box::new(pattern),
                        filter,
                        map: Box::new(map),
                    });
                }
            }
            self.i = save;
        }
        let mut items = Vec::new();
        if !self.is_sym("]") {
            items.push(self.expr()?);
            while self.eat_sym(",") {
                items.push(self.expr()?);
            }
        }
        self.expect_sym("]")?;
        Ok(Expr::List(items))
    }

    fn ident_atom(&mut self, word: String) -> Result<Expr> {
        let upper = word.to_ascii_uppercase();
        match upper.as_str() {
            "TRUE" => {
                self.bump();
                return Ok(Expr::Bool(true));
            }
            "FALSE" => {
                self.bump();
                return Ok(Expr::Bool(false));
            }
            "NULL" => {
                self.bump();
                return Ok(Expr::Null);
            }
            "CASE" => return self.case_expr(),
            "EXISTS" if self.is_sym_at(1, "{") => {
                self.bump();
                self.bump();
                self.eat_kw("MATCH");
                let patterns = self.pattern_list()?;
                let where_ = if self.eat_kw("WHERE") {
                    Some(Box::new(self.expr()?))
                } else {
                    None
                };
                // `EXISTS { MATCH … RETURN … }`: the projection does not change existence.
                if self.eat_kw("RETURN") {
                    let _ = self.projection(false)?;
                }
                self.expect_sym("}")?;
                return Ok(Expr::Exists(patterns, where_));
            }
            "ALL" | "ANY" | "NONE" | "SINGLE"
                if self.is_sym_at(1, "(") && self.is_kw_at(3, "IN") =>
            {
                self.bump();
                self.bump();
                let var = self.variable()?;
                self.expect_kw("IN")?;
                let list = self.expr()?;
                self.expect_kw("WHERE")?;
                let pred = self.expr()?;
                self.expect_sym(")")?;
                let q = match upper.as_str() {
                    "ALL" => Quantifier::All,
                    "ANY" => Quantifier::Any,
                    "NONE" => Quantifier::None,
                    _ => Quantifier::Single,
                };
                return Ok(Expr::Quantified {
                    q,
                    var,
                    list: Box::new(list),
                    pred: Box::new(pred),
                });
            }
            "REDUCE" if self.is_sym_at(1, "(") => {
                self.bump();
                self.bump();
                let acc = self.variable()?;
                self.expect_sym("=")?;
                let init = self.expr()?;
                self.expect_sym(",")?;
                let var = self.variable()?;
                self.expect_kw("IN")?;
                let list = self.expr()?;
                self.expect_sym("|")?;
                let expr = self.expr()?;
                self.expect_sym(")")?;
                return Ok(Expr::Reduce {
                    acc,
                    init: Box::new(init),
                    var,
                    list: Box::new(list),
                    expr: Box::new(expr),
                });
            }
            _ => {}
        }
        // Function call, possibly namespaced: `db.labels()`, `toUpper(x)`.
        let mut n = 1;
        while self.is_sym_at(n, ".") && matches!(self.peek_at(n + 1), Tok::Ident(_)) {
            n += 2;
        }
        if self.is_sym_at(n, "(") {
            let mut name = String::new();
            for k in 0..n {
                match self.bump() {
                    Tok::Ident(s) => name.push_str(&s),
                    Tok::Sym(".") => name.push('.'),
                    _ => unreachable!("checked above"),
                }
                let _ = k;
            }
            self.expect_sym("(")?;
            let name = name.to_lowercase();
            if name == "count" && self.is_sym("*") {
                self.bump();
                self.expect_sym(")")?;
                return Ok(Expr::CountStar);
            }
            let distinct = self.eat_kw("DISTINCT");
            let mut args = Vec::new();
            if !self.is_sym(")") {
                args.push(self.expr()?);
                while self.eat_sym(",") {
                    args.push(self.expr()?);
                }
            }
            self.expect_sym(")")?;
            return Ok(Expr::Func {
                name,
                distinct,
                args,
            });
        }
        let v = self.variable()?;
        Ok(Expr::Var(v))
    }

    fn case_expr(&mut self) -> Result<Expr> {
        self.expect_kw("CASE")?;
        let operand = if self.is_kw("WHEN") {
            None
        } else {
            Some(Box::new(self.expr()?))
        };
        let mut whens = Vec::new();
        while self.eat_kw("WHEN") {
            let w = self.expr()?;
            self.expect_kw("THEN")?;
            whens.push((w, self.expr()?));
        }
        if whens.is_empty() {
            return Err(self.err("expected WHEN"));
        }
        let else_ = if self.eat_kw("ELSE") {
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.expect_kw("END")?;
        Ok(Expr::Case {
            operand,
            whens,
            else_,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_read_query() {
        let q = parse(
            "MATCH (p:Person {name: 'Ada'})-[r:KNOWS*1..3]->(f) WHERE f.age > 30 AND NOT (f)-->(:Bot) \
             WITH p, count(f) AS n ORDER BY n DESC LIMIT 5 WHERE n > 1 RETURN p.name AS name, n",
        )
        .unwrap();
        assert_eq!(q.parts[0].clauses.len(), 3);
        let Clause::Match { patterns, .. } = &q.parts[0].clauses[0] else {
            panic!()
        };
        assert_eq!(
            patterns[0].element.chain[0].0.length,
            Some((Some(1), Some(3)))
        );
    }

    #[test]
    fn parses_writes() {
        let q = parse(
            "MERGE (a:P {id: $id}) ON CREATE SET a.created = true ON MATCH SET a += {seen: 1} \
             WITH a MATCH (b) DETACH DELETE b",
        )
        .unwrap();
        assert!(matches!(q.parts[0].clauses[0], Clause::Merge { .. }));
    }

    #[test]
    fn parses_expressions() {
        for e in [
            "RETURN [x IN range(1, 10) WHERE x % 2 = 0 | x * x] AS xs",
            "RETURN CASE WHEN 1 < 2 < 3 THEN 'a' ELSE 'b' END",
            "RETURN all(x IN [1,2] WHERE x > 0), reduce(s = 0, x IN [1,2] | s + x)",
            "MATCH (n) RETURN n {.name, .*, k: 1}, n:Person, labels(n)[0], n.xs[1..]",
            "MATCH p = shortestPath((a)-[*]-(b)) RETURN length(p)",
            "CALL db.labels() YIELD label RETURN label",
            "MATCH (a)<-[:T|:U]-(b)--(c)<--(d) RETURN count(*)",
        ] {
            parse(e).unwrap_or_else(|err| panic!("{e}: {err}"));
        }
    }
}
