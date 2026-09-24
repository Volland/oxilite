//! Recursive-descent parser for the Datalog dialect.
//!
//! Grammar, informally:
//!
//! ```text
//! program    := ( directive | rule | goal )*
//! directive  := '@prefix' NAME ':' IRI '.'
//! rule       := head ( ':-' body )? '.'
//! goal       := '?-' body '.'
//! head       := pred '(' headarg ( ',' headarg )* ')'
//! body       := item ( ',' item )*
//! item       := 'not' atom | atom | constraint
//! ```
//!
// @lat: [[architecture#Datalog frontend]]

use crate::ast::*;
use crate::error::{DatalogError, Result, Span};
use crate::lexer::{Lexer, Spanned, Tok};
use oxrdf::vocab::xsd;
use oxrdf::{Literal, NamedNode, Term};
use std::collections::HashMap;

/// Parses a program.
pub fn parse(src: &str) -> Result<Program> {
    let tokens = Lexer::new(src).tokenize()?;
    Parser {
        toks: tokens,
        pos: 0,
        prefixes: default_prefixes(),
    }
    .program()
}

fn default_prefixes() -> HashMap<String, String> {
    // Always available, so a program can write `xsd:date` without a directive.
    [
        ("xsd", "http://www.w3.org/2001/XMLSchema#"),
        ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
        ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
        ("owl", "http://www.w3.org/2002/07/owl#"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

struct Parser {
    toks: Vec<Spanned>,
    pos: usize,
    prefixes: HashMap<String, String>,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|s| &s.tok)
    }

    fn peek_at(&self, n: usize) -> Option<&Tok> {
        self.toks.get(self.pos + n).map(|s| &s.tok)
    }

    fn span(&self) -> Span {
        self.toks
            .get(self.pos)
            .or_else(|| self.toks.last())
            .map(|s| s.span)
            .unwrap_or_default()
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos)?.tok.clone();
        self.pos += 1;
        Some(t)
    }

    fn eat(&mut self, want: &Tok) -> bool {
        if self.peek() == Some(want) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, want: &Tok, what: &str) -> Result<()> {
        if self.eat(want) {
            Ok(())
        } else {
            Err(DatalogError::parse(
                self.span(),
                format!("expected {what}, found {}", self.describe()),
            ))
        }
    }

    fn describe(&self) -> String {
        match self.peek() {
            None => "end of program".to_owned(),
            Some(t) => format!("{t:?}"),
        }
    }

    fn program(&mut self) -> Result<Program> {
        let mut program = Program::default();
        while self.pos < self.toks.len() {
            match self.peek() {
                Some(Tok::AtPrefix) => self.directive()?,
                Some(Tok::Query) => {
                    self.bump();
                    let items = self.body()?;
                    self.expect(&Tok::Dot, "`.` after the goal")?;
                    let mut atom = None;
                    let mut constraints = Vec::new();
                    for item in items {
                        match item {
                            BodyItem::Atom(a) if atom.is_none() => atom = Some(a),
                            BodyItem::Constraint(e) => constraints.push(e),
                            _ => {
                                return Err(DatalogError::parse(
                                    self.span(),
                                    "a goal is one atom, optionally followed by constraints",
                                ))
                            }
                        }
                    }
                    let atom = atom.ok_or_else(|| {
                        DatalogError::parse(self.span(), "a goal needs an atom")
                    })?;
                    program.goal = Some(Goal { atom, constraints });
                }
                _ => program.rules.push(self.rule()?),
            }
        }
        Ok(program)
    }

    fn directive(&mut self) -> Result<()> {
        self.bump();
        let span = self.span();
        let name = match self.bump() {
            // `@prefix ex: <...>` lexes the `ex:` as an empty-local CURIE.
            Some(Tok::Curie(p, local)) if local.is_empty() => p,
            Some(Tok::Name(n)) => {
                self.expect(&Tok::Eq, "`:` after the prefix name").ok();
                n
            }
            _ => return Err(DatalogError::parse(span, "expected a prefix name")),
        };
        let iri = match self.bump() {
            Some(Tok::Iri(i)) => i,
            _ => return Err(DatalogError::parse(span, "expected an IRI after the prefix")),
        };
        self.expect(&Tok::Dot, "`.` after the @prefix directive")?;
        self.prefixes.insert(name, iri);
        Ok(())
    }

    fn rule(&mut self) -> Result<Rule> {
        let head = self.head()?;
        let body = if self.eat(&Tok::Implies) {
            self.body()?
        } else {
            Vec::new()
        };
        self.expect(&Tok::Dot, "`.` at the end of the rule")?;
        Ok(Rule { head, body })
    }

    fn head(&mut self) -> Result<Head> {
        let span = self.span();
        let pred = self.pred()?;
        self.expect(&Tok::LParen, "`(` after the head predicate")?;
        let mut args = Vec::new();
        loop {
            args.push(self.head_arg()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "`)` closing the head")?;
        Ok(Head { pred, args, span })
    }

    fn head_arg(&mut self) -> Result<HeadArg> {
        // An aggregate looks like a call: `COUNT(?y)`.
        if let (Some(Tok::Name(name)), Some(Tok::LParen)) = (self.peek(), self.peek_at(1)) {
            if let Some(func) = agg_fn(name) {
                self.bump();
                self.bump();
                let distinct = matches!(self.peek(), Some(Tok::Name(n)) if n.eq_ignore_ascii_case("distinct"));
                if distinct {
                    self.bump();
                }
                let span = self.span();
                let var = match self.bump() {
                    Some(Tok::Var(v)) => v,
                    // `COUNT(*)` counts rows; bind it to the group itself.
                    Some(Tok::Star) => "*".to_owned(),
                    _ => {
                        return Err(DatalogError::parse(
                            span,
                            "an aggregate takes one variable",
                        ))
                    }
                };
                self.expect(&Tok::RParen, "`)` closing the aggregate")?;
                let func = if distinct && func == AggFn::Count {
                    AggFn::CountDistinct
                } else {
                    func
                };
                return Ok(HeadArg::Agg { func, var });
            }
        }
        Ok(HeadArg::Plain(self.arg()?))
    }

    fn pred(&mut self) -> Result<Pred> {
        let span = self.span();
        match self.bump() {
            Some(Tok::Iri(i)) => Ok(Pred::Edb(self.iri(&i, span)?)),
            Some(Tok::Curie(p, l)) => Ok(Pred::Edb(self.curie(&p, &l, span)?)),
            Some(Tok::Name(n)) => {
                if n == "triple" || n == "quad" {
                    // Arity decides which form this is; fixed up once the arguments are read.
                    Ok(Pred::Triple { graph: n == "quad" })
                } else {
                    Ok(Pred::Idb(n))
                }
            }
            _ => Err(DatalogError::parse(span, "expected a predicate")),
        }
    }

    fn arg(&mut self) -> Result<Arg> {
        let span = self.span();
        match self.peek() {
            Some(Tok::Var(_)) => match self.bump() {
                Some(Tok::Var(v)) => Ok(Arg::Var(v)),
                _ => unreachable!("peeked a variable"),
            },
            Some(Tok::Wildcard) => {
                self.bump();
                Ok(Arg::Wildcard)
            }
            _ => Ok(Arg::Const(self.term()?)),
        }
        .map_err(|e: DatalogError| match e {
            DatalogError::Parse { message, .. } => DatalogError::parse(span, message),
            other => other,
        })
    }

    fn term(&mut self) -> Result<Term> {
        let span = self.span();
        match self.bump() {
            Some(Tok::Iri(i)) => Ok(self.iri(&i, span)?.into()),
            Some(Tok::Curie(p, l)) => Ok(self.curie(&p, &l, span)?.into()),
            Some(Tok::Int(v)) => Ok(Literal::new_typed_literal(v.to_string(), xsd::INTEGER).into()),
            Some(Tok::Num(v)) => Ok(Literal::new_typed_literal(
                format_double(v),
                xsd::DOUBLE,
            )
            .into()),
            Some(Tok::Minus) => {
                // A negative numeric literal.
                match self.bump() {
                    Some(Tok::Int(v)) => {
                        Ok(Literal::new_typed_literal((-v).to_string(), xsd::INTEGER).into())
                    }
                    Some(Tok::Num(v)) => {
                        Ok(Literal::new_typed_literal(format_double(-v), xsd::DOUBLE).into())
                    }
                    _ => Err(DatalogError::parse(span, "expected a number after `-`")),
                }
            }
            Some(Tok::Str {
                value,
                lang,
                datatype,
            }) => {
                if let Some(lang) = lang {
                    return Literal::new_language_tagged_literal(value, lang)
                        .map(Term::from)
                        .map_err(|e| DatalogError::parse(span, e.to_string()));
                }
                match datatype {
                    None => Ok(Literal::new_simple_literal(value).into()),
                    Some(dt) => {
                        let dt = match *dt {
                            Tok::Iri(i) => self.iri(&i, span)?,
                            Tok::Curie(p, l) => self.curie(&p, &l, span)?,
                            Tok::Name(n) => self.curie("xsd", &n, span)?,
                            _ => return Err(DatalogError::parse(span, "expected a datatype")),
                        };
                        Ok(Literal::new_typed_literal(value, dt).into())
                    }
                }
            }
            Some(Tok::Name(n)) if n == "true" || n == "false" => {
                Ok(Literal::new_typed_literal(n, xsd::BOOLEAN).into())
            }
            _ => Err(DatalogError::parse(span, "expected a term")),
        }
    }

    fn iri(&self, i: &str, span: Span) -> Result<NamedNode> {
        NamedNode::new(i).map_err(|e| DatalogError::parse(span, e.to_string()))
    }

    fn curie(&self, prefix: &str, local: &str, span: Span) -> Result<NamedNode> {
        let base = self
            .prefixes
            .get(prefix)
            .ok_or_else(|| DatalogError::UnknownPrefix(prefix.to_owned()))?;
        NamedNode::new(format!("{base}{local}")).map_err(|e| DatalogError::parse(span, e.to_string()))
    }

    fn body(&mut self) -> Result<Vec<BodyItem>> {
        let mut out = Vec::new();
        loop {
            out.push(self.body_item()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        Ok(out)
    }

    fn body_item(&mut self) -> Result<BodyItem> {
        let negated = match self.peek() {
            Some(Tok::Name(n)) if n == "not" => {
                self.bump();
                true
            }
            // `!p(...)` is `not p(...)`; `!` before anything else negates an expression.
            Some(Tok::Bang) if self.looks_like_atom(1) => {
                self.bump();
                true
            }
            _ => false,
        };
        if negated || self.looks_like_atom(0) {
            let atom = self.atom()?;
            return Ok(if negated {
                BodyItem::Negated(atom)
            } else {
                BodyItem::Atom(atom)
            });
        }
        Ok(BodyItem::Constraint(self.expr()?))
    }

    /// An atom is a predicate followed by `(`, and is not a known function call.
    fn looks_like_atom(&self, offset: usize) -> bool {
        let head = self.peek_at(offset);
        let next = self.peek_at(offset + 1);
        if next != Some(&Tok::LParen) {
            return false;
        }
        match head {
            Some(Tok::Iri(_)) | Some(Tok::Curie(..)) => true,
            Some(Tok::Name(n)) => !is_function(n),
            _ => false,
        }
    }

    fn atom(&mut self) -> Result<Atom> {
        let span = self.span();
        let mut pred = self.pred()?;
        self.expect(&Tok::LParen, "`(` after the predicate")?;
        let mut args = Vec::new();
        loop {
            args.push(self.arg()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "`)` closing the atom")?;
        // `triple(s,p,o)` and `triple(s,p,o,g)` share a name; the arity picks the form.
        if let Pred::Triple { .. } = pred {
            pred = Pred::Triple {
                graph: args.len() == 4,
            };
        }
        if let Some(want) = pred.arity() {
            if args.len() != want {
                return Err(DatalogError::Arity {
                    predicate: pred.to_string(),
                    expected: want,
                    found: args.len(),
                });
            }
        }
        if let Pred::Edb(_) = &pred {
            if !matches!(args.len(), 1 | 2) {
                return Err(DatalogError::Arity {
                    predicate: pred.to_string(),
                    expected: 2,
                    found: args.len(),
                });
            }
        }
        Ok(Atom { pred, args, span })
    }

    // Expression parsing, lowest precedence first.

    fn expr(&mut self) -> Result<Expr> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr> {
        let mut left = self.and_expr()?;
        while self.eat(&Tok::Or) {
            let right = self.and_expr()?;
            left = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Expr> {
        let mut left = self.cmp_expr()?;
        while self.eat(&Tok::And) {
            let right = self.cmp_expr()?;
            left = Expr::Binary {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn cmp_expr(&mut self) -> Result<Expr> {
        let left = self.add_expr()?;
        let op = match self.peek() {
            Some(Tok::Eq) => BinOp::Eq,
            Some(Tok::Ne) => BinOp::Ne,
            Some(Tok::Lt) => BinOp::Lt,
            Some(Tok::Le) => BinOp::Le,
            Some(Tok::Gt) => BinOp::Gt,
            Some(Tok::Ge) => BinOp::Ge,
            _ => return Ok(left),
        };
        self.bump();
        let right = self.add_expr()?;
        Ok(Expr::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        })
    }

    fn add_expr(&mut self) -> Result<Expr> {
        let mut left = self.mul_expr()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => return Ok(left),
            };
            self.bump();
            let right = self.mul_expr()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
    }

    fn mul_expr(&mut self) -> Result<Expr> {
        let mut left = self.unary_expr()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinOp::Mul,
                Some(Tok::Slash) => BinOp::Div,
                _ => return Ok(left),
            };
            self.bump();
            let right = self.unary_expr()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
    }

    fn unary_expr(&mut self) -> Result<Expr> {
        if self.eat(&Tok::Bang) {
            return Ok(Expr::Not(Box::new(self.unary_expr()?)));
        }
        if self.peek() == Some(&Tok::Minus) {
            // Only a negation when a number does not directly follow, which `term` handles.
            if !matches!(self.peek_at(1), Some(Tok::Int(_)) | Some(Tok::Num(_))) {
                self.bump();
                return Ok(Expr::Neg(Box::new(self.unary_expr()?)));
            }
        }
        self.primary_expr()
    }

    fn primary_expr(&mut self) -> Result<Expr> {
        if self.eat(&Tok::LParen) {
            let e = self.expr()?;
            self.expect(&Tok::RParen, "`)` closing the expression")?;
            return Ok(e);
        }
        match self.peek() {
            Some(Tok::Var(_)) => match self.bump() {
                Some(Tok::Var(v)) => Ok(Expr::Var(v)),
                _ => unreachable!("peeked a variable"),
            },
            Some(Tok::Name(n)) if self.peek_at(1) == Some(&Tok::LParen) => {
                let name = n.to_uppercase();
                self.bump();
                self.bump();
                let mut args = Vec::new();
                if self.peek() != Some(&Tok::RParen) {
                    loop {
                        args.push(self.expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&Tok::RParen, "`)` closing the function call")?;
                Ok(Expr::Call { name, args })
            }
            _ => Ok(Expr::Const(self.term()?)),
        }
    }
}

fn agg_fn(name: &str) -> Option<AggFn> {
    Some(match name.to_ascii_uppercase().as_str() {
        "COUNT" => AggFn::Count,
        "SUM" => AggFn::Sum,
        "MIN" => AggFn::Min,
        "MAX" => AggFn::Max,
        "AVG" => AggFn::Avg,
        "SAMPLE" => AggFn::Sample,
        "GROUP_CONCAT" | "CONCAT_AGG" => AggFn::GroupConcat,
        _ => return None,
    })
}

/// Names that are functions in a constraint, not predicates.
pub(crate) fn is_function(name: &str) -> bool {
    const FUNCTIONS: &[&str] = &[
        "REGEX", "STR", "STRLEN", "SUBSTR", "UCASE", "LCASE", "STRSTARTS", "STRENDS", "CONTAINS",
        "STRBEFORE", "STRAFTER", "CONCAT", "ABS", "CEIL", "FLOOR", "ROUND", "YEAR", "MONTH", "DAY",
        "HOURS", "MINUTES", "SECONDS", "DATATYPE", "LANG", "ISIRI", "ISURI", "ISLITERAL",
        "ISNUMERIC", "ISBLANK", "BOUND", "IF", "COALESCE", "SAMETERM", "STRDT", "STRLANG",
    ];
    FUNCTIONS.iter().any(|f| f.eq_ignore_ascii_case(name))
}

/// Formats a double in the canonical `xsd:double` form.
fn format_double(v: f64) -> String {
    if v == v.trunc() && v.is_finite() && v.abs() < 1e15 {
        format!("{v:.1}E0")
    } else {
        format!("{v:E}")
    }
}
