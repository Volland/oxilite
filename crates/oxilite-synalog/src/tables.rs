//! The store as relational tables: `triples`, plus predicate and class tables a program
//! declares, each a common table expression over the quad table with terms decoded to native
//! SQL values.
//!
// @lat: [[architecture#Synalog frontend#Store tables]]

use crate::error::{Result, SynalogError};
use crate::Options;
use oxilite_core::encoding::{
    boolean_id, named_node_id, rdf_type_id, Tag, DEFAULT_GRAPH_ID, INT_OFFSET, PAYLOAD_MASK,
};
use oxrdf::vocab::{rdf, xsd};
use oxrdf::NamedNode;

/// The table every program on the store can read.
pub const TRIPLES: &str = "triples";

/// A table a program declares over one predicate or one class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Table {
    /// `NAME(subject, object, kind, datatype, lang, graph)`: the triples with this predicate.
    Predicate { name: String, iri: String },
    /// `NAME(subject, graph)`: the instances of this class (`rdf:type`).
    Class { name: String, iri: String },
}

impl Table {
    pub fn name(&self) -> &str {
        match self {
            Self::Predicate { name, .. } | Self::Class { name, .. } => name,
        }
    }
}

/// Reads the `# @table NAME <IRI>` and `# @class NAME <IRI>` pragmas of a program. They are
/// Synalog comments, so the program stays valid Synalog.
pub fn pragmas(src: &str) -> Result<Vec<Table>> {
    let mut out = Vec::new();
    for (n, line) in src.lines().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix('#') else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let kind = match parts.next() {
            Some("@table") => "table",
            Some("@class") => "class",
            _ => continue,
        };
        let err = |message: String| SynalogError::Pragma {
            line: n + 1,
            message,
        };
        let (Some(name), Some(iri), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(err(format!("expected `# @{kind} NAME <IRI>`")));
        };
        let iri = iri
            .strip_prefix('<')
            .and_then(|i| i.strip_suffix('>'))
            .ok_or_else(|| err(format!("the IRI of `{name}` must be written <…>")))?;
        out.push(if kind == "table" {
            Table::Predicate {
                name: name.to_owned(),
                iri: iri.to_owned(),
            }
        } else {
            Table::Class {
                name: name.to_owned(),
                iri: iri.to_owned(),
            }
        });
    }
    Ok(out)
}

/// Checks declared tables: lower-case names (Synalog reads tables by lower-case name), valid
/// IRIs, no duplicates and no shadowing of `triples`.
pub fn validate(tables: &[Table]) -> Result<()> {
    for (i, t) in tables.iter().enumerate() {
        let name = t.name();
        let valid = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid {
            return Err(SynalogError::unsupported(format!(
                "table name `{name}` must be lower case (letters, digits, `_`), as Synalog reads tables"
            )));
        }
        if name == TRIPLES || tables[..i].iter().any(|u| u.name() == name) {
            return Err(SynalogError::unsupported(format!(
                "table `{name}` is declared twice or shadows a built-in table"
            )));
        }
        let (Table::Predicate { iri, .. } | Table::Class { iri, .. }) = t;
        NamedNode::new(iri.as_str()).map_err(|e| {
            SynalogError::unsupported(format!("table `{name}`: invalid IRI <{iri}>: {e}"))
        })?;
    }
    Ok(())
}

/// Prepends the store tables a statement references, as `NOT MATERIALIZED` CTEs so SQLite can
/// push a caller's filters into them. Returns the statement and the tables injected.
pub fn inject(sql: &str, declared: &[Table], options: &Options) -> (String, Vec<String>) {
    let used: std::collections::HashSet<String> = crate::rewrite::identifiers(sql).collect();
    let source = quad_source(options);
    let mut ctes = Vec::new();
    let mut names = Vec::new();
    if used.contains(TRIPLES) {
        ctes.push(format!(
            "{TRIPLES} AS NOT MATERIALIZED (SELECT {s} AS subject, \
             (SELECT lex FROM terms WHERE id = q.p) AS predicate, {o} AS object, \
             {kind} AS kind, {dt} AS datatype, {lang} AS lang, {g} AS graph FROM {source} AS q{w})",
            s = value("q.s"),
            o = value("q.o"),
            kind = kind("q.o"),
            dt = datatype("q.o"),
            lang = lang("q.o"),
            g = graph(),
            w = graph_scope(options, " WHERE "),
        ));
        names.push(TRIPLES.to_owned());
    }
    for t in declared.iter().filter(|t| used.contains(t.name())) {
        ctes.push(match t {
            Table::Predicate { name, iri } => format!(
                "{name} AS NOT MATERIALIZED (SELECT {s} AS subject, {o} AS object, {kind} AS kind, \
                 {dt} AS datatype, {lang} AS lang, {g} AS graph FROM {source} AS q \
                 WHERE q.p = {p}{w})",
                s = value("q.s"),
                o = value("q.o"),
                kind = kind("q.o"),
                dt = datatype("q.o"),
                lang = lang("q.o"),
                g = graph(),
                p = named_node_id(iri),
                w = graph_scope(options, " AND "),
            ),
            Table::Class { name, iri } => format!(
                "{name} AS NOT MATERIALIZED (SELECT {s} AS subject, {g} AS graph FROM {source} AS q \
                 WHERE q.p = {p} AND q.o = {c}{w})",
                s = value("q.s"),
                g = graph(),
                p = rdf_type_id(),
                c = named_node_id(iri),
                w = graph_scope(options, " AND "),
            ),
        });
        names.push(t.name().to_owned());
    }
    if ctes.is_empty() {
        return (sql.to_owned(), names);
    }
    let prelude = ctes.join(",\n");
    let trimmed = sql.trim_start();
    let with = trimmed
        .get(..5)
        .is_some_and(|w| w.eq_ignore_ascii_case("WITH ") || w.eq_ignore_ascii_case("WITH\n"));
    let sql = if with {
        format!("WITH {prelude},\n{}", &trimmed[5..])
    } else {
        format!("WITH {prelude}\n{trimmed}")
    };
    (sql, names)
}

/// The `(s, p, o, g)` relation the tables read, as the Datalog compiler chooses it.
fn quad_source(options: &Options) -> String {
    if let Some(t) = options.as_of_tick {
        return oxilite_core::version::as_of_sql(&t.to_string());
    }
    if options.include_inferred {
        "(SELECT s, p, o, g FROM quads UNION ALL SELECT s, p, o, g FROM quads_inf)".to_owned()
    } else {
        "quads".to_owned()
    }
}

/// The default graph only, unless every graph is asked for. The unary plus keeps the planner
/// off the graph index when another column is bound (see the storage schema).
fn graph_scope(options: &Options, glue: &str) -> String {
    if options.union_default_graph {
        String::new()
    } else {
        format!("{glue}+q.g = {DEFAULT_GRAPH_ID}")
    }
}

fn range(tag: Tag) -> (i64, i64) {
    (tag.base(), tag.base() | PAYLOAD_MASK)
}

fn is(x: &str, tag: Tag) -> String {
    let (lo, hi) = range(tag);
    format!("{x} BETWEEN {lo} AND {hi}")
}

/// A term id as a native SQL value: numbers as numbers, booleans as 1/0, blank nodes as
/// `_:label`, triple terms as NULL, anything else as its lexical form.
fn value(x: &str) -> String {
    format!(
        "(CASE WHEN {int} THEN {x} - {zero} WHEN {x} = {t} THEN 1 WHEN {x} = {f} THEN 0 \
         WHEN {bnode} THEN '_:' || (SELECT lex FROM terms WHERE id = {x}) WHEN {triple} THEN NULL \
         ELSE (SELECT CASE WHEN nt IS NOT NULL THEN num ELSE lex END FROM terms WHERE id = {x}) END)",
        int = is(x, Tag::Integer),
        zero = Tag::Integer.base() + INT_OFFSET,
        t = boolean_id(true),
        f = boolean_id(false),
        bnode = is(x, Tag::BlankNode),
        triple = is(x, Tag::Triple),
    )
}

/// What kind of term an id is: `iri`, `blank`, `triple` or `literal`.
fn kind(x: &str) -> String {
    format!(
        "(CASE WHEN {iri} THEN 'iri' WHEN {bnode} THEN 'blank' WHEN {triple} THEN 'triple' \
         ELSE 'literal' END)",
        iri = is(x, Tag::Iri),
        bnode = is(x, Tag::BlankNode),
        triple = is(x, Tag::Triple),
    )
}

/// The datatype IRI of a literal id; NULL for anything else.
fn datatype(x: &str) -> String {
    format!(
        "(CASE WHEN {int} THEN '{xint}' WHEN {boolean} THEN '{xbool}' WHEN {string} THEN '{xstr}' \
         WHEN {lang} THEN '{lstr}' WHEN {dir} THEN '{dstr}' \
         WHEN {typed} THEN (SELECT dt FROM terms WHERE id = {x}) END)",
        int = is(x, Tag::Integer),
        boolean = is(x, Tag::Boolean),
        string = is(x, Tag::String),
        lang = is(x, Tag::LangString),
        dir = is(x, Tag::DirLangString),
        typed = is(x, Tag::Typed),
        xint = xsd::INTEGER.as_str(),
        xbool = xsd::BOOLEAN.as_str(),
        xstr = xsd::STRING.as_str(),
        lstr = rdf::LANG_STRING.as_str(),
        dstr = rdf::DIR_LANG_STRING.as_str(),
    )
}

/// The language tag of a language-tagged string id; NULL for anything else.
fn lang(x: &str) -> String {
    format!(
        "(CASE WHEN {lang} OR {dir} THEN (SELECT lang FROM terms WHERE id = {x}) END)",
        lang = is(x, Tag::LangString),
        dir = is(x, Tag::DirLangString),
    )
}

/// The graph of a quad: NULL for the default graph, else its IRI or blank node.
fn graph() -> String {
    format!(
        "(CASE WHEN q.g = {DEFAULT_GRAPH_ID} THEN NULL ELSE {} END)",
        value("q.g")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pragmas_are_read_from_comments() {
        let src = "# @table parent <http://ex.org/parent>\n  #@class person <http://ex.org/P>\n# plain comment\nP(x:) :- parent(subject: x);";
        assert_eq!(
            pragmas(src).unwrap(),
            vec![
                Table::Predicate {
                    name: "parent".into(),
                    iri: "http://ex.org/parent".into()
                },
                Table::Class {
                    name: "person".into(),
                    iri: "http://ex.org/P".into()
                },
            ]
        );
        assert!(matches!(
            pragmas("# @table parent http://ex.org/parent"),
            Err(SynalogError::Pragma { line: 1, .. })
        ));
    }

    #[test]
    fn declared_names_are_checked() {
        let t = |name: &str| Table::Predicate {
            name: name.into(),
            iri: "http://ex.org/p".into(),
        };
        assert!(validate(&[t("parent")]).is_ok());
        assert!(validate(&[t("Parent")]).is_err());
        assert!(validate(&[t("triples")]).is_err());
        assert!(validate(&[t("a"), t("a")]).is_err());
    }

    #[test]
    fn only_referenced_tables_are_injected() {
        let options = Options::default();
        let declared = [Table::Class {
            name: "person".into(),
            iri: "http://ex.org/P".into(),
        }];
        let (sql, names) = inject("SELECT 1", &declared, &options);
        assert_eq!((sql.as_str(), names.len()), ("SELECT 1", 0));
        let (sql, names) = inject(
            "WITH t AS (SELECT * FROM person) SELECT * FROM t",
            &declared,
            &options,
        );
        assert_eq!(names, ["person"]);
        assert!(
            sql.starts_with("WITH person AS NOT MATERIALIZED ("),
            "{sql}"
        );
        assert!(
            sql.ends_with(",\nt AS (SELECT * FROM person) SELECT * FROM t"),
            "{sql}"
        );
    }
}
