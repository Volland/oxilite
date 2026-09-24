//! Editor intelligence for SPARQL and the Turtle family: syntax diagnostics from the strict
//! parsers, and completion, hover targets and symbols from the lenient scanner.
//!
// @lat: [[architecture#Studio server#Language features]]

use super::conn::Vocab;
use super::index::SourceIndex;
use super::scanner::{scan, Kind, Pos, Role, Scan, Token, RDF_TYPE};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Diagnostic, DiagnosticSeverity,
    Documentation, MarkupContent, MarkupKind, Position, Range, TextEdit,
};
use oxilite::io::RdfParser;
use oxilite::sparql::SparqlParser;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Sparql,
    Turtle,
    Datalog,
    Cypher,
}

impl Lang {
    pub fn from_id(id: &str, uri: &str) -> Option<Self> {
        match id {
            "sparql" => Some(Self::Sparql),
            "turtle" => Some(Self::Turtle),
            "datalog" => Some(Self::Datalog),
            "cypher" => Some(Self::Cypher),
            _ => match uri.rsplit('.').next()? {
                "rq" | "ru" | "sparql" => Some(Self::Sparql),
                "ttl" | "trig" | "nt" | "nq" | "n3" => Some(Self::Turtle),
                "dl" => Some(Self::Datalog),
                "cypher" | "cyp" => Some(Self::Cypher),
                _ => None,
            },
        }
    }
}

pub const WELL_KNOWN: &[(&str, &str)] = &[
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("owl", "http://www.w3.org/2002/07/owl#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("sh", "http://www.w3.org/ns/shacl#"),
    ("skos", "http://www.w3.org/2004/02/skos/core#"),
    ("dcterms", "http://purl.org/dc/terms/"),
    ("foaf", "http://xmlns.com/foaf/0.1/"),
    ("schema", "https://schema.org/"),
    ("prov", "http://www.w3.org/ns/prov#"),
];

const SPARQL_KEYWORDS: &[&str] = &[
    "SELECT",
    "CONSTRUCT",
    "DESCRIBE",
    "ASK",
    "WHERE",
    "PREFIX",
    "BASE",
    "DISTINCT",
    "OPTIONAL",
    "UNION",
    "MINUS",
    "GRAPH",
    "FILTER",
    "BIND",
    "VALUES",
    "EXISTS",
    "NOT EXISTS",
    "ORDER BY",
    "GROUP BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "INSERT DATA",
    "DELETE DATA",
    "INSERT",
    "DELETE",
    "SERVICE",
    "COUNT",
    "SUM",
    "MIN",
    "MAX",
    "AVG",
    "SAMPLE",
    "GROUP_CONCAT",
    "STR",
    "LANG",
    "DATATYPE",
    "REGEX",
    "CONTAINS",
    "STRSTARTS",
    "IF",
    "COALESCE",
    "BOUND",
    "isIRI",
    "isLiteral",
];

/// An LSP position from a zero-based line and a column in characters.
fn lsp_pos(text: &str, line: u32, char_col: u32) -> Position {
    let col = text
        .lines()
        .nth(line as usize)
        .map(|l| {
            l.chars()
                .take(char_col as usize)
                .map(|c| c.len_utf16() as u32)
                .sum()
        })
        .unwrap_or(char_col);
    Position::new(line, col)
}

fn error(range: Range, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("oxilite".into()),
        message,
        ..Diagnostic::default()
    }
}

fn point(p: Position) -> Range {
    Range::new(p, Position::new(p.line, p.character + 1))
}

/// Syntax errors of a SPARQL query or update, or of a Turtle-family document.
pub fn syntax_diagnostics(lang: Lang, text: &str, uri: &str) -> Vec<Diagnostic> {
    match lang {
        Lang::Sparql => sparql_diagnostics(text, uri),
        Lang::Turtle => turtle_diagnostics(text, uri),
        Lang::Datalog => datalog_diagnostics(text),
        Lang::Cypher => cypher_diagnostics(text),
    }
}

/// Parse errors, then oxilite's own program checks (safety, stratification, arities).
fn datalog_diagnostics(text: &str) -> Vec<Diagnostic> {
    use oxilite::datalog::DatalogError;
    let program = match oxilite::datalog::parse(text) {
        Ok(p) => p,
        Err(DatalogError::Parse { span, message }) => {
            let p = lsp_pos(
                text,
                span.line.saturating_sub(1),
                span.column.saturating_sub(1),
            );
            return vec![error(point(p), message)];
        }
        Err(e) => return vec![error(point(Position::new(0, 0)), e.to_string())],
    };
    match oxilite::datalog::program::analyse(&program) {
        Ok(_) => Vec::new(),
        Err(e) => {
            // Point at the rule head the error names, when there is one.
            let name = match &e {
                DatalogError::Unsafe { predicate, .. }
                | DatalogError::NotTripleShaped { predicate } => Some(predicate.clone()),
                DatalogError::Arity { predicate, .. } => Some(predicate.clone()),
                DatalogError::UnknownGoal(p) => Some(p.clone()),
                DatalogError::UnknownPrefix(p) => Some(format!("{p}:")),
                _ => None,
            };
            let at = name
                .and_then(|n| {
                    let short = n.trim_start_matches('<').trim_end_matches('>');
                    let local = short.rsplit(['/', '#']).next().unwrap_or(short);
                    text.find(short).or_else(|| text.find(local))
                })
                .map_or(Position::new(0, 0), |offset| offset_pos(text, offset));
            vec![error(point(at), e.to_string())]
        }
    }
}

fn cypher_diagnostics(text: &str) -> Vec<Diagnostic> {
    match oxilite::cypher::parse(text) {
        Ok(_) => Vec::new(),
        Err(oxilite::cypher::CypherError::Syntax { pos, message }) => {
            vec![error(point(offset_pos(text, pos)), message)]
        }
        Err(e) => vec![error(point(Position::new(0, 0)), e.to_string())],
    }
}

/// The LSP position of a byte offset.
fn offset_pos(text: &str, offset: usize) -> Position {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() as u32;
    let col = before
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    Position::new(line, col)
}

fn sparql_diagnostics(text: &str, uri: &str) -> Vec<Diagnostic> {
    let parser = || {
        SparqlParser::new()
            .with_base_iri(uri)
            .unwrap_or_else(|_| SparqlParser::new())
    };
    let query = match parser().parse_query(text) {
        Ok(_) => return Vec::new(),
        Err(e) => e,
    };
    let update = match parser().parse_update(text) {
        Ok(_) => return Vec::new(),
        Err(e) => e,
    };
    // Report the error of the form the text looks like.
    let looks_like_update = scan(text, None).tokens.iter().any(|t| {
        matches!(&t.kind, Kind::Word(w) if matches!(w.to_ascii_uppercase().as_str(),
            "INSERT" | "DELETE" | "LOAD" | "CLEAR" | "DROP" | "CREATE" | "ADD" | "MOVE" | "COPY" | "WITH"))
    });
    let e = if looks_like_update { update } else { query };
    let message = e.to_string();
    let at = regex::Regex::new(r"at (\d+):(\d+)")
        .expect("valid regex")
        .captures(&message)
        .and_then(|c| Some((c[1].parse::<u32>().ok()?, c[2].parse::<u32>().ok()?)));
    let p = at.map_or(Position::new(0, 0), |(l, c)| {
        lsp_pos(text, l.saturating_sub(1), c.saturating_sub(1))
    });
    vec![error(point(p), message)]
}

fn turtle_diagnostics(text: &str, uri: &str) -> Vec<Diagnostic> {
    let format = super::project::format_of(std::path::Path::new(uri))
        .unwrap_or(oxilite::io::RdfFormat::Turtle);
    let mut parser = RdfParser::from_format(format);
    if let Ok(p) = parser.clone().with_base_iri(uri) {
        parser = p;
    }
    let mut out = Vec::new();
    for q in parser.for_slice(text.as_bytes()) {
        if let Err(e) = q {
            let range = e.location().map_or(point(Position::new(0, 0)), |r| {
                let start = lsp_pos(text, r.start.line as u32, r.start.column as u32);
                let end = lsp_pos(text, r.end.line as u32, r.end.column as u32);
                if end == start {
                    point(start)
                } else {
                    Range::new(start, end)
                }
            });
            out.push(error(range, e.to_string()));
            if out.len() >= 50 {
                break;
            }
        }
    }
    out
}

/// Warnings for SPARQL predicates the connected store never uses: usually a typo.
pub fn vocabulary_warnings(text: &str, uri: &str, vocab: &Vocab) -> Vec<Diagnostic> {
    if vocab.predicates.is_empty() {
        return Vec::new();
    }
    scan(text, Some(uri))
        .tokens
        .iter()
        .filter(|t| {
            t.role == Role::Predicate && matches!(t.kind, Kind::Iri(_) | Kind::PName { .. })
        })
        .filter_map(|t| {
            let iri = t.iri.as_ref()?;
            (vocab.predicate_count(iri).is_none() && iri != RDF_TYPE).then(|| Diagnostic {
                range: range(t),
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("oxilite".into()),
                message: format!("<{iri}> is not used as a predicate in the connected store"),
                ..Diagnostic::default()
            })
        })
        .collect()
}

pub fn range(t: &Token) -> Range {
    Range::new(
        Position::new(t.start.line, t.start.col),
        Position::new(t.end.line, t.end.col),
    )
}

fn pos(p: Position) -> Pos {
    Pos {
        line: p.line,
        col: p.character,
    }
}

/// The IRI under the cursor, and the range of its token.
pub fn iri_at(text: &str, uri: &str, p: Position) -> Option<(String, Range)> {
    let s = scan(text, Some(uri));
    let t = s.at(pos(p))?;
    Some((t.iri.clone()?, range(t)))
}

/// Subjects of a Turtle document, for the outline.
pub fn subjects(text: &str, uri: &str) -> Vec<(String, Range)> {
    let mut seen = BTreeSet::new();
    scan(text, Some(uri))
        .tokens
        .iter()
        .filter(|t| t.role == Role::Subject && matches!(t.kind, Kind::Iri(_) | Kind::PName { .. }))
        .filter_map(|t| {
            let iri = t.iri.clone()?;
            seen.insert(iri)
                .then(|| (text[t.from..t.to].to_string(), range(t)))
        })
        .collect()
}

pub struct CompletionContext<'a> {
    pub vocab: Option<&'a Vocab>,
    pub index: &'a SourceIndex,
    /// How Cypher names map to IRIs on the active connection.
    pub cypher: Option<&'a oxilite::cypher::Vocabulary>,
}

/// Cypher: labels after `(x:`, relationship types after `[r:`, property keys after `x.`.
fn cypher_completion(text: &str, p: Position, ctx: &CompletionContext<'_>) -> Vec<CompletionItem> {
    let (Some(v), Some(names)) = (ctx.vocab, ctx.cypher) else {
        return Vec::new();
    };
    let line = text.lines().nth(p.line as usize).unwrap_or("");
    let before: String = {
        let mut units = 0u32;
        line.chars()
            .take_while(|c| {
                units += c.len_utf16() as u32;
                units <= p.character
            })
            .collect()
    };
    let word_start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(before.len(), |(i, _)| i);
    let sigil = before[..word_start].chars().last();
    let open_brackets = text[..text
        .lines()
        .take(p.line as usize)
        .map(|l| l.len() + 1)
        .sum::<usize>()
        + before.len()]
        .chars()
        .fold(0i32, |d, c| match c {
            '[' => d + 1,
            ']' => d - 1,
            _ => d,
        });
    let pool: Vec<(&String, &u64, CompletionItemKind)> = match sigil {
        Some(':') if open_brackets > 0 => v
            .predicates
            .iter()
            .map(|(i, n)| (i, n, CompletionItemKind::FIELD))
            .collect(),
        Some(':') => v
            .classes
            .iter()
            .map(|(i, n)| (i, n, CompletionItemKind::CLASS))
            .collect(),
        Some('.') => v
            .predicates
            .iter()
            .map(|(i, n)| (i, n, CompletionItemKind::PROPERTY))
            .collect(),
        _ => return Vec::new(),
    };
    let r = Range::new(
        Position::new(
            p.line,
            p.character
                - before[word_start..]
                    .chars()
                    .map(|c| c.len_utf16() as u32)
                    .sum::<u32>(),
        ),
        p,
    );
    pool.into_iter()
        .take(300)
        .enumerate()
        .map(|(i, (iri, n, kind))| {
            let name = names.name(iri);
            let insert = if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                name.clone()
            } else {
                format!("`{name}`")
            };
            CompletionItem {
                label: name,
                kind: Some(kind),
                detail: Some(format!("{n} uses · <{iri}>")),
                sort_text: Some(format!("{i:05}")),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(r, insert))),
                ..CompletionItem::default()
            }
        })
        .collect()
}

/// Completion at `p`: prefixed names in the right role, prefixes, variables and keywords.
pub fn complete(
    lang: Lang,
    text: &str,
    uri: &str,
    p: Position,
    ctx: &CompletionContext<'_>,
) -> Vec<CompletionItem> {
    if lang == Lang::Cypher {
        return cypher_completion(text, p, ctx);
    }
    let s = scan(text, Some(uri));
    let at = pos(p);
    let current = s.tokens.iter().find(|t| t.start < at && at <= t.end);
    // In Datalog every atom's name is a predicate or a class.
    let role = match lang {
        Lang::Datalog => Role::Predicate,
        _ => current
            .map(|t| t.role)
            .unwrap_or_else(|| role_after(&s, at)),
    };
    let after_type =
        previous_predicate(&s, at).is_some_and(|i| s.tokens[i].iri.as_deref() == Some(RDF_TYPE));
    let known = known_prefixes(&s, ctx.index);
    let mut items = Vec::new();
    match current.map(|t| &t.kind) {
        Some(Kind::Var(_)) => {
            let vars: BTreeSet<_> = s
                .tokens
                .iter()
                .filter_map(|t| match &t.kind {
                    Kind::Var(v) if !v.is_empty() && Some(t.from) != current.map(|c| c.from) => {
                        Some(v.clone())
                    }
                    _ => None,
                })
                .collect();
            let r = range(current.expect("current token"));
            for v in vars {
                let label = format!("?{v}");
                items.push(CompletionItem {
                    label: label.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(r, label))),
                    ..CompletionItem::default()
                });
            }
        }
        Some(Kind::PName { prefix, .. }) => {
            let t = current.expect("current token");
            if let Some(ns) = known.get(prefix) {
                let r = range(t);
                for (i, (iri, detail)) in candidates(ns, role, after_type, ctx)
                    .into_iter()
                    .enumerate()
                {
                    let local = &iri[ns.len()..];
                    let (label, insert) = if valid_local(local) {
                        (format!("{prefix}:{local}"), format!("{prefix}:{local}"))
                    } else {
                        (format!("{prefix}:{local}"), format!("<{iri}>"))
                    };
                    items.push(term_item(label, insert, r, i, detail, &iri, ctx));
                }
                if let Some(edit) = declare(lang, &s, prefix, ns) {
                    for item in &mut items {
                        item.additional_text_edits = Some(vec![edit.clone()]);
                    }
                }
            }
        }
        Some(Kind::Iri(typed)) => {
            let t = current.expect("current token");
            let r = range(t);
            let mut seen = BTreeSet::new();
            for (i, (iri, detail)) in candidates(typed, role, after_type, ctx)
                .into_iter()
                .enumerate()
            {
                if seen.insert(iri.clone()) {
                    items.push(term_item(
                        iri.clone(),
                        format!("<{iri}>"),
                        r,
                        i,
                        detail,
                        &iri,
                        ctx,
                    ));
                }
            }
        }
        _ => {
            let r = current.map_or(Range::new(p, p), range);
            for (prefix, ns) in &known {
                let mut item = CompletionItem {
                    label: format!("{prefix}:"),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some(ns.clone()),
                    sort_text: Some(format!("2{prefix}")),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                        r,
                        format!("{prefix}:"),
                    ))),
                    command: Some(lsp_types::Command::new(
                        "suggest".into(),
                        "editor.action.triggerSuggest".into(),
                        None,
                    )),
                    ..CompletionItem::default()
                };
                if let Some(edit) = declare(lang, &s, prefix, ns) {
                    item.additional_text_edits = Some(vec![edit]);
                }
                items.push(item);
            }
            // The store's most used terms, as prefixed names where a prefix is known.
            if matches!(role, Role::Predicate | Role::Object) {
                let pool = if role == Role::Predicate {
                    ctx.vocab.map(|v| &v.predicates)
                } else if after_type {
                    ctx.vocab.map(|v| &v.classes)
                } else {
                    None
                };
                for (i, (iri, n)) in pool.into_iter().flatten().take(100).enumerate() {
                    if let Some((prefix, ns)) =
                        known.iter().find(|(_, ns)| iri.starts_with(ns.as_str()))
                    {
                        let local = &iri[ns.len()..];
                        if !valid_local(local) {
                            continue;
                        }
                        let label = format!("{prefix}:{local}");
                        let mut item =
                            term_item(label.clone(), label, r, i, format!("{n} uses"), iri, ctx);
                        if let Some(edit) = declare(lang, &s, prefix, ns) {
                            item.additional_text_edits = Some(vec![edit]);
                        }
                        items.push(item);
                    }
                }
                if role == Role::Predicate {
                    items.push(CompletionItem {
                        label: "a".into(),
                        kind: Some(CompletionItemKind::KEYWORD),
                        detail: Some("rdf:type".into()),
                        sort_text: Some("0a".into()),
                        ..CompletionItem::default()
                    });
                }
            }
            if lang == Lang::Sparql {
                for k in SPARQL_KEYWORDS {
                    items.push(CompletionItem {
                        label: (*k).into(),
                        kind: Some(CompletionItemKind::KEYWORD),
                        sort_text: Some(format!("3{k}")),
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(r, (*k).into()))),
                        ..CompletionItem::default()
                    });
                }
            }
        }
    }
    items
}

fn term_item(
    label: String,
    insert: String,
    r: Range,
    rank: usize,
    detail: String,
    iri: &str,
    ctx: &CompletionContext<'_>,
) -> CompletionItem {
    let doc = ctx.vocab.and_then(|v| {
        let label = v.labels.get(iri);
        let comment = v.comments.get(iri);
        (label.is_some() || comment.is_some()).then(|| {
            format!(
                "{}{}\n\n`<{iri}>`",
                label.map(|l| format!("**{l}**\n\n")).unwrap_or_default(),
                comment.cloned().unwrap_or_default()
            )
        })
    });
    CompletionItem {
        label,
        kind: Some(CompletionItemKind::REFERENCE),
        detail: Some(detail),
        sort_text: Some(format!("1{rank:05}")),
        filter_text: Some(insert.clone()),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(r, insert))),
        documentation: doc.map(|value| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            })
        }),
        ..CompletionItem::default()
    }
}

/// IRIs under `namespace` for a position: predicates for predicates, classes after `a`.
fn candidates(
    namespace: &str,
    role: Role,
    after_type: bool,
    ctx: &CompletionContext<'_>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut add = |iri: &str, detail: String, out: &mut Vec<(String, String)>| {
        if iri.starts_with(namespace) && iri.len() > namespace.len() && seen.insert(iri.to_string())
        {
            out.push((iri.to_string(), detail));
        }
    };
    if let Some(v) = ctx.vocab {
        let (first, second) = match role {
            Role::Predicate => (&v.predicates, None),
            Role::Object if after_type => (&v.classes, None),
            _ => (&v.classes, Some(&v.predicates)),
        };
        for (iri, n) in first {
            add(iri, format!("{n} uses"), &mut out);
        }
        for (iri, n) in second.into_iter().flatten() {
            add(iri, format!("{n} uses"), &mut out);
        }
    }
    let in_role = match role {
        Role::Predicate => Some(Role::Predicate),
        Role::Object if after_type => Some(Role::Object),
        _ => None,
    };
    for (iri, n) in ctx.index.iris_in(namespace, in_role) {
        // After `a`, only IRIs something is typed with.
        if after_type && ctx.vocab.is_some_and(|v| v.class_count(&iri).is_none()) {
            continue;
        }
        add(&iri, format!("{n} mentions in the workspace"), &mut out);
    }
    out.truncate(500);
    out
}

fn valid_local(local: &str) -> bool {
    !local.is_empty()
        && !local.ends_with('.')
        && local
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !local.starts_with('-')
        && !local.starts_with('.')
}

/// The role a new term typed at `at` would get: the role after the preceding tokens.
fn role_after(s: &Scan, at: Pos) -> Role {
    let mut text = String::new();
    // Re-scan with a placeholder term inserted, and read its role.
    let prev = s
        .tokens
        .iter()
        .take_while(|t| t.end <= at)
        .collect::<Vec<_>>();
    for t in &prev {
        text.push_str(&token_text(t));
        text.push(' ');
    }
    text.push_str("<urn:x-cursor>");
    scan(&text, None)
        .tokens
        .last()
        .map_or(Role::Other, |t| t.role)
}

fn token_text(t: &Token) -> String {
    match &t.kind {
        Kind::Iri(i) => format!("<{i}>"),
        Kind::PName { prefix, local } => format!("{prefix}:{local}"),
        Kind::Var(v) => format!("?{v}"),
        Kind::Str => "\"s\"".into(),
        Kind::At(a) => format!("@{a}"),
        Kind::BNode => "_:b".into(),
        Kind::Number => "1".into(),
        Kind::Punct(p) => p.clone(),
        Kind::Word(w) => w.clone(),
    }
}

/// The last predicate-position token before `at`.
fn previous_predicate(s: &Scan, at: Pos) -> Option<usize> {
    s.tokens
        .iter()
        .rposition(|t| t.end <= at && t.role == Role::Predicate)
        .filter(|&i| {
            // Only when nothing but the object being typed follows it.
            s.tokens[i + 1..]
                .iter()
                .take_while(|t| t.end <= at)
                .all(|t| t.role == Role::Object)
        })
}

/// Prefixes the document declares, then the workspace's, then well-known ones.
fn known_prefixes(s: &Scan, index: &SourceIndex) -> BTreeMap<String, String> {
    let mut known: BTreeMap<String, String> = s
        .prefixes
        .iter()
        .map(|(p, (ns, _))| (p.clone(), ns.clone()))
        .collect();
    for (p, ns) in index.prefixes() {
        known.entry(p.clone()).or_insert_with(|| ns.clone());
    }
    for (p, ns) in WELL_KNOWN {
        known.entry((*p).into()).or_insert_with(|| (*ns).into());
    }
    known
}

/// The edit declaring `prefix` when the document does not, after its last declaration.
fn declare(lang: Lang, s: &Scan, prefix: &str, ns: &str) -> Option<TextEdit> {
    if s.prefixes.contains_key(prefix) {
        return None;
    }
    let line = s.prefixes.values().map(|(_, l)| l + 1).max().unwrap_or(0);
    let decl = match lang {
        Lang::Sparql => format!("PREFIX {prefix}: <{ns}>\n"),
        _ => format!("@prefix {prefix}: <{ns}> .\n"),
    };
    Some(TextEdit::new(
        Range::new(Position::new(line, 0), Position::new(line, 0)),
        decl,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocab {
        Vocab {
            predicates: vec![
                ("http://ex.org/name".into(), 10),
                ("http://ex.org/knows".into(), 3),
            ],
            classes: vec![("http://ex.org/Person".into(), 5)],
            ..Vocab::default()
        }
    }

    fn labels(items: &[CompletionItem]) -> Vec<String> {
        items.iter().map(|i| i.label.clone()).collect()
    }

    // @lat: [[tests#Studio server#Completion follows the triple role]]
    #[test]
    fn completion_follows_role() {
        let ix = SourceIndex::default();
        let v = vocab();
        let ctx = CompletionContext {
            vocab: Some(&v),
            index: &ix,
            cypher: None,
        };
        let q = "PREFIX ex: <http://ex.org/>\nSELECT * { ?s ex:";
        let items = complete(Lang::Sparql, q, "file:///q.rq", Position::new(1, 17), &ctx);
        assert_eq!(labels(&items), ["ex:name", "ex:knows"]);
        let q = "PREFIX ex: <http://ex.org/>\nSELECT * { ?s a ex:";
        let items = complete(Lang::Sparql, q, "file:///q.rq", Position::new(1, 19), &ctx);
        assert_eq!(labels(&items), ["ex:Person"]);
        let q = "SELECT * { ?s ?p ?o . ?s ";
        let items = complete(Lang::Sparql, q, "file:///q.rq", Position::new(0, 25), &ctx);
        let l = labels(&items);
        assert!(l.contains(&"rdf:".to_string()));
        assert!(l.contains(&"a".to_string()));
        assert!(l.contains(&"SELECT".to_string()));
        let q = "SELECT ?name WHERE { ?x <http://ex.org/name> ?name } ORDER BY ?n";
        let items = complete(
            Lang::Sparql,
            q,
            "file:///q.rq",
            Position::new(0, q.len() as u32),
            &ctx,
        );
        assert_eq!(labels(&items), ["?name", "?x"]);
    }

    // @lat: [[tests#Studio server#Completion declares missing prefixes]]
    #[test]
    fn completion_declares_prefix() {
        let mut ix = SourceIndex::default();
        ix.set_file(
            "file:///d.ttl",
            "@prefix ex: <http://ex.org/> .\nex:alice ex:name \"A\" .\n",
        );
        let v = vocab();
        let ctx = CompletionContext {
            vocab: Some(&v),
            index: &ix,
            cypher: None,
        };
        let q = "SELECT * { ?s ex:";
        let items = complete(Lang::Sparql, q, "file:///q.rq", Position::new(0, 17), &ctx);
        assert_eq!(items[0].label, "ex:name");
        let edit = &items[0].additional_text_edits.as_ref().unwrap()[0];
        assert_eq!(edit.new_text, "PREFIX ex: <http://ex.org/>\n");
    }

    // @lat: [[tests#Studio server#Syntax diagnostics as you type]]
    #[test]
    fn syntax_positions() {
        let d = syntax_diagnostics(
            Lang::Sparql,
            "SELECT * WHERE {\n  ?s ?p \n}",
            "file:///q.rq",
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].range.start.line, 2);
        assert!(syntax_diagnostics(
            Lang::Sparql,
            "INSERT DATA { <a:b> <a:c> <a:d> }",
            "file:///q.ru"
        )
        .is_empty());
        let d = syntax_diagnostics(
            Lang::Turtle,
            "@prefix ex: <http://ex.org/> .\nex:a ex:b .\nex:c ex:d ex:e .\nex:f .\n",
            "file:///d.ttl",
        );
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].range.start.line, 1);
        assert_eq!(d[1].range.start.line, 3);
        let w = vocabulary_warnings(
            "PREFIX ex: <http://ex.org/> SELECT * { ?s ex:nmae ?o }",
            "file:///q.rq",
            &vocab(),
        );
        assert_eq!(w.len(), 1);
    }
}
