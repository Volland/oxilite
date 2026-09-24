//! Tokenizer for the Datalog dialect.
//!
//! RDF-native: variables are `?x`, IRIs are `<...>` or CURIEs declared with `@prefix`, and
//! literals carry `^^` datatypes and `@` language tags, as in Turtle and SPARQL.
//!
// @lat: [[architecture#Datalog frontend]]

use crate::error::{DatalogError, Result, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// `?x`
    Var(String),
    /// `_`
    Wildcard,
    /// A bare name: a derived predicate, a function name, or a keyword.
    Name(String),
    /// `<http://...>`
    Iri(String),
    /// `ex:name`
    Curie(String, String),
    /// A quoted string, with an optional language tag or datatype.
    Str {
        value: String,
        lang: Option<String>,
        datatype: Option<Box<Tok>>,
    },
    /// An integer literal.
    Int(i64),
    /// A decimal or double literal.
    Num(f64),
    /// `:-`
    Implies,
    /// `?-`
    Query,
    /// `@prefix`
    AtPrefix,
    LParen,
    RParen,
    Comma,
    Dot,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `!`
    Bang,
}

/// A token with its position.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
    pub tok: Tok,
    pub span: Span,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    column: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    fn span(&self) -> Span {
        Span {
            line: self.line,
            column: self.column,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<u8> {
        self.src.get(self.pos + n).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(b)
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b) if b.is_ascii_whitespace() => {
                    self.bump();
                }
                // `%` and `#` both start a line comment: `%` is Prolog's, `#` is Turtle's.
                Some(b'%') | Some(b'#') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => return,
            }
        }
    }

    /// Tokenizes the whole program.
    pub fn tokenize(mut self) -> Result<Vec<Spanned>> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            let span = self.span();
            let Some(b) = self.peek() else { break };
            let tok = match b {
                b'?' => {
                    self.bump();
                    if self.peek() == Some(b'-') {
                        self.bump();
                        Tok::Query
                    } else {
                        Tok::Var(self.read_name(span)?)
                    }
                }
                b'$' => {
                    self.bump();
                    Tok::Var(self.read_name(span)?)
                }
                b'<' => {
                    if self.peek_at(1) == Some(b'=') {
                        self.bump();
                        self.bump();
                        Tok::Le
                    } else if self.is_iri_start() {
                        self.read_iri(span)?
                    } else {
                        self.bump();
                        Tok::Lt
                    }
                }
                b'>' => {
                    self.bump();
                    if self.peek() == Some(b'=') {
                        self.bump();
                        Tok::Ge
                    } else {
                        Tok::Gt
                    }
                }
                b'=' => {
                    self.bump();
                    // `==` is accepted as a synonym for `=`.
                    if self.peek() == Some(b'=') {
                        self.bump();
                    }
                    Tok::Eq
                }
                b'!' => {
                    self.bump();
                    if self.peek() == Some(b'=') {
                        self.bump();
                        Tok::Ne
                    } else {
                        Tok::Bang
                    }
                }
                b':' => {
                    if self.peek_at(1) == Some(b'-') {
                        self.bump();
                        self.bump();
                        Tok::Implies
                    } else {
                        // A CURIE in the default prefix, e.g. `:name`.
                        self.bump();
                        Tok::Curie(String::new(), self.read_name(span)?)
                    }
                }
                b'@' => {
                    self.bump();
                    let word = self.read_name(span)?;
                    if word == "prefix" || word == "base" {
                        Tok::AtPrefix
                    } else {
                        return Err(DatalogError::parse(span, format!("unknown directive @{word}")));
                    }
                }
                b'&' => {
                    self.bump();
                    if self.peek() == Some(b'&') {
                        self.bump();
                    }
                    Tok::And
                }
                b'|' => {
                    self.bump();
                    if self.peek() == Some(b'|') {
                        self.bump();
                    }
                    Tok::Or
                }
                b'"' | b'\'' => self.read_string(span)?,
                b'(' => {
                    self.bump();
                    Tok::LParen
                }
                b')' => {
                    self.bump();
                    Tok::RParen
                }
                b',' => {
                    self.bump();
                    Tok::Comma
                }
                b'.' => {
                    self.bump();
                    Tok::Dot
                }
                b'+' => {
                    self.bump();
                    Tok::Plus
                }
                b'-' => {
                    self.bump();
                    Tok::Minus
                }
                b'*' => {
                    self.bump();
                    Tok::Star
                }
                b'/' => {
                    self.bump();
                    Tok::Slash
                }
                b'0'..=b'9' => self.read_number(span)?,
                b'_' if !Self::is_name_byte(self.peek_at(1).unwrap_or(b' ')) => {
                    self.bump();
                    Tok::Wildcard
                }
                b if Self::is_name_start(b) => {
                    let name = self.read_name(span)?;
                    if self.peek() == Some(b':') && self.peek_at(1) != Some(b'-') {
                        self.bump();
                        let local = if Self::is_name_start(self.peek().unwrap_or(b' ')) {
                            self.read_name(span)?
                        } else {
                            String::new()
                        };
                        Tok::Curie(name, local)
                    } else {
                        Tok::Name(name)
                    }
                }
                other => {
                    return Err(DatalogError::parse(
                        span,
                        format!("unexpected character `{}`", other as char),
                    ))
                }
            };
            out.push(Spanned { tok, span });
        }
        Ok(out)
    }

    /// Distinguishes `<http://...>` from the `<` operator by scanning for a closing `>`
    /// with no whitespace in between.
    fn is_iri_start(&self) -> bool {
        let mut i = self.pos + 1;
        while let Some(b) = self.src.get(i).copied() {
            if b == b'>' {
                return i > self.pos + 1;
            }
            if b.is_ascii_whitespace() || b == b'<' {
                return false;
            }
            i += 1;
        }
        false
    }

    fn read_iri(&mut self, span: Span) -> Result<Tok> {
        self.bump();
        let mut s = String::new();
        loop {
            match self.bump() {
                Some(b'>') => return Ok(Tok::Iri(s)),
                Some(b) => s.push(b as char),
                None => return Err(DatalogError::parse(span, "unterminated IRI")),
            }
        }
    }

    const fn is_name_start(b: u8) -> bool {
        b.is_ascii_alphabetic() || b == b'_'
    }

    const fn is_name_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'
    }

    fn read_name(&mut self, span: Span) -> Result<String> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            // A trailing `.` ends a clause rather than a name, so only keep an interior one.
            if b == b'.' && !Self::is_name_byte(self.peek_at(1).unwrap_or(b' ')) {
                break;
            }
            if !Self::is_name_byte(b) {
                break;
            }
            self.bump();
        }
        if start == self.pos {
            return Err(DatalogError::parse(span, "expected a name"));
        }
        Ok(String::from_utf8_lossy(&self.src[start..self.pos]).into_owned())
    }

    fn read_number(&mut self, span: Span) -> Result<Tok> {
        let start = self.pos;
        let mut float = false;
        while let Some(b) = self.peek() {
            match b {
                b'0'..=b'9' => {
                    self.bump();
                }
                // A `.` is part of the number only when a digit follows it.
                b'.' if self.peek_at(1).is_some_and(|d| d.is_ascii_digit()) => {
                    float = true;
                    self.bump();
                }
                b'e' | b'E' => {
                    float = true;
                    self.bump();
                    if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                        self.bump();
                    }
                }
                _ => break,
            }
        }
        let text = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        if float {
            text.parse::<f64>()
                .map(Tok::Num)
                .map_err(|_| DatalogError::parse(span, format!("invalid number `{text}`")))
        } else {
            match text.parse::<i64>() {
                Ok(v) => Ok(Tok::Int(v)),
                // Integers too large for i64 are still valid RDF, so keep them as doubles.
                Err(_) => text
                    .parse::<f64>()
                    .map(Tok::Num)
                    .map_err(|_| DatalogError::parse(span, format!("invalid number `{text}`"))),
            }
        }
    }

    fn read_string(&mut self, span: Span) -> Result<Tok> {
        let quote = self.bump().expect("checked by the caller");
        let mut value = String::new();
        loop {
            match self.bump() {
                Some(b'\\') => {
                    let e = self
                        .bump()
                        .ok_or_else(|| DatalogError::parse(span, "unterminated escape"))?;
                    value.push(match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'"' => '"',
                        b'\'' => '\'',
                        b'\\' => '\\',
                        other => {
                            return Err(DatalogError::parse(
                                span,
                                format!("unknown escape `\\{}`", other as char),
                            ))
                        }
                    });
                }
                Some(b) if b == quote => break,
                // Multi-byte UTF-8 passes through one byte at a time and is reassembled below.
                Some(b) => value.push(b as char),
                None => return Err(DatalogError::parse(span, "unterminated string")),
            }
        }
        let value = fix_utf8(&value);
        let mut lang = None;
        let mut datatype = None;
        if self.peek() == Some(b'@') {
            self.bump();
            lang = Some(self.read_name(span)?);
        } else if self.peek() == Some(b'^') && self.peek_at(1) == Some(b'^') {
            self.bump();
            self.bump();
            let dt_span = self.span();
            let dt = match self.peek() {
                Some(b'<') => self.read_iri(dt_span)?,
                Some(b) if Self::is_name_start(b) || b == b':' => {
                    if b == b':' {
                        self.bump();
                        Tok::Curie(String::new(), self.read_name(dt_span)?)
                    } else {
                        let prefix = self.read_name(dt_span)?;
                        if self.peek() == Some(b':') {
                            self.bump();
                            Tok::Curie(prefix, self.read_name(dt_span)?)
                        } else {
                            Tok::Name(prefix)
                        }
                    }
                }
                _ => return Err(DatalogError::parse(dt_span, "expected a datatype IRI")),
            };
            datatype = Some(Box::new(dt));
        }
        Ok(Tok::Str {
            value,
            lang,
            datatype,
        })
    }
}

/// Reassembles the bytes pushed one at a time by [`Lexer::read_string`] into UTF-8.
fn fix_utf8(s: &str) -> String {
    if s.chars().all(|c| (c as u32) < 0x80) {
        return s.to_owned();
    }
    let bytes: Vec<u8> = s.chars().map(|c| c as u32 as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}
