//! `oxilite [FILE]`: an interactive SPARQL shell in the style of `sqlite3`.
//!
//! Without a file the store is a transient in-memory store; with one it is that SQLite file,
//! created with the schema when missing. Statements span lines and run when complete, results
//! print as tables fitted to the terminal, and Tab completes commands, keywords, prefixes and
//! the store's own terms. When standard input is not a terminal it runs as a script.
//!
// @lat: [[architecture#Command line and HTTP endpoint#Interactive shell]]

mod editor;
mod input;
mod prefixes;
mod render;
#[cfg(test)]
mod tests;

use crate::db::Db;
use crate::studio::conn::Vocab;
use crate::Location;
use oxilite::io::{RdfFormat, RdfSerializer};
use oxilite::model::{GraphName, NamedNode, NamedOrBlankNode, Quad, Term};
use oxilite::sparql::SparqlParser;
use oxilite_core::QueryOutput;
use prefixes::Prefixes;
use render::{Mode, Paint, BOLD, DIM, RED, YELLOW};
use std::cell::RefCell;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The dot-commands: name, arguments and what they do.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    (
        ".datalog",
        "PROGRAM|FILE",
        "Run a Datalog program and print its goal",
    ),
    (
        ".dump",
        "?FILE?",
        "Write every quad as N-Quads (to standard output without FILE)",
    ),
    (".exit", "?CODE?", "Exit the shell with CODE (default 0)"),
    (
        ".explain",
        "?QUERY?",
        "Show the SQL a query compiles to (default: the last query)",
    ),
    (".graphs", "", "List the named graphs and their sizes"),
    (".help", "", "Show this message"),
    (
        ".load",
        "FILE ?GRAPH?",
        "Load an RDF file, into GRAPH if given",
    ),
    (".maxrows", "?N?", "Show or set how many rows a table shows"),
    (".mode", "?MODE?", "Show or set the output mode"),
    (
        ".open",
        "?FILE?",
        "Close this store and open FILE (a new in-memory store without one)",
    ),
    (
        ".optimize",
        "",
        "Refresh planner statistics and the reasoning closure",
    ),
    (
        ".prefix",
        "?P: <IRI>?",
        "List session prefixes, add one, or remove P without an IRI",
    ),
    (".quit", "", "Exit the shell"),
    (".read", "FILE", "Run the statements and commands in FILE"),
    (".save", "FILE", "Copy this store into a new SQLite file"),
    (".stats", "", "Summarize the store"),
    (".timer", "on|off", "Show how long each statement takes"),
];

pub enum Flow {
    Continue,
    Exit(i32),
}

pub struct Session {
    db: Db,
    location: Location,
    pub prefixes: Prefixes,
    /// Lines of the statement being typed.
    pending: String,
    pending_lines: usize,
    pending_start: usize,
    line_no: usize,
    /// The store's vocabulary for completion, computed on demand after each change.
    vocab: Option<Vocab>,
    mode: Mode,
    timer: bool,
    max_rows: usize,
    paint: Paint,
    err_paint: Paint,
    /// Fit tables to the terminal (off when output is not a terminal).
    fit: bool,
    /// Report line numbers with errors (scripts and `.read`).
    numbered: bool,
    last_query: Option<String>,
    /// The statement or command that just ran, for the history.
    pub last_entry: Option<String>,
    pub failed: bool,
    out: Box<dyn Write>,
    err: Box<dyn Write>,
}

impl Session {
    pub fn new(db: Db, location: Location, out: Box<dyn Write>, err: Box<dyn Write>) -> Self {
        Self {
            db,
            location,
            prefixes: Prefixes::default(),
            pending: String::new(),
            pending_lines: 0,
            pending_start: 0,
            line_no: 0,
            vocab: None,
            mode: Mode::Table,
            timer: true,
            max_rows: 200,
            paint: Paint { color: false },
            err_paint: Paint { color: false },
            fit: false,
            numbered: false,
            last_query: None,
            last_entry: None,
            failed: false,
            out,
            err,
        }
    }

    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Drops the statement being typed (Ctrl-C).
    pub fn cancel(&mut self) {
        self.pending.clear();
        self.pending_lines = 0;
    }

    /// The vocabulary of the store, computed if the store changed since.
    pub fn vocab(&mut self) -> &Vocab {
        if self.vocab.is_none() {
            let db = &self.db;
            let v = Vocab::from_rows(|q| match db.query(q, &[], &[])? {
                QueryOutput::Solutions { rows, .. } => Ok(rows),
                _ => Ok(Vec::new()),
            })
            .unwrap_or_default();
            self.vocab = Some(v);
        }
        self.vocab.as_ref().expect("computed")
    }

    fn width(&self) -> Option<usize> {
        self.fit
            .then(|| terminal_size::terminal_size().map(|(w, _)| w.0 as usize))
            .flatten()
    }

    fn print(&mut self, text: &str) {
        let _ = self.out.write_all(text.as_bytes());
        let _ = self.out.flush();
    }

    fn note(&mut self, text: &str) {
        let t = self.paint.paint(text, DIM);
        self.print(&format!("{t}\n"));
    }

    fn footer(&mut self, text: &str, took: Duration) {
        let mut t = text.to_string();
        if self.timer {
            if !t.is_empty() {
                t.push_str(" · ");
            }
            t.push_str(&render::elapsed(took));
        }
        if !t.is_empty() {
            self.note(&t);
        }
    }

    fn error(&mut self, message: &str) {
        self.failed = true;
        let label = if self.numbered {
            format!("Error (line {}):", self.pending_start.max(1))
        } else {
            "Error:".into()
        };
        let label = self.err_paint.paint(&label, &format!("{BOLD};{RED}"));
        let _ = writeln!(self.err, "{label} {message}");
        let _ = self.err.flush();
    }

    /// Takes one line of input; runs a command, or a statement once it is complete.
    pub fn feed_line(&mut self, line: &str) -> Flow {
        self.line_no += 1;
        let trimmed = line.trim();
        if self.pending.is_empty() {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return Flow::Continue;
            }
            if trimmed.starts_with('.') {
                self.pending_start = self.line_no;
                self.last_entry = Some(trimmed.to_string());
                return self.command(trimmed);
            }
            self.pending_start = self.line_no;
        } else {
            self.pending.push('\n');
        }
        self.pending.push_str(line);
        self.pending_lines += 1;
        let ready = input::complete(
            &self.pending,
            self.pending_lines == 1,
            trimmed.is_empty(),
            |t| self.parses(t),
        );
        if ready {
            self.flush();
        }
        Flow::Continue
    }

    /// Runs whatever is pending (end of input).
    pub fn flush(&mut self) {
        if self.pending.trim().is_empty() {
            self.cancel();
            return;
        }
        let statement = std::mem::take(&mut self.pending);
        self.pending_lines = 0;
        self.last_entry = Some(statement.trim_end().to_string());
        self.statement(&statement);
    }

    fn parses(&self, text: &str) -> bool {
        let full = self.prefixes.declare_missing(input::strip_terminator(text));
        SparqlParser::new().parse_query(&full).is_ok()
            || SparqlParser::new().parse_update(&full).is_ok()
    }

    /// Runs one SPARQL query or update.
    fn statement(&mut self, text: &str) {
        let body = input::strip_terminator(text);
        if body.is_empty() {
            return;
        }
        let full = self.prefixes.declare_missing(body);
        let start = Instant::now();
        match SparqlParser::new().parse_query(&full) {
            Ok(_) => match self.db.query(&full, &[], &[]) {
                Ok(out) => {
                    let took = start.elapsed();
                    self.prefixes.learn(body);
                    self.last_query = Some(full);
                    self.show(&out, took);
                }
                Err(e) => self.error(&e.to_string()),
            },
            Err(query_error) => match SparqlParser::new().parse_update(&full) {
                Ok(u) if u.operations.is_empty() => {
                    // Only declarations: they join the session.
                    self.prefixes.learn(body);
                    for (p, (ns, _)) in crate::studio::scanner::scan(body, None).prefixes {
                        self.note(&format!("{p}: <{ns}>"));
                    }
                }
                Ok(_) => match self.db.update(&full, &[]) {
                    Ok(()) => {
                        let took = start.elapsed();
                        self.prefixes.learn(body);
                        self.vocab = None;
                        self.footer("OK", took);
                    }
                    Err(e) => self.error(&e.to_string()),
                },
                Err(update_error) => {
                    let e = if input::is_update(body) {
                        update_error.to_string()
                    } else {
                        query_error.to_string()
                    };
                    self.error(&e);
                }
            },
        }
    }

    fn show(&mut self, out: &QueryOutput, took: Duration) {
        match render::output(
            out,
            self.mode,
            &self.prefixes,
            self.max_rows,
            self.width(),
            self.paint,
        ) {
            Ok((text, footer)) => {
                self.print(&text);
                if self.mode == Mode::Table || !footer.is_empty() && self.fit {
                    self.footer(&footer, took);
                }
            }
            Err(e) => self.error(&e),
        }
    }

    /// Runs a dot-command.
    fn command(&mut self, line: &str) -> Flow {
        let (name, rest) = line
            .split_once(char::is_whitespace)
            .map_or((line, ""), |(n, r)| (n, r.trim()));
        let args: Vec<&str> = rest.split_whitespace().collect();
        let result: Result<()> = match name {
            ".exit" | ".quit" => {
                // A script that failed exits with 1 unless it names a status.
                let failed = i32::from(self.numbered && self.failed);
                return Flow::Exit(args.first().and_then(|c| c.parse().ok()).unwrap_or(failed));
            }
            ".help" => {
                self.help();
                Ok(())
            }
            ".open" => self.open(args.first().copied()),
            ".save" => match args.first() {
                Some(f) => self.save(f),
                None => Err("usage: .save FILE".into()),
            },
            ".load" => match args.first() {
                Some(f) => self.load(f, args.get(1).copied()),
                None => Err("usage: .load FILE ?GRAPH?".into()),
            },
            ".read" => match args.first() {
                Some(f) => return self.read(f),
                None => Err("usage: .read FILE".into()),
            },
            ".dump" => self.dump(args.first().copied()),
            ".mode" => self.set_mode(args.first().copied()),
            ".prefix" => self.prefix(&args),
            ".explain" => self.explain(rest),
            ".datalog" => self.datalog(rest),
            ".graphs" => self.graphs(),
            ".stats" => self.stats(),
            ".optimize" => {
                let start = Instant::now();
                self.db.optimize().map(|()| {
                    self.vocab = None;
                    self.footer("Optimized", start.elapsed());
                })
            }
            ".timer" => match args.first() {
                Some(&"on") => {
                    self.timer = true;
                    Ok(())
                }
                Some(&"off") => {
                    self.timer = false;
                    Ok(())
                }
                _ => Err("usage: .timer on|off".into()),
            },
            ".maxrows" => match args.first() {
                None => {
                    self.note(&format!("{}", self.max_rows));
                    Ok(())
                }
                Some(n) => n
                    .parse()
                    .map(|n: usize| self.max_rows = n.max(1))
                    .map_err(|_| "usage: .maxrows N".into()),
            },
            _ => Err(format!("unknown command {name}; enter \".help\" for the list").into()),
        };
        if let Err(e) = result {
            self.error(&e.to_string());
        }
        Flow::Continue
    }

    fn help(&mut self) {
        let p = self.paint;
        let mut text = String::new();
        for (name, args, what) in COMMANDS {
            let head = format!("{name} {args}");
            let pad = " ".repeat(26usize.saturating_sub(head.chars().count()));
            text.push_str(&format!(
                "{} {}{pad}{}\n",
                p.paint(name, &format!("{BOLD};{YELLOW}")),
                p.paint(args, DIM),
                what
            ));
        }
        text.push_str(&format!(
            "\n{}\n",
            p.paint(
                &[
                    "A statement is a SPARQL query or update. It runs when it is complete on one line,",
                    "or when it ends with ';' or an empty line. Ctrl-C drops it, Ctrl-D exits.",
                    "Tab completes commands, keywords, prefixes and the store's predicates and classes.",
                    &format!("Output modes: {}.", Mode::NAMES.join(", ")),
                ]
                .join("\n"),
                DIM
            )
        ));
        self.print(&text);
    }

    /// Opens another store with the same flags; `:memory:` without a file.
    fn open(&mut self, file: Option<&str>) -> Result<()> {
        let mut location = self.location.clone();
        location.location = file.map(Into::into);
        if file.is_some() {
            location.d1_sidecar = None;
        }
        let created = file.is_some_and(|f| !Path::new(f).exists());
        self.db = Db::open(&location)?;
        self.location = location;
        self.vocab = None;
        let what = describe(&self.location, created);
        self.note(&format!("Opened {what}"));
        Ok(())
    }

    /// Every quad of the store: the default graph, then the named graphs.
    fn quads(&self) -> Result<Vec<Quad>> {
        let out = self.db.query(
            "SELECT ?s ?p ?o ?g WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } }",
            &[],
            &[],
        )?;
        let QueryOutput::Solutions { rows, .. } = out else {
            return Ok(Vec::new());
        };
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let subject: NamedOrBlankNode = match r[0].clone()? {
                    Term::NamedNode(n) => n.into(),
                    Term::BlankNode(b) => b.into(),
                    _ => return None,
                };
                let Term::NamedNode(predicate) = r[1].clone()? else {
                    return None;
                };
                let graph = match r[3].clone() {
                    None => GraphName::DefaultGraph,
                    Some(Term::NamedNode(n)) => n.into(),
                    Some(Term::BlankNode(b)) => b.into(),
                    Some(_) => return None,
                };
                Some(Quad::new(subject, predicate, r[2].clone()?, graph))
            })
            .collect())
    }

    fn nquads(&self) -> Result<(Vec<u8>, usize)> {
        let quads = self.quads()?;
        let mut w = RdfSerializer::from_format(RdfFormat::NQuads).for_writer(Vec::new());
        for q in &quads {
            w.serialize_quad(q)?;
        }
        Ok((w.finish()?, quads.len()))
    }

    fn dump(&mut self, file: Option<&str>) -> Result<()> {
        let (bytes, n) = self.nquads()?;
        match file {
            Some(f) => {
                std::fs::write(f, bytes)?;
                self.note(&format!(
                    "Wrote {} to {f}",
                    render::count(n, "quad", "quads")
                ));
            }
            None => self.print(&String::from_utf8_lossy(&bytes)),
        }
        Ok(())
    }

    fn save(&mut self, file: &str) -> Result<()> {
        if Path::new(file).exists() {
            return Err(format!("{file} already exists").into());
        }
        let start = Instant::now();
        let (bytes, n) = self.nquads()?;
        let target = Location {
            location: Some(file.into()),
            d1_sidecar: None,
            ..self.location.clone()
        };
        Db::open(&target)?.load(RdfFormat::NQuads, &bytes, None, true)?;
        let text = format!("Saved {} to {file}", render::count(n, "quad", "quads"));
        self.footer(&text, start.elapsed());
        Ok(())
    }

    fn load(&mut self, file: &str, graph: Option<&str>) -> Result<()> {
        let format = RdfFormat::from_extension(file.rsplit('.').next().unwrap_or_default())
            .ok_or_else(|| format!("unknown RDF format of {file}"))?;
        let graph = graph.map(|g| self.expand(g)).transpose()?;
        let data = std::fs::read(file)?;
        let start = Instant::now();
        self.db.load(format, &data, graph.as_deref(), true)?;
        self.vocab = None;
        if matches!(format, RdfFormat::Turtle | RdfFormat::TriG | RdfFormat::N3) {
            self.prefixes.learn(&String::from_utf8_lossy(&data));
        }
        self.footer(&format!("Loaded {file}"), start.elapsed());
        Ok(())
    }

    /// An IRI given as `<iri>`, `prefix:local` or plain text.
    fn expand(&self, term: &str) -> Result<String> {
        if let Some(iri) = term.strip_prefix('<').and_then(|t| t.strip_suffix('>')) {
            return Ok(iri.into());
        }
        if let Some((p, local)) = term.split_once(':') {
            if let Some(ns) = self.prefixes.get(p) {
                return Ok(format!("{ns}{local}"));
            }
        }
        NamedNode::new(term)?;
        Ok(term.into())
    }

    fn read(&mut self, file: &str) -> Flow {
        let text = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) => {
                self.error(&format!("{file}: {e}"));
                return Flow::Continue;
            }
        };
        let saved = (self.line_no, self.numbered);
        self.line_no = 0;
        self.numbered = true;
        let mut flow = Flow::Continue;
        for line in text.lines() {
            if let Flow::Exit(c) = self.feed_line(line) {
                flow = Flow::Exit(c);
                break;
            }
        }
        if matches!(flow, Flow::Continue) {
            self.flush();
        }
        (self.line_no, self.numbered) = saved;
        self.last_entry = Some(format!(".read {file}"));
        flow
    }

    fn set_mode(&mut self, mode: Option<&str>) -> Result<()> {
        match mode {
            None => {
                let text = format!("{} (one of {})", self.mode.name(), Mode::NAMES.join(", "));
                self.note(&text);
                Ok(())
            }
            Some(m) => {
                self.mode = Mode::parse(m).ok_or_else(|| {
                    format!("unknown mode {m}; one of {}", Mode::NAMES.join(", "))
                })?;
                Ok(())
            }
        }
    }

    fn prefix(&mut self, args: &[&str]) -> Result<()> {
        match args {
            [] => {
                let rows: Vec<Vec<render::Cell>> = self
                    .prefixes
                    .iter()
                    .map(|(p, ns)| {
                        vec![
                            render::Cell {
                                text: format!("{p}:"),
                                code: render::YELLOW,
                                right: false,
                            },
                            render::Cell {
                                text: format!("<{ns}>"),
                                code: render::BLUE,
                                right: false,
                            },
                        ]
                    })
                    .collect();
                let t = render::table(
                    &["prefix".into(), "namespace".into()],
                    &rows,
                    self.width(),
                    self.paint,
                );
                self.print(&t);
                Ok(())
            }
            [p] => {
                let p = p.trim_end_matches(':');
                if self.prefixes.remove(p) {
                    Ok(())
                } else {
                    Err(format!("no prefix {p}:").into())
                }
            }
            [p, ns, ..] => {
                let p = p.trim_end_matches(':');
                let ns = ns.trim_start_matches('<').trim_end_matches('>');
                NamedNode::new(ns)?;
                self.prefixes.insert(p, ns);
                Ok(())
            }
        }
    }

    fn explain(&mut self, query: &str) -> Result<()> {
        let full = if query.is_empty() {
            self.last_query
                .clone()
                .ok_or("no query yet: use .explain QUERY")?
        } else {
            self.prefixes
                .declare_missing(input::strip_terminator(query))
        };
        let text = self.db.explain(&full)?;
        self.print(&format!("{}\n", text.trim_end()));
        Ok(())
    }

    fn datalog(&mut self, program: &str) -> Result<()> {
        if program.is_empty() {
            return Err("usage: .datalog PROGRAM|FILE".into());
        }
        let source = if Path::new(program).is_file() {
            std::fs::read_to_string(program)?
        } else {
            program.to_string()
        };
        let source = self.prefixes.declare_missing_datalog(&source);
        let start = Instant::now();
        let r = self.db.datalog(&source)?;
        let took = start.elapsed();
        let (body, footer) = render::solutions(
            &r.variables,
            &r.rows,
            &self.prefixes,
            self.max_rows,
            self.width(),
            self.paint,
        );
        self.print(&body);
        self.footer(&footer, took);
        Ok(())
    }

    fn rows(&self, query: &str) -> Result<Vec<Vec<Option<Term>>>> {
        match self.db.query(query, &[], &[])? {
            QueryOutput::Solutions { rows, .. } => Ok(rows),
            _ => Ok(Vec::new()),
        }
    }

    fn graphs(&mut self) -> Result<()> {
        let start = Instant::now();
        let rows = self.rows(
            "SELECT ?graph (COUNT(*) AS ?triples) WHERE { GRAPH ?graph { ?s ?p ?o } } GROUP BY ?graph ORDER BY DESC(?triples)",
        )?;
        let (body, footer) = render::solutions(
            &["graph".into(), "triples".into()],
            &rows,
            &self.prefixes,
            self.max_rows,
            self.width(),
            self.paint,
        );
        self.print(&body);
        self.footer(&footer.replace("row", "graph"), start.elapsed());
        Ok(())
    }

    fn stats(&mut self) -> Result<()> {
        let number = |rows: Vec<Vec<Option<Term>>>, i: usize| -> usize {
            match rows.first().and_then(|r| r.get(i).cloned().flatten()) {
                Some(Term::Literal(l)) => l.value().parse().unwrap_or(0),
                _ => 0,
            }
        };
        let default = number(self.rows("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")?, 0);
        let named = self.rows(
            "SELECT (COUNT(DISTINCT ?g) AS ?g) (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }",
        )?;
        let (graphs, in_graphs) = (number(named.clone(), 0), number(named, 1));
        let (predicates, classes) = {
            let v = self.vocab();
            (v.predicates.len(), v.classes.len())
        };
        let store = describe(&self.location, false);
        let facts = [
            ("store", store),
            ("default graph", render::count(default, "triple", "triples")),
            (
                "named graphs",
                format!(
                    "{} holding {}",
                    render::count(graphs, "graph", "graphs"),
                    render::count(in_graphs, "triple", "triples")
                ),
            ),
            (
                "predicates",
                render::count(predicates, "", "").trim().to_string(),
            ),
            ("classes", render::count(classes, "", "").trim().to_string()),
            ("session prefixes", self.prefixes.iter().count().to_string()),
        ];
        let rows: Vec<Vec<render::Cell>> = facts
            .into_iter()
            .map(|(k, v)| {
                vec![
                    render::Cell {
                        text: k.into(),
                        code: BOLD,
                        right: false,
                    },
                    render::Cell::plain(v),
                ]
            })
            .collect();
        let t = render::table(&["".into(), "".into()], &rows, self.width(), self.paint);
        // A key-value table needs no header row.
        let t: Vec<&str> = t.lines().collect();
        let t = [&t[..1], &t[3..]].concat().join("\n");
        self.print(&format!("{t}\n"));
        Ok(())
    }
}

/// How the banner and `.open` name a store.
fn describe(location: &Location, created: bool) -> String {
    if let Some(url) = &location.d1_sidecar {
        return format!("the D1 database behind {url}");
    }
    match location.location.as_deref() {
        None | Some(":memory:") => "a transient in-memory database".into(),
        Some(f) if created => format!("{f} (new database)"),
        Some(f) => f.to_string(),
    }
}

fn color_allowed() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

/// Runs the shell on the store `location` names; returns the exit status.
pub fn run(location: Location) -> Result<i32> {
    let created = location
        .location
        .as_deref()
        .is_some_and(|f| f != ":memory:" && !Path::new(f).exists());
    let db = Db::open(&location)?;
    let interactive = std::io::stdin().is_terminal();
    let mut session = Session::new(
        db,
        location,
        Box::new(std::io::stdout()),
        Box::new(std::io::stderr()),
    );
    session.paint.color = std::io::stdout().is_terminal() && color_allowed();
    session.err_paint.color = std::io::stderr().is_terminal() && color_allowed();
    session.fit = std::io::stdout().is_terminal();
    if !interactive {
        session.numbered = true;
        for line in std::io::stdin().lines() {
            if let Flow::Exit(c) = session.feed_line(&line?) {
                return Ok(c);
            }
        }
        session.flush();
        return Ok(i32::from(session.failed));
    }
    let p = session.paint;
    let what = describe(&session.location, created);
    let persist = if session.location.location.is_none() && session.location.d1_sidecar.is_none() {
        "\nUse \".open FILE\" to reopen on a persistent database, or \".save FILE\" to keep this one."
    } else {
        ""
    };
    session.print(&format!(
        "{} {}\n{}\n",
        p.paint("oxilite", &format!("{BOLD};{}", render::GREEN)),
        p.paint(
            &format!("{} · SPARQL 1.2 on SQLite", env!("CARGO_PKG_VERSION")),
            DIM
        ),
        p.paint(
            &format!("Connected to {what}.{persist}\nEnter \".help\" for usage hints."),
            DIM
        ),
    ));
    let session = Rc::new(RefCell::new(session));
    editor::interact(&session)
}
