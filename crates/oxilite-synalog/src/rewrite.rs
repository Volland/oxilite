//! Making Synalog's SQLite output portable: runtime helpers rewritten to plain SQL, what has no
//! SQL form rejected, and the backend's limits checked — all before a request is sent.
//!
//! Synalog's SQLite dialect assumes the Logica runtime registered helper functions on the
//! connection. D1 cannot register functions, so the store never relies on them.
//!
// @lat: [[architecture#Synalog frontend#Portable SQL]]

use crate::error::{Result, SynalogError};
use oxilite_core::sql::Capabilities;

/// Builds the replacement of a call from its arguments.
type Rewrite = fn(&[&str]) -> String;

/// Helpers with a plain SQLite form: name, arity, rewrite.
const REWRITES: &[(&str, usize, Rewrite)] = &[
    // Only there to stop an optimizer hoisting a subquery; SQLite does not need it.
    ("MagicalEntangle", 2, |a| format!("({})", a[0])),
    ("IN_LIST", 2, |a| {
        format!("(({}) IN (SELECT value FROM JSON_EACH({})))", a[0], a[1])
    }),
    ("JOIN_STRINGS", 2, |a| {
        format!(
            "(SELECT GROUP_CONCAT(value, {}) FROM (SELECT value FROM JSON_EACH({}) ORDER BY key))",
            a[1], a[0]
        )
    }),
    ("DistinctListAgg", 1, |a| {
        format!("JSON_GROUP_ARRAY(DISTINCT {})", a[0])
    }),
    ("SortList", 1, |a| {
        format!(
            "(SELECT JSON_GROUP_ARRAY(value) FROM (SELECT value FROM JSON_EACH({}) ORDER BY value))",
            a[0]
        )
    }),
];

/// Runtime functions with no SQLite form, and what to tell the author.
const UNSUPPORTED: &[(&str, &str)] = &[
    ("ArgMax", "`ArgMax=` / `ArgMaxK` need a Logica runtime aggregate; use `Max=` and join back"),
    ("ArgMin", "`ArgMin=` / `ArgMinK` / `Array=` need a Logica runtime aggregate; use `Min=` and join back"),
    ("ReadFile", "`ReadFile` / `ReadJson` read the local file system"),
    ("WriteFile", "`WriteFile` writes the local file system"),
    ("Fingerprint", "`Fingerprint` needs a Logica runtime function"),
    ("Intelligence", "`Intelligence` calls a language model from SQL"),
    ("RunClingo", "`RunClingo` runs an external solver"),
    ("RunClingoFile", "`RunClingoFile` runs an external solver"),
    ("AssembleRecord", "`AssembleRecord` needs a Logica runtime function"),
    ("DisassembleRecord", "`DisassembleRecord` needs a Logica runtime function"),
    ("PrintToConsole", "`PrintToConsole` needs a Logica runtime function"),
    ("ARRAY_AGG", "`SomeValue` compiles to BigQuery syntax on SQLite"),
];

/// Turns the SQL Synalog produced for SQLite into one statement the store can run anywhere.
pub fn portable(sql: &str) -> Result<String> {
    let sql = single_statement(sql)?;
    let mut out = sql.to_owned();
    // A rewrite keeps its arguments' text, so nested helpers surface in a later pass.
    loop {
        let mut changed = false;
        for (name, arity, rewrite) in REWRITES {
            while let Some(call) = find_call(&out, name) {
                if call.args.len() != *arity {
                    return Err(SynalogError::unsupported(format!(
                        "`{name}` called with {} arguments, expected {arity}",
                        call.args.len()
                    )));
                }
                let args: Vec<&str> = call.args.iter().map(|&(a, b)| out[a..b].trim()).collect();
                let replacement = rewrite(&args);
                out.replace_range(call.start..call.end, &replacement);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (name, why) in UNSUPPORTED {
        if find_call(&out, name).is_some() {
            return Err(SynalogError::unsupported(format!(
                "{why}, which the store cannot provide on SQLite or D1"
            )));
        }
    }
    Ok(out)
}

/// The one statement of a compiled program, without its trailing semicolons.
fn single_statement(sql: &str) -> Result<&str> {
    let body = sql
        .trim()
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace());
    let first = words(body).next().map(|w| w.to_ascii_uppercase());
    let several = top_level(body).any(|(_, c)| c == b';');
    if several || matches!(first.as_deref(), Some("ATTACH" | "CREATE" | "DROP")) {
        return Err(SynalogError::unsupported(
            "the program compiles to several statements that create tables (`@Ground`, \
             `@AttachDatabase`, or a `@Recursive` bound above 20, which Synalog evaluates \
             iteratively); a read on the store runs exactly one",
        ));
    }
    Ok(body)
}

/// Checks a statement against the backend's limits, so D1 reports a Synalog error rather than
/// an opaque SQLite one.
pub fn check_limits(sql: &str, caps: &Capabilities) -> Result<()> {
    if sql.len() > caps.max_sql_len {
        return Err(SynalogError::unsupported(format!(
            "the compiled statement is {} bytes; {} allows {} (lower the @Recursive bound)",
            sql.len(),
            caps.name,
            caps.max_sql_len
        )));
    }
    let terms = longest_compound(sql);
    if terms > caps.max_compound_select {
        return Err(SynalogError::unsupported(format!(
            "a compound SELECT has {terms} terms; {} allows {} (fewer rules per predicate)",
            caps.name, caps.max_compound_select
        )));
    }
    Ok(())
}

/// Every lower-cased identifier outside string literals, quoted identifiers and comments.
pub fn identifiers(sql: &str) -> impl Iterator<Item = String> + '_ {
    words(sql).map(|w| w.to_ascii_lowercase())
}

/// A call found in SQL text: the byte range of the whole call and of each argument.
struct Call {
    start: usize,
    end: usize,
    args: Vec<(usize, usize)>,
}

/// The first call of `name` (case-insensitive, as SQLite resolves functions).
fn find_call(sql: &str, name: &str) -> Option<Call> {
    let bytes = sql.as_bytes();
    for (start, word) in word_spans(sql) {
        if !word.eq_ignore_ascii_case(name) {
            continue;
        }
        let mut open = start + word.len();
        while open < bytes.len() && bytes[open].is_ascii_whitespace() {
            open += 1;
        }
        if bytes.get(open) != Some(&b'(') {
            continue;
        }
        let mut depth = 0usize;
        let mut args = Vec::new();
        let mut arg_start = open + 1;
        for (i, c) in top_level_from(sql, open) {
            match c {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        if !sql[arg_start..i].trim().is_empty() || !args.is_empty() {
                            args.push((arg_start, i));
                        }
                        return Some(Call {
                            start,
                            end: i + 1,
                            args,
                        });
                    }
                }
                b',' if depth == 1 => {
                    args.push((arg_start, i));
                    arg_start = i + 1;
                }
                _ => {}
            }
        }
        return None;
    }
    None
}

/// The number of terms in the longest compound SELECT, counted per parenthesis depth so that a
/// nested compound is measured on its own.
fn longest_compound(sql: &str) -> usize {
    let mut counts = vec![1usize];
    let mut max = 1;
    let mut word = String::new();
    for (_, c) in top_level(sql).chain(std::iter::once((sql.len(), b' '))) {
        if c.is_ascii_alphanumeric() || c == b'_' {
            word.push(c as char);
            continue;
        }
        if ["UNION", "EXCEPT", "INTERSECT"]
            .iter()
            .any(|k| word.eq_ignore_ascii_case(k))
        {
            let n = counts.last_mut().expect("never empty");
            *n += 1;
            max = max.max(*n);
        }
        word.clear();
        match c {
            b'(' => counts.push(1),
            b')' if counts.len() > 1 => {
                counts.pop();
            }
            _ => {}
        }
    }
    max
}

/// Bytes of SQL outside string literals, quoted identifiers and `--` comments, with their offsets.
fn top_level(sql: &str) -> impl Iterator<Item = (usize, u8)> + '_ {
    top_level_from(sql, 0)
}

fn top_level_from(sql: &str, from: usize) -> impl Iterator<Item = (usize, u8)> + '_ {
    let bytes = sql.as_bytes();
    let mut i = from;
    std::iter::from_fn(move || {
        while i < bytes.len() {
            let c = bytes[i];
            match c {
                b'\'' | b'"' | b'`' => {
                    // A doubled quote inside a literal is an escaped quote.
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == c {
                            if bytes.get(i + 1) == Some(&c) {
                                i += 2;
                                continue;
                            }
                            break;
                        }
                        i += 1;
                    }
                    i += 1;
                }
                b'-' if bytes.get(i + 1) == Some(&b'-') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                }
                _ => {
                    i += 1;
                    return Some((i - 1, c));
                }
            }
        }
        None
    })
}

/// Identifier-like words outside literals and comments, with their offsets.
fn word_spans(sql: &str) -> impl Iterator<Item = (usize, &str)> + '_ {
    let bytes = sql.as_bytes();
    let mut chars = top_level(sql).peekable();
    std::iter::from_fn(move || {
        while let Some((i, c)) = chars.next() {
            if !(c.is_ascii_alphabetic() || c == b'_') {
                continue;
            }
            // A word cannot start in the middle of another (`t_0_triples`, `x1`).
            if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
                continue;
            }
            let mut end = i + 1;
            while let Some(&(j, d)) = chars.peek() {
                if j == end && (d.is_ascii_alphanumeric() || d == b'_') {
                    end += 1;
                    chars.next();
                } else {
                    break;
                }
            }
            return Some((i, &sql[i..end]));
        }
        None
    })
}

fn words(sql: &str) -> impl Iterator<Item = &str> + '_ {
    word_spans(sql).map(|(_, w)| w)
}

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[tests#Synalog#Scanner ignores literals]]
    #[test]
    fn scanner_ignores_literals() {
        let sql = "SELECT 'MagicalEntangle(1, 2)' AS s, \"ArgMax(x)\" FROM t -- ArgMax(y)\n";
        assert!(find_call(sql, "MagicalEntangle").is_none());
        assert!(find_call(sql, "ArgMax").is_none());
        assert_eq!(portable(sql).unwrap(), sql.trim());
        let ids: Vec<String> = identifiers("SELECT t_0_triples.x FROM 'triples' AS t").collect();
        assert_eq!(ids, ["select", "t_0_triples", "x", "from", "as", "t"]);
    }

    // @lat: [[tests#Synalog#Runtime helpers are rewritten]]
    #[test]
    fn runtime_helpers_are_rewritten() {
        let sql = "SELECT MIN(MagicalEntangle(1, x_6.value)) AS v, \
                   IN_LIST(a, JSON_ARRAY(1, 2)) AS i, \
                   DistinctListAgg(SortList(MagicalEntangle(b, ')'))) AS d FROM t;;";
        let out = portable(sql).unwrap();
        assert_eq!(
            out,
            "SELECT MIN((1)) AS v, \
             ((a) IN (SELECT value FROM JSON_EACH(JSON_ARRAY(1, 2)))) AS i, \
             JSON_GROUP_ARRAY(DISTINCT (SELECT JSON_GROUP_ARRAY(value) FROM (SELECT value FROM JSON_EACH((b)) ORDER BY value))) AS d FROM t"
        );
    }

    #[test]
    fn several_statements_are_rejected() {
        let err = portable("DROP TABLE IF EXISTS g; SELECT 1;").unwrap_err();
        assert!(err.to_string().contains("@Ground"), "{err}");
    }

    #[test]
    fn compound_terms_are_counted_per_depth() {
        assert_eq!(longest_compound("SELECT 1"), 1);
        assert_eq!(
            longest_compound("SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 'union'"),
            3
        );
        assert_eq!(
            longest_compound("SELECT * FROM (SELECT 1 UNION ALL SELECT 2) UNION SELECT 3"),
            2
        );
    }
}
