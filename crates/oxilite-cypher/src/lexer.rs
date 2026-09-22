//! Tokenizer for openCypher.
//!
// @lat: [[architecture#Property graph frontend]]

use crate::error::{CypherError, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// An identifier or keyword (keywords are matched case-insensitively by the parser).
    Ident(String),
    /// A backtick-quoted identifier (never a keyword).
    Quoted(String),
    Int(i64),
    Float(f64),
    Str(String),
    Param(String),
    /// Punctuation and operators.
    Sym(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub pos: usize,
}

const SYMS: &[&str] = &[
    "<>", "!=", "<=", ">=", "=~", "+=", "..", "(", ")", "[", "]", "{", "}", ",", ".", ":", ";",
    "|", "+", "-", "*", "/", "%", "^", "=", "<", ">",
];

pub fn tokenize(src: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments.
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        let pos = i;
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Token {
                tok: Tok::Ident(chars[start..i].iter().collect()),
                pos,
            });
            continue;
        }
        if c == '`' {
            let mut s = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return Err(CypherError::syntax(pos, "unterminated `identifier`")),
                    Some('`') if chars.get(i + 1) == Some(&'`') => {
                        s.push('`');
                        i += 2;
                    }
                    Some('`') => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        s.push(*ch);
                        i += 1;
                    }
                }
            }
            out.push(Token {
                tok: Tok::Quoted(s),
                pos,
            });
            continue;
        }
        if c == '$' {
            i += 1;
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            if start == i {
                return Err(CypherError::syntax(
                    pos,
                    "expected a parameter name after $",
                ));
            }
            out.push(Token {
                tok: Tok::Param(chars[start..i].iter().collect()),
                pos,
            });
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            out.push(Token {
                tok: number(&chars, &mut i)?,
                pos,
            });
            continue;
        }
        if c == '\'' || c == '"' {
            out.push(Token {
                tok: Tok::Str(string(&chars, &mut i, c)?),
                pos,
            });
            continue;
        }
        let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
        match SYMS.iter().find(|s| rest.starts_with(**s)) {
            Some(s) => {
                i += s.len();
                out.push(Token {
                    tok: Tok::Sym(s),
                    pos,
                });
            }
            None => {
                return Err(CypherError::syntax(
                    pos,
                    format!("unexpected character '{c}'"),
                ))
            }
        }
    }
    out.push(Token {
        tok: Tok::Eof,
        pos: chars.len(),
    });
    Ok(out)
}

fn number(chars: &[char], i: &mut usize) -> Result<Tok> {
    let start = *i;
    if chars[*i] == '0' && matches!(chars.get(*i + 1), Some('o' | 'O')) {
        *i += 2;
        let s = *i;
        while *i < chars.len() && chars[*i].is_ascii_alphanumeric() {
            *i += 1;
        }
        let text: String = chars[s..*i].iter().collect();
        return i64::from_str_radix(&text, 8)
            .map(Tok::Int)
            .map_err(|_| CypherError::syntax(start, "invalid octal integer"));
    }
    if chars[*i] == '0' && matches!(chars.get(*i + 1), Some('x' | 'X')) {
        *i += 2;
        let s = *i;
        while *i < chars.len() && chars[*i].is_ascii_alphanumeric() {
            *i += 1;
        }
        let text: String = chars[s..*i].iter().collect();
        return i64::from_str_radix(&text, 16)
            .map(Tok::Int)
            .map_err(|_| CypherError::syntax(start, "invalid hexadecimal integer"));
    }
    let mut float = false;
    while *i < chars.len() && chars[*i].is_ascii_digit() {
        *i += 1;
    }
    // `1..2` is a range, not a float.
    if *i < chars.len() && chars[*i] == '.' && chars.get(*i + 1).is_some_and(char::is_ascii_digit) {
        float = true;
        *i += 1;
        while *i < chars.len() && chars[*i].is_ascii_digit() {
            *i += 1;
        }
    }
    if *i < chars.len() && matches!(chars[*i], 'e' | 'E') {
        let mut j = *i + 1;
        if matches!(chars.get(j), Some('+' | '-')) {
            j += 1;
        }
        if chars.get(j).is_some_and(char::is_ascii_digit) {
            float = true;
            *i = j;
            while *i < chars.len() && chars[*i].is_ascii_digit() {
                *i += 1;
            }
        }
    }
    let text: String = chars[start..*i].iter().collect();
    if float {
        text.parse()
            .map(Tok::Float)
            .map_err(|_| CypherError::syntax(start, "invalid float"))
    } else {
        text.parse()
            .map(Tok::Int)
            .map_err(|_| CypherError::syntax(start, "integer out of range"))
    }
}

fn string(chars: &[char], i: &mut usize, quote: char) -> Result<String> {
    let start = *i;
    *i += 1;
    let mut s = String::new();
    loop {
        let Some(&c) = chars.get(*i) else {
            return Err(CypherError::syntax(start, "unterminated string"));
        };
        *i += 1;
        if c == quote {
            return Ok(s);
        }
        if c != '\\' {
            s.push(c);
            continue;
        }
        let Some(&e) = chars.get(*i) else {
            return Err(CypherError::syntax(start, "unterminated string"));
        };
        *i += 1;
        match e {
            'n' => s.push('\n'),
            't' => s.push('\t'),
            'r' => s.push('\r'),
            'b' => s.push('\u{8}'),
            'f' => s.push('\u{c}'),
            '0' => s.push('\0'),
            'u' | 'U' => {
                let n = if e == 'u' { 4 } else { 8 };
                let hex: String = chars.get(*i..*i + n).unwrap_or_default().iter().collect();
                let cp = u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| CypherError::syntax(*i, "invalid unicode escape"))?;
                s.push(cp);
                *i += n;
            }
            other => s.push(other),
        }
    }
}
