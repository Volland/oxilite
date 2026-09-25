//! How results look: terms compacted with the session prefixes, box tables fitted to the
//! terminal, and the other output modes.

use super::prefixes::Prefixes;
use oxilite::model::{Literal, Term};
use oxilite_core::QueryOutput;
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const BOLD: &str = "1";
pub const DIM: &str = "2";
pub const RED: &str = "31";
pub const GREEN: &str = "32";
pub const YELLOW: &str = "33";
pub const BLUE: &str = "34";
pub const MAGENTA: &str = "35";
pub const CYAN: &str = "36";

/// ANSI styling, or none when output is not a terminal or `NO_COLOR` is set.
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    pub color: bool,
}

impl Paint {
    pub fn paint(&self, text: &str, code: &str) -> String {
        if self.color && !code.is_empty() && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

/// How results print; the SPARQL results formats and the RDF formats for graph results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Table,
    Json,
    Xml,
    Csv,
    Tsv,
    Turtle,
    NTriples,
    NQuads,
    TriG,
    RdfXml,
}

impl Mode {
    pub const NAMES: &[&str] = &[
        "table", "json", "xml", "csv", "tsv", "turtle", "ntriples", "nquads", "trig", "rdfxml",
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "table" | "box" => Self::Table,
            "json" => Self::Json,
            "xml" => Self::Xml,
            "csv" => Self::Csv,
            "tsv" => Self::Tsv,
            "turtle" | "ttl" => Self::Turtle,
            "ntriples" | "nt" => Self::NTriples,
            "nquads" | "nq" => Self::NQuads,
            "trig" => Self::TriG,
            "rdfxml" | "rdf" | "xml-rdf" => Self::RdfXml,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// The extension of the SPARQL results format of this mode, if it is one.
    fn results_extension(self) -> Option<&'static str> {
        Some(match self {
            Self::Json => "json",
            Self::Xml => "xml",
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            _ => return None,
        })
    }

    /// The extension of the RDF format of this mode, if it is one.
    fn rdf_extension(self) -> Option<&'static str> {
        Some(match self {
            Self::NTriples => "nt",
            Self::NQuads => "nq",
            Self::TriG => "trig",
            Self::RdfXml => "rdf",
            _ => return None,
        })
    }
}

/// A table cell: its text, colour and alignment.
pub struct Cell {
    pub text: String,
    pub code: &'static str,
    pub right: bool,
}

impl Cell {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            code: "",
            right: false,
        }
    }
}

const NUMERIC: &[&str] = &[
    "integer",
    "decimal",
    "double",
    "float",
    "int",
    "long",
    "short",
    "byte",
    "nonNegativeInteger",
    "positiveInteger",
    "nonPositiveInteger",
    "negativeInteger",
    "unsignedInt",
    "unsignedLong",
    "unsignedShort",
    "unsignedByte",
];
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// Control characters as visible symbols, so a cell stays on one line.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\n' => '↵',
            '\t' => '→',
            '\r' => '␍',
            c if c.is_control() => '�',
            c => c,
        })
        .collect()
}

/// A term as the shell shows it.
pub fn term_text(t: &Term, prefixes: &Prefixes) -> String {
    match t {
        Term::NamedNode(n) => prefixes
            .compact(n.as_str())
            .unwrap_or_else(|| format!("<{}>", n.as_str())),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        Term::Literal(l) => literal_text(l, prefixes),
        #[allow(unreachable_patterns)]
        Term::Triple(tr) => format!(
            "<<( {} {} {} )>>",
            term_text(&tr.subject.clone().into(), prefixes),
            term_text(&tr.predicate.clone().into(), prefixes),
            term_text(&tr.object, prefixes)
        ),
    }
}

fn literal_text(l: &Literal, prefixes: &Prefixes) -> String {
    let dt = l.datatype().as_str();
    if let Some(lang) = l.language() {
        return format!("{}@{lang}", l.value());
    }
    match dt.strip_prefix(XSD) {
        Some("string") => l.value().to_string(),
        Some(local) if local == "boolean" || NUMERIC.contains(&local) => l.value().to_string(),
        _ => format!(
            "\"{}\"^^{}",
            l.value(),
            prefixes.compact(dt).unwrap_or_else(|| format!("<{dt}>"))
        ),
    }
}

pub fn term_cell(t: &Term, prefixes: &Prefixes) -> Cell {
    let text = one_line(&term_text(t, prefixes));
    let (code, right) = match t {
        Term::NamedNode(_) => (BLUE, false),
        Term::BlankNode(_) => (MAGENTA, false),
        Term::Literal(l) => match l.datatype().as_str().strip_prefix(XSD) {
            Some(local) if NUMERIC.contains(&local) => (CYAN, true),
            Some("boolean") => (CYAN, false),
            Some("string") | None if l.language().is_none() => (GREEN, false),
            _ => (GREEN, false),
        },
        #[allow(unreachable_patterns)]
        _ => (YELLOW, false),
    };
    Cell { text, code, right }
}

/// `s` cut to `width` columns, ending in `…` when cut.
fn clip(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        w += cw;
        out.push(c);
    }
    out.push('…');
    out
}

/// Column widths that fit `total` columns: the widest columns give way first.
pub fn fit_widths(natural: &[usize], total: Option<usize>) -> Vec<usize> {
    let mut w: Vec<usize> = natural.iter().map(|n| (*n).max(1)).collect();
    let Some(total) = total else { return w };
    let frame = 3 * w.len() + 1;
    let available = total.saturating_sub(frame).max(w.len() * 3);
    for x in &mut w {
        *x = (*x).min(available);
    }
    let mut sum: usize = w.iter().sum();
    while sum > available {
        let (i, max) = w
            .iter()
            .copied()
            .enumerate()
            .max_by_key(|(_, x)| *x)
            .expect("non-empty");
        if max <= 3 {
            break;
        }
        // Shrink the widest column down to the next widest, or just enough.
        let next = w
            .iter()
            .copied()
            .filter(|x| *x < max)
            .max()
            .unwrap_or(3)
            .max(3);
        let cut = (max - next).max(1).min(sum - available);
        w[i] -= cut;
        sum -= cut;
    }
    w
}

/// A box table: bold header, dim borders, cells clipped to fit `width`.
pub fn table(headers: &[String], rows: &[Vec<Cell>], width: Option<usize>, p: Paint) -> String {
    let natural: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            rows.iter()
                .map(|r| r.get(i).map_or(0, |c| c.text.width()))
                .max()
                .unwrap_or(0)
                .max(h.width())
        })
        .collect();
    let w = fit_widths(&natural, width);
    let rule = |l: &str, m: &str, r: &str| {
        let line = w
            .iter()
            .map(|x| "─".repeat(x + 2))
            .collect::<Vec<_>>()
            .join(m);
        format!("{}\n", p.paint(&format!("{l}{line}{r}"), DIM))
    };
    let bar = p.paint("│", DIM);
    let line = |cells: Vec<(String, &str, bool)>| {
        let mut s = bar.clone();
        for ((text, code, right), x) in cells.into_iter().zip(&w) {
            let t = clip(&text, *x);
            let pad = " ".repeat(x - t.width());
            let body = p.paint(&t, code);
            s.push(' ');
            if right {
                s.push_str(&pad);
                s.push_str(&body);
            } else {
                s.push_str(&body);
                s.push_str(&pad);
            }
            s.push(' ');
            s.push_str(&bar);
        }
        s.push('\n');
        s
    };
    let mut out = rule("┌", "┬", "┐");
    out.push_str(&line(
        headers.iter().map(|h| (h.clone(), BOLD, false)).collect(),
    ));
    out.push_str(&rule("├", "┼", "┤"));
    for r in rows {
        out.push_str(&line(
            (0..headers.len())
                .map(|i| match r.get(i) {
                    Some(c) => (c.text.clone(), c.code, c.right),
                    None => (String::new(), "", false),
                })
                .collect(),
        ));
    }
    out.push_str(&rule("└", "┴", "┘"));
    out
}

pub fn elapsed(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 0.001 {
        format!("{:.0} µs", s * 1e6)
    } else if s < 1.0 {
        format!("{:.2} ms", s * 1e3)
    } else {
        format!("{s:.2} s")
    }
}

pub fn count(n: usize, one: &str, many: &str) -> String {
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped} {}", if n == 1 { one } else { many })
}

/// Solutions as a table of at most `max_rows` rows, and a footer line counting them.
pub fn solutions(
    variables: &[String],
    rows: &[Vec<Option<Term>>],
    prefixes: &Prefixes,
    max_rows: usize,
    width: Option<usize>,
    p: Paint,
) -> (String, String) {
    let shown: Vec<Vec<Cell>> = rows
        .iter()
        .take(max_rows)
        .map(|r| {
            r.iter()
                .map(|t| match t {
                    Some(t) => term_cell(t, prefixes),
                    None => Cell::plain(""),
                })
                .collect()
        })
        .collect();
    let body = if variables.is_empty() {
        String::new()
    } else {
        table(variables, &shown, width, p)
    };
    let mut footer = count(rows.len(), "row", "rows");
    if rows.len() > shown.len() {
        footer.push_str(&format!(" ({} shown, .maxrows to see more)", shown.len()));
    }
    (body, footer)
}

/// Graph results as Turtle with the session prefixes it uses.
pub fn turtle(triples: &[oxilite::model::Triple], prefixes: &Prefixes) -> Result<String, String> {
    let mut serializer = oxrdfio::RdfSerializer::from_format(oxrdfio::RdfFormat::Turtle);
    let used = |ns: &str| {
        triples.iter().any(|t| {
            let (s, p, o): (Term, Term, &Term) = (
                t.subject.clone().into(),
                t.predicate.clone().into(),
                &t.object,
            );
            [&s, &p, o].iter().any(|x| match x {
                Term::NamedNode(n) => n.as_str().starts_with(ns),
                Term::Literal(l) => l.datatype().as_str().starts_with(ns),
                _ => false,
            })
        })
    };
    for (p, ns) in prefixes.iter() {
        if used(ns) {
            serializer = serializer.with_prefix(p, ns).map_err(|e| e.to_string())?;
        }
    }
    let mut w = serializer.for_writer(Vec::new());
    for t in triples {
        w.serialize_triple(t).map_err(|e| e.to_string())?;
    }
    let bytes = w.finish().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The text of a result in `mode`, and a footer (empty for serialized formats).
pub fn output(
    out: &QueryOutput,
    mode: Mode,
    prefixes: &Prefixes,
    max_rows: usize,
    width: Option<usize>,
    p: Paint,
) -> Result<(String, String), String> {
    let fmt = |f: &str| oxilite_core::json::output_to_format(out, f).map_err(|e| e.to_string());
    match out {
        QueryOutput::Graph(triples) => {
            let footer = count(triples.len(), "triple", "triples");
            match mode.rdf_extension() {
                Some(ext) => Ok((fmt(ext)?, footer)),
                None => Ok((turtle(triples, prefixes)?, footer)),
            }
        }
        _ if mode.results_extension().is_some() => {
            let mut text = fmt(mode.results_extension().expect("results mode"))?;
            if !text.ends_with('\n') {
                text.push('\n');
            }
            Ok((text, String::new()))
        }
        QueryOutput::Boolean(b) => Ok((
            format!(
                "{}\n",
                p.paint(&b.to_string(), if *b { GREEN } else { RED })
            ),
            String::new(),
        )),
        QueryOutput::Solutions { variables, rows } => {
            let names: Vec<String> = variables.iter().map(|v| v.as_str().to_string()).collect();
            Ok(solutions(&names, rows, prefixes, max_rows, width, p))
        }
    }
}
