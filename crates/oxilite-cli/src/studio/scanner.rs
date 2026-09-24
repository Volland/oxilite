//! A lenient tokenizer for Turtle, TriG and SPARQL. It never fails: half-typed text still gives
//! tokens with LSP positions, prefix declarations and a guess at each term's role in its triple,
//! which is what completion, hover, definitions and the source-position index need.
//!
// @lat: [[architecture#Studio server#Language features]]

use oxiri::Iri;
use std::collections::BTreeMap;

/// A zero-based LSP position (UTF-16 columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// `<…>`, the text between the brackets.
    Iri(String),
    /// `prefix:local`, `prefix` possibly empty.
    PName {
        prefix: String,
        local: String,
    },
    /// `?x` or `$x`, without the sigil.
    Var(String),
    /// A quoted string (quotes included).
    Str,
    /// `@en`, or a Turtle directive such as `@prefix`.
    At(String),
    /// `_:label`.
    BNode,
    Number,
    /// `.`, `;`, `,`, `{`, `}`, `(`, `)`, `[`, `]`, `^^` and operators.
    Punct(String),
    /// Keywords, `a`, `true`, `false`, function names.
    Word(String),
}

/// Where a term sits in its triple, as far as a lenient scan can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Subject,
    Predicate,
    Object,
    /// Directives, graph names, function arguments and anything unclear.
    Other,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: Kind,
    pub start: Pos,
    pub end: Pos,
    /// Byte offsets in the source.
    pub from: usize,
    pub to: usize,
    pub role: Role,
    /// The absolute IRI of an `Iri` or `PName` token, when its prefix is known.
    pub iri: Option<String>,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub tokens: Vec<Token>,
    /// Prefix declarations in order of appearance: prefix, namespace, and the line declared on.
    pub prefixes: BTreeMap<String, (String, u32)>,
}

impl Scan {
    /// The token containing or ending exactly at `pos`.
    pub fn at(&self, pos: Pos) -> Option<&Token> {
        self.tokens
            .iter()
            .find(|t| t.start <= pos && pos <= t.end && t.start != pos)
            .or_else(|| self.tokens.iter().find(|t| t.start == pos))
    }
}

struct Lexer<'a> {
    src: &'a str,
    i: usize,
    line: u32,
    col: u32,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.i..].chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.src[self.i..].chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.col = 0;
        } else {
            self.col += c.len_utf16() as u32;
        }
        Some(c)
    }

    fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            col: self.col,
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.i..].starts_with(s)
    }
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || (c as u32) > 0x7F
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '%' || (c as u32) > 0x7F
}

/// Tokenizes `src`, resolving prefixed names and relative IRIs against `base`.
pub fn scan(src: &str, base: Option<&str>) -> Scan {
    let mut lx = Lexer {
        src,
        i: 0,
        line: 0,
        col: 0,
    };
    let mut tokens = Vec::new();
    while let Some(c) = lx.peek() {
        let (start, from) = (lx.pos(), lx.i);
        let kind = match c {
            c if c.is_whitespace() => {
                lx.bump();
                continue;
            }
            '#' => {
                while lx.peek().is_some_and(|c| c != '\n') {
                    lx.bump();
                }
                continue;
            }
            '<' if lx.peek_at(1).is_some_and(|n| n == '<' || n == '(') => {
                lx.bump();
                lx.bump();
                Kind::Punct(src[from..lx.i].into())
            }
            '<' if looks_like_iri(&src[lx.i..]) => {
                lx.bump();
                while lx
                    .peek()
                    .is_some_and(|c| c != '>' && c != '\n' && !c.is_whitespace())
                {
                    lx.bump();
                }
                let text = src[from + 1..lx.i].to_string();
                if lx.peek() == Some('>') {
                    lx.bump();
                }
                Kind::Iri(text)
            }
            '"' | '\'' => {
                let long = lx.starts_with(&c.to_string().repeat(3));
                let quote = if long { 3 } else { 1 };
                for _ in 0..quote {
                    lx.bump();
                }
                loop {
                    match lx.peek() {
                        None => break,
                        Some('\\') => {
                            lx.bump();
                            lx.bump();
                        }
                        Some('\n') if !long => break,
                        Some(q)
                            if q == c && (!long || lx.starts_with(&c.to_string().repeat(3))) =>
                        {
                            for _ in 0..quote {
                                lx.bump();
                            }
                            break;
                        }
                        Some(_) => {
                            lx.bump();
                        }
                    }
                }
                Kind::Str
            }
            '?' | '$' if lx.peek_at(1).is_some_and(|n| is_name_char(n) && n != '.') => {
                lx.bump();
                while lx.peek().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    lx.bump();
                }
                Kind::Var(src[from + 1..lx.i].into())
            }
            '?' | '$' => {
                // A lone sigil while typing: an empty variable, so completion can offer names.
                lx.bump();
                Kind::Var(String::new())
            }
            '@' => {
                lx.bump();
                while lx.peek().is_some_and(|c| c.is_alphanumeric() || c == '-') {
                    lx.bump();
                }
                Kind::At(src[from + 1..lx.i].into())
            }
            '_' if lx.peek_at(1) == Some(':') => {
                lx.bump();
                lx.bump();
                while lx.peek().is_some_and(is_name_char) {
                    lx.bump();
                }
                Kind::BNode
            }
            c if c.is_ascii_digit()
                || ((c == '+' || c == '-' || c == '.')
                    && lx.peek_at(1).is_some_and(|n| n.is_ascii_digit())) =>
            {
                lx.bump();
                while lx
                    .peek()
                    .is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
                {
                    // A trailing `.` ends the statement, not the number.
                    if lx.peek() == Some('.') && !lx.peek_at(1).is_some_and(|n| n.is_ascii_digit())
                    {
                        break;
                    }
                    lx.bump();
                }
                Kind::Number
            }
            c if is_name_start(c) || c == ':' => {
                while lx.peek().is_some_and(|c| is_name_char(c) || c == ':') {
                    lx.bump();
                }
                // Names do not end with a dot: that dot ends the statement.
                while src[from..lx.i].ends_with('.') {
                    lx.i -= 1;
                    lx.col -= 1;
                }
                let text = &src[from..lx.i];
                match text.split_once(':') {
                    Some((p, l)) => Kind::PName {
                        prefix: p.into(),
                        local: l.into(),
                    },
                    None => Kind::Word(text.into()),
                }
            }
            '^' if lx.peek_at(1) == Some('^') => {
                lx.bump();
                lx.bump();
                Kind::Punct("^^".into())
            }
            _ => {
                lx.bump();
                // Two-character operators, so `>=` is one token and `>>` closes a triple term.
                if let Some(n) = lx.peek() {
                    if matches!(
                        (c, n),
                        ('>', '>')
                            | ('>', '=')
                            | ('<', '=')
                            | ('!', '=')
                            | ('&', '&')
                            | ('|', '|')
                            | (')', '>')
                    ) {
                        lx.bump();
                    }
                }
                Kind::Punct(src[from..lx.i].into())
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: lx.pos(),
            from,
            to: lx.i,
            role: Role::Other,
            iri: None,
        });
    }
    let mut scan = Scan {
        tokens,
        prefixes: BTreeMap::new(),
    };
    resolve(&mut scan, base);
    assign_roles(&mut scan.tokens);
    scan
}

/// `<` starts an IRI unless it is a comparison: an IRI has no spaces before its `>`.
fn looks_like_iri(rest: &str) -> bool {
    for c in rest[1..].chars() {
        match c {
            '>' => return true,
            c if c.is_whitespace() || c == '<' || c == '"' => return false,
            _ => {}
        }
    }
    // An unterminated `<http…` at the end of a buffer is an IRI being typed.
    !rest[1..].is_empty()
}

fn resolve(scan: &mut Scan, base: Option<&str>) {
    let mut base = base.and_then(|b| Iri::parse(b.to_string()).ok());
    let mut i = 0;
    while i < scan.tokens.len() {
        // `@prefix p: <ns>` / `PREFIX p: <ns>` and `@base <b>` / `BASE <b>`.
        let directive = match &scan.tokens[i].kind {
            Kind::At(d) => Some(d.to_ascii_lowercase()),
            Kind::Word(w) => Some(w.to_ascii_lowercase()),
            _ => None,
        };
        match directive.as_deref() {
            Some("prefix") => {
                if let (Some(Kind::PName { prefix, local }), Some(Kind::Iri(ns))) = (
                    scan.tokens.get(i + 1).map(|t| t.kind.clone()),
                    scan.tokens.get(i + 2).map(|t| t.kind.clone()),
                ) {
                    if local.is_empty() {
                        let ns = absolute(base.as_ref(), &ns);
                        let line = scan.tokens[i].start.line;
                        scan.tokens[i + 2].iri = Some(ns.clone());
                        scan.prefixes.insert(prefix, (ns, line));
                        i += 3;
                        continue;
                    }
                }
            }
            Some("base") => {
                if let Some(Kind::Iri(b)) = scan.tokens.get(i + 1).map(|t| t.kind.clone()) {
                    let b = absolute(base.as_ref(), &b);
                    base = Iri::parse(b).ok().or(base);
                    i += 2;
                    continue;
                }
            }
            _ => {}
        }
        let resolved = match &scan.tokens[i].kind {
            Kind::Iri(iri) => Some(absolute(base.as_ref(), iri)),
            Kind::PName { prefix, local } => scan
                .prefixes
                .get(prefix)
                .map(|(ns, _)| format!("{ns}{}", unescape_local(local))),
            Kind::Word(w) if w == "a" => Some(RDF_TYPE.into()),
            _ => None,
        };
        scan.tokens[i].iri = resolved;
        i += 1;
    }
}

pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

fn absolute(base: Option<&Iri<String>>, iri: &str) -> String {
    match base {
        Some(b) => b
            .resolve(iri)
            .map(|i| i.into_inner())
            .unwrap_or_else(|_| iri.into()),
        None => iri.into(),
    }
}

fn unescape_local(local: &str) -> String {
    let mut out = String::with_capacity(local.len());
    let mut chars = local.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Keywords after which a SPARQL group starts over at subject position.
const RESET_WORDS: &[&str] = &[
    "where",
    "optional",
    "minus",
    "union",
    "graph",
    "service",
    "filter",
    "bind",
    "values",
    "select",
    "construct",
    "describe",
    "ask",
    "insert",
    "delete",
    "data",
    "exists",
    "not",
];

/// Walks the triple structure: subject, then predicate, then objects; `;` returns to predicate
/// position, `,` to object, `.` and `{` to subject; `[` opens a nested predicate list.
fn assign_roles(tokens: &mut [Token]) {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Subject,
        Predicate,
        Object,
        AfterObject,
    }
    let mut state = State::Subject;
    let mut stack: Vec<State> = Vec::new();
    // Inside `( … )` of a function call or expression, roles do not apply.
    let mut expr_depth = 0usize;
    let mut prev_word: Option<String> = None;
    let mut i = 0;
    while i < tokens.len() {
        let is_term = matches!(
            tokens[i].kind,
            Kind::Iri(_)
                | Kind::PName { .. }
                | Kind::Var(_)
                | Kind::Str
                | Kind::BNode
                | Kind::Number
        ) || matches!(&tokens[i].kind, Kind::Word(w) if w == "a" || w == "true" || w == "false");
        match &tokens[i].kind {
            // A prefix declaration's tokens are not triple terms.
            Kind::At(d) | Kind::Word(d)
                if d.eq_ignore_ascii_case("prefix") || d.eq_ignore_ascii_case("base") =>
            {
                i += if d.eq_ignore_ascii_case("prefix") {
                    3
                } else {
                    2
                };
                state = State::Subject;
                continue;
            }
            Kind::Punct(p) if expr_depth > 0 => match p.as_str() {
                "(" => expr_depth += 1,
                ")" => expr_depth -= 1,
                _ => {}
            },
            _ if expr_depth > 0 => {}
            Kind::Punct(p) => match p.as_str() {
                "." | "{" | "}" => state = State::Subject,
                ";" => state = State::Predicate,
                "," => state = State::Object,
                "[" => {
                    stack.push(state);
                    state = State::Predicate;
                }
                "]" => {
                    let outer = stack.pop().unwrap_or(State::Subject);
                    state = match outer {
                        State::Subject => State::Predicate,
                        _ => State::AfterObject,
                    };
                }
                "(" => {
                    // A collection in object or subject position, or an expression.
                    let expression = prev_word.is_some() || state == State::Predicate;
                    if expression {
                        expr_depth = 1;
                    } else {
                        stack.push(state);
                        state = State::Object;
                    }
                }
                ")" => {
                    let outer = stack.pop().unwrap_or(State::Subject);
                    state = match outer {
                        State::Subject => State::Predicate,
                        _ => State::AfterObject,
                    };
                }
                "<<" | "<<(" => {
                    stack.push(state);
                    state = State::Subject;
                }
                ">>" | ")>>" => {
                    let outer = stack.pop().unwrap_or(State::Subject);
                    state = match outer {
                        State::Subject => State::Predicate,
                        _ => State::AfterObject,
                    };
                }
                _ => {}
            },
            Kind::Word(w) if RESET_WORDS.contains(&w.to_ascii_lowercase().as_str()) => {
                state = State::Subject;
                // `FILTER(`, `BIND(`, `EXISTS {`: a following `(` is an expression.
                prev_word = Some(w.clone());
                i += 1;
                continue;
            }
            Kind::Word(w) if !is_term => {
                // A function or keyword: a following `(` is an expression.
                prev_word = Some(w.clone());
                i += 1;
                continue;
            }
            _ if is_term => {
                let role = match state {
                    State::Subject => {
                        state = State::Predicate;
                        Role::Subject
                    }
                    State::Predicate => {
                        state = State::Object;
                        Role::Predicate
                    }
                    State::Object => {
                        state = State::AfterObject;
                        Role::Object
                    }
                    // TriG `<g> {`, or two terms in a row: treat as a fresh subject.
                    State::AfterObject => {
                        state = State::Predicate;
                        Role::Subject
                    }
                };
                tokens[i].role = role;
            }
            _ => {}
        }
        prev_word = None;
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(src: &str) -> Vec<(String, Role)> {
        scan(src, Some("http://base.org/"))
            .tokens
            .into_iter()
            .filter(|t| t.iri.is_some() || matches!(t.kind, Kind::Var(_)))
            .map(|t| (src[t.from..t.to].to_string(), t.role))
            .collect()
    }

    // @lat: [[tests#Studio server#Scanner roles and prefixes]]
    #[test]
    fn turtle_roles_and_prefixes() {
        let src = "@prefix ex: <http://ex.org/> .\nex:a a ex:C ; ex:p ex:b , [ ex:q <rel> ] .\n";
        let s = scan(src, Some("http://base.org/"));
        assert_eq!(s.prefixes["ex"].0, "http://ex.org/");
        let r = roles(src);
        assert_eq!(
            r[1..],
            [
                ("ex:a".into(), Role::Subject),
                ("a".into(), Role::Predicate),
                ("ex:C".into(), Role::Object),
                ("ex:p".into(), Role::Predicate),
                ("ex:b".into(), Role::Object),
                ("ex:q".into(), Role::Predicate),
                ("<rel>".into(), Role::Object),
            ]
        );
        let rel = s
            .tokens
            .iter()
            .find(|t| t.kind == Kind::Iri("rel".into()))
            .unwrap();
        assert_eq!(rel.iri.as_deref(), Some("http://base.org/rel"));
    }

    #[test]
    fn sparql_roles_skip_expressions() {
        let src = "PREFIX ex: <http://ex.org/>\nSELECT ?x WHERE { ?x ex:p ?y . FILTER(?y > 3) ?x a ex:C }";
        let r = roles(src);
        let find = |s: &str| {
            r.iter()
                .filter(|(t, _)| t == s)
                .map(|(_, r)| *r)
                .collect::<Vec<_>>()
        };
        assert_eq!(find("ex:p"), [Role::Predicate]);
        assert_eq!(find("ex:C"), [Role::Object]);
        assert_eq!(find("?y"), [Role::Object, Role::Other]);
    }

    #[test]
    fn unfinished_text_still_scans() {
        let s = scan("PREFIX ex: <http://ex.org/>\nSELECT * { ?s ex:", None);
        let last = s.tokens.last().unwrap();
        assert_eq!(
            last.kind,
            Kind::PName {
                prefix: "ex".into(),
                local: String::new()
            }
        );
        assert_eq!(last.role, Role::Predicate);
        assert_eq!(last.iri.as_deref(), Some("http://ex.org/"));
        // UTF-16 columns.
        let s = scan("\"😀\" ex:a", None);
        assert_eq!(s.tokens[1].start.col, 5);
    }
}
