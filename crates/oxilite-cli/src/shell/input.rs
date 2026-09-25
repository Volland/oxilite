//! When the lines typed so far make a statement to run.
//!
//! SPARQL has no terminator and `;` also separates predicate-object lists, so a statement is
//! complete when its brackets balance and it ends with `;`, is followed by an empty line, or is a
//! single line that parses.

/// Whether `(`, `[` and `{` are closed outside strings, IRIs and comments.
pub fn balanced(text: &str) -> bool {
    let b = text.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            q @ (b'"' | b'\'') => {
                let long = b[i..].starts_with(&[q, q, q]);
                i += if long { 3 } else { 1 };
                loop {
                    if i >= b.len() {
                        return false;
                    }
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if long && b[i..].starts_with(&[q, q, q]) {
                        i += 2;
                        break;
                    }
                    if !long && b[i] == q {
                        break;
                    }
                    if !long && b[i] == b'\n' {
                        return false;
                    }
                    i += 1;
                }
            }
            // An IRI runs to `>` without spaces; otherwise `<` is a comparison.
            b'<' => {
                if let Some(end) = b[i + 1..].iter().position(|c| {
                    matches!(
                        c,
                        b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'<' | b'"' | b'{' | b'}'
                    )
                }) {
                    if b[i + 1 + end] == b'>' {
                        i += end + 1;
                    }
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth <= 0
}

/// Whether the pending statement should run now.
///
/// `single_line` is true when the statement is only the line just entered, `line_empty` when
/// that line was blank, and `parses` checks the text (without a trailing `;`).
pub fn complete(
    text: &str,
    single_line: bool,
    line_empty: bool,
    parses: impl Fn(&str) -> bool,
) -> bool {
    let t = text.trim();
    if t.is_empty() || !balanced(t) {
        return false;
    }
    t.ends_with(';') || line_empty || (single_line && parses(t))
}

/// The statement without the trailing `;` that ended it.
pub fn strip_terminator(text: &str) -> &str {
    text.trim()
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace())
}

/// The first keyword after the prologue, to tell an update from a query.
pub fn is_update(text: &str) -> bool {
    const UPDATES: &[&str] = &[
        "INSERT", "DELETE", "LOAD", "CLEAR", "CREATE", "DROP", "COPY", "MOVE", "ADD", "WITH",
    ];
    crate::studio::scanner::scan(text, None)
        .tokens
        .iter()
        .find_map(|t| match &t.kind {
            crate::studio::scanner::Kind::Word(w)
                if !w.eq_ignore_ascii_case("PREFIX") && !w.eq_ignore_ascii_case("BASE") =>
            {
                Some(UPDATES.iter().any(|u| w.eq_ignore_ascii_case(u)))
            }
            _ => None,
        })
        .unwrap_or(false)
}
