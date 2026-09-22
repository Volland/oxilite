//! Optional full-text search over string literals with SQLite FTS5.
//!
//! When `StoreOptions::text_index` is set, `terms_fts` indexes the lexical form of every simple,
//! language-tagged and directional string literal (triggers on `terms` keep it current), and
//! the SPARQL function `oxl:textMatch(?literal, "fts query")` compiles to an FTS5 `MATCH`
//! lookup. Without the index, the function is evaluated by the fallback evaluator with the
//! same tokenization (case-insensitive words; `word*` prefixes; all terms required).
//!
// @lat: [[architecture#Text search]]

use crate::encoding::PAYLOAD_BITS;
use crate::sql::Statement;
use oxrdf::{Literal, NamedNode, Term};

/// `oxl:textMatch(?literal, "query")`: does the literal match the full-text query?
pub const TEXT_MATCH: &str = "https://oxilite.dev/ns#textMatch";

/// Ids of string literal kinds (simple / `xsd:string`, language-tagged, directional).
fn is_text(x: &str) -> String {
    format!("(({x}) >> {PAYLOAD_BITS}) IN (3, 4, 10)")
}

/// Schema statements of the text index (FTS5 over `terms`, kept current by triggers, and
/// back-filled once when the index is enabled on an existing store).
pub fn schema_statements() -> Vec<Statement> {
    vec![
        Statement::new(
            "CREATE VIRTUAL TABLE IF NOT EXISTS terms_fts USING fts5(lex, content='terms', content_rowid='id', tokenize='unicode61 remove_diacritics 2')",
        ),
        Statement::new(format!(
            "CREATE TRIGGER IF NOT EXISTS terms_fts_insert AFTER INSERT ON terms WHEN {} \
             BEGIN INSERT INTO terms_fts(rowid, lex) VALUES (NEW.id, NEW.lex); END",
            is_text("NEW.id")
        )),
        Statement::new(format!(
            "CREATE TRIGGER IF NOT EXISTS terms_fts_delete AFTER DELETE ON terms WHEN {} \
             BEGIN INSERT INTO terms_fts(terms_fts, rowid, lex) VALUES ('delete', OLD.id, OLD.lex); END",
            is_text("OLD.id")
        )),
        Statement::new(format!(
            "INSERT INTO terms_fts(rowid, lex) SELECT id, lex FROM terms WHERE {} \
             AND NOT EXISTS (SELECT 1 FROM oxilite_meta WHERE key = 'text_index' AND value = '1')",
            is_text("id")
        )),
        Statement::new("INSERT OR REPLACE INTO oxilite_meta(key, value) VALUES ('text_index', '1')"),
    ]
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The fallback evaluation of `textMatch` (no index, or a query the compiler cannot express).
pub fn text_match(args: &[Term]) -> Option<Term> {
    let [Term::Literal(value), Term::Literal(query)] = args else {
        return None;
    };
    let words = tokens(value.value());
    let matched = query
        .value()
        .split_whitespace()
        .map(|t| t.trim_matches('"'))
        .filter(|t| !t.is_empty() && *t != "AND")
        .all(|t| match t.strip_suffix('*') {
            Some(prefix) => {
                let p = prefix.to_lowercase();
                words.iter().any(|w| w.starts_with(&p))
            }
            None => tokens(t).iter().all(|q| words.contains(q)),
        });
    Some(Literal::from(matched).into())
}

/// The function's name.
pub fn text_match_name() -> NamedNode {
    NamedNode::new_unchecked(TEXT_MATCH)
}
