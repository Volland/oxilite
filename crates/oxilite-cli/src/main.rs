//! `oxilite`: an interactive SPARQL shell (the default command), load and query a store from the
//! command line, or serve it over the SPARQL 1.1 protocol with the same routes as
//! `oxigraph serve` (`/query`, `/update`, `/store`).
//!
//! The store is a SQLite file (bundled SQLite), the same file through a SQLite shared library
//! (`--library`), or a D1 database behind the local sidecar (`--d1-sidecar`, used by the
//! benchmarks).
//!
// @lat: [[architecture#Command line and HTTP endpoint]]

mod db;
mod shell;
mod studio;
mod versioning;

use versioning::VersioningCommand;

use clap::{Parser, Subcommand};
use db::Db;
use oxilite::io::RdfFormat;
use std::sync::Arc;
use std::time::Instant;
use tiny_http::{Header, Method, Request, Response, Server};

#[derive(Parser)]
#[command(
    name = "oxilite",
    version,
    about = "oxilite: an Oxigraph-compatible SPARQL store on SQLite",
    long_about = "oxilite: an Oxigraph-compatible SPARQL store on SQLite.\n\n\
        Without a command, opens an interactive SPARQL shell on DATABASE (a SQLite file, created \
        with the schema if missing) or on a transient in-memory store.",
    args_conflicts_with_subcommands = true
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// The shell's SQLite database file (created if missing; in memory when omitted).
    database: Option<String>,
    #[command(flatten)]
    location: Location,
}

#[derive(clap::Args, Clone, Default)]
struct Location {
    /// SQLite database file (created if missing).
    #[arg(long, short)]
    location: Option<String>,
    /// Load this SQLite shared library instead of the bundled SQLite.
    #[arg(long)]
    library: Option<String>,
    /// Use the D1 database of a local sidecar (`testsuite/d1-sidecar`) at this URL.
    #[arg(long)]
    d1_sidecar: Option<String>,
    /// Create the store without the graph index (fewer writes if you only use the default graph).
    #[arg(long)]
    no_graph_index: bool,
    /// Create the full-text index over string literals (for `oxl:textMatch`).
    #[arg(long)]
    text_index: bool,
    /// Versioning of a new store: `off` (default), `stamped` (a store clock and the tick that
    /// added each quad) or `log` (an immutable change log: history and time travel). An existing
    /// store keeps its level; change it with `oxilite versioning set`.
    #[arg(long, value_name = "LEVEL")]
    versioning: Option<String>,
    /// With `--versioning log`: also index the change log by predicate and object (faster
    /// as-of queries, two more rows written per change).
    #[arg(long)]
    as_of_index: bool,
    /// With `--versioning stamped` or `log`: index the tick that added each quad (faster
    /// "added since" queries, one more row written per quad).
    #[arg(long)]
    stamp_index: bool,
    /// Author recorded on the commits of this command's writes (versioned stores).
    #[arg(long)]
    author: Option<String>,
    /// Message recorded on the commits of this command's writes (versioned stores).
    #[arg(long, short = 'm')]
    message: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Serves the store over the SPARQL 1.1 protocol.
    Serve {
        #[command(flatten)]
        location: Location,
        #[arg(long, short, default_value = "127.0.0.1:7879")]
        bind: String,
        /// Worker threads.
        #[arg(long, default_value_t = 8)]
        threads: usize,
    },
    /// Bulk-loads RDF files, then refreshes planner statistics.
    Load {
        #[command(flatten)]
        location: Location,
        /// Files to load; the format comes from the extension unless `--format` is given.
        #[arg(long, short, required = true, num_args = 1..)]
        file: Vec<String>,
        #[arg(long)]
        format: Option<String>,
    },
    /// Runs a SPARQL query and prints the results.
    Query {
        #[command(flatten)]
        location: Location,
        #[arg(long, short)]
        query: String,
        /// Read the store as it was at this version: `HEAD~2`, `#42` (a tick) or
        /// `@2026-09-01T12:00:00Z` (versioning `log`).
        #[arg(long, value_name = "VERSION")]
        as_of: Option<String>,
        /// Results format (json, xml, csv, tsv, or an RDF format for graph results).
        #[arg(long, default_value = "json")]
        results_format: String,
    },
    /// Prints the SQL a query compiles to, with the planner's notes.
    Explain {
        #[command(flatten)]
        location: Location,
        #[arg(long, short)]
        query: String,
        /// Compile the query against this version of the store (see `query --as-of`).
        #[arg(long, value_name = "VERSION")]
        as_of: Option<String>,
    },
    /// Runs a SPARQL update.
    Update {
        #[command(flatten)]
        location: Location,
        #[arg(long, short)]
        update: String,
    },
    /// Runs a Datalog program: recursive rules with stratified negation.
    ///
    /// The program is read from `--program`, or from the file named by `--file`, or from
    /// standard input when neither is given.
    Datalog {
        #[command(flatten)]
        location: Location,
        /// The program text.
        #[arg(long, short, conflicts_with = "file")]
        program: Option<String>,
        /// A file holding the program.
        #[arg(long, short)]
        file: Option<String>,
        /// Print the strata, the strategy per recursive component and the SQL, and run nothing.
        #[arg(long)]
        explain: bool,
        /// Store what the program derives as inferences instead of returning its goal.
        #[arg(long, conflicts_with = "explain")]
        materialize: bool,
        /// Run the program on this version of the store (like an `@version` directive).
        #[arg(long, value_name = "VERSION", conflicts_with = "materialize")]
        as_of: Option<String>,
    },
    /// Versioning: the level, the history, changes, diffs, purges and D1 migrations.
    Versioning {
        #[command(subcommand)]
        action: VersioningCommand,
    },
    /// Prints the schema as a SQL script, e.g. for `wrangler d1 migrations` (the store options
    /// choose the indexes and the versioning level).
    Schema {
        /// Without the graph index.
        #[arg(long)]
        no_graph_index: bool,
        /// With the full-text index.
        #[arg(long)]
        text_index: bool,
        /// Versioning level: off (default), stamped or log.
        #[arg(long, value_name = "LEVEL", default_value = "off")]
        versioning: String,
        /// With `log`: the as-of index.
        #[arg(long)]
        as_of_index: bool,
        /// With `stamped` or `log`: the stamp index.
        #[arg(long)]
        stamp_index: bool,
    },
    /// Refreshes planner statistics and the reasoning closure.
    Optimize {
        #[command(flatten)]
        location: Location,
    },
    /// Checks a project like oxilite studio does: load errors, rule errors, SHACL results and the
    /// tests in `oxilite.toml`. Exits with status 1 when anything fails.
    Check {
        /// The project root (default: the current directory).
        #[arg(default_value = ".")]
        root: String,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Serves the studio's tools (query, schema, validate, why) to agents over the Model Context
    /// Protocol on standard input and output.
    Mcp {
        /// The project root to load like oxilite studio's Project store (default: here).
        #[arg(long)]
        root: Option<String>,
        /// Open this store file read-only instead of loading a project.
        #[arg(long, conflicts_with = "root")]
        location: Option<String>,
    },
    /// Runs the language server behind oxilite studio (LSP over standard input and output).
    StudioServer {
        /// Scratch store location (default `<workspace>/.oxilite/studio.sqlite`; `:memory:` works).
        #[arg(long)]
        store: Option<String>,
    },
}

fn main() {
    let args = Args::parse();
    let result = match args.command {
        Some(command) => run(command),
        None => {
            let mut location = args.location;
            if let Some(database) = args.database {
                location.location = Some(database);
            }
            match shell::run(location) {
                Ok(0) => Ok(()),
                Ok(code) => std::process::exit(code),
                Err(e) => Err(e),
            }
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(command: Command) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match command {
        Command::Serve {
            location,
            bind,
            threads,
        } => {
            let db = Arc::new(Db::open(&location)?);
            let server = Arc::new(Server::http(&bind).map_err(|e| e.to_string())?);
            eprintln!("oxilite listening on http://{bind}/ (query endpoint /query, update endpoint /update)");
            let workers: Vec<_> = (0..threads.max(1))
                .map(|_| {
                    let (server, db) = (Arc::clone(&server), Arc::clone(&db));
                    std::thread::spawn(move || {
                        for request in server.incoming_requests() {
                            handle(&db, request);
                        }
                    })
                })
                .collect();
            for w in workers {
                let _ = w.join();
            }
            Ok(())
        }
        Command::Load {
            location,
            file,
            format,
        } => {
            let db = Db::open(&location)?;
            for f in file {
                let format = match &format {
                    Some(m) => parse_format(m)?,
                    None => RdfFormat::from_extension(f.rsplit('.').next().unwrap_or_default())
                        .ok_or_else(|| format!("unknown format of {f}"))?,
                };
                let start = Instant::now();
                let data = std::fs::read(&f)?;
                db.load(format, &data, None, true)?;
                eprintln!("loaded {f} in {:.2}s", start.elapsed().as_secs_f64());
            }
            Ok(())
        }
        Command::Query {
            location,
            query,
            results_format,
            as_of,
        } => {
            let db = Db::open(&location)?;
            let out = db.query_at(&query, &[], &[], as_of.as_deref())?;
            print!(
                "{}",
                oxilite_core::json::output_to_format(&out, &results_format)?
            );
            Ok(())
        }
        Command::Explain {
            location,
            query,
            as_of,
        } => {
            println!(
                "{}",
                Db::open(&location)?.explain_at(&query, as_of.as_deref())?
            );
            Ok(())
        }
        Command::Update { location, update } => Ok(Db::open(&location)?.update(&update, &[])?),
        Command::Datalog {
            location,
            program,
            file,
            explain,
            materialize,
            as_of,
        } => {
            let source = read_program(program, file)?;
            let db = Db::open(&location)?;
            if explain {
                println!("{}", db.explain_datalog(&source)?);
                return Ok(());
            }
            if materialize {
                let stats = db.datalog_materialize(&source)?;
                println!(
                    "{} inferred triple(s) from {} relation(s)",
                    stats.inferred, stats.relations
                );
                return Ok(());
            }
            let r = db.datalog_at(&source, as_of.as_deref())?;
            println!("{}", r.variables.join("\t"));
            for row in &r.rows {
                let line: Vec<String> = row
                    .iter()
                    .map(|c| c.as_ref().map(ToString::to_string).unwrap_or_default())
                    .collect();
                println!("{}", line.join("\t"));
            }
            Ok(())
        }
        Command::Optimize { location } => Ok(Db::open(&location)?.optimize()?),
        Command::Versioning { action } => versioning::run(action),
        Command::Schema {
            no_graph_index,
            text_index,
            versioning,
            as_of_index,
            stamp_index,
        } => {
            let options = oxilite::StoreOptions {
                graph_index: !no_graph_index,
                text_index,
                versioning: versioning.parse()?,
                as_of_index,
                stamp_index,
            };
            print!("{}", oxilite_core::schema::schema_sql(&options));
            Ok(())
        }
        Command::StudioServer { store } => studio::run(store),
        Command::Mcp { root, location } => studio::mcp::serve(root.map(Into::into), location),
        Command::Check { root, json } => {
            let (passed, report) = studio::check::check(root.into())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", studio::check::render(&report));
            }
            if !passed {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

/// A program given inline, in a file, or on standard input.
fn read_program(
    program: Option<String>,
    file: Option<String>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(p) = program {
        return Ok(p);
    }
    if let Some(f) = file {
        return Ok(std::fs::read_to_string(f)?);
    }
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
    if buf.trim().is_empty() {
        return Err("no program given: use --program, --file, or pipe one in".into());
    }
    Ok(buf)
}

fn parse_format(media: &str) -> Result<RdfFormat, String> {
    let m = media.split(';').next().unwrap_or_default().trim();
    RdfFormat::from_media_type(m)
        .or_else(|| RdfFormat::from_extension(m))
        .ok_or_else(|| format!("unsupported RDF format {media}"))
}

fn header(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.to_string().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn respond(request: Request, status: u16, content_type: &str, body: String) {
    let r = Response::from_string(body)
        .with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", content_type).expect("valid header"));
    let _ = request.respond(r);
}

/// Picks the response media type from the `Accept` header.
fn negotiate(accept: Option<&str>, graph: bool) -> &'static str {
    let candidates: &[&str] = if graph {
        &[
            "application/n-triples",
            "text/turtle",
            "application/rdf+xml",
            "application/n-quads",
            "application/trig",
        ]
    } else {
        &[
            "application/sparql-results+json",
            "application/sparql-results+xml",
            "text/csv",
            "text/tab-separated-values",
        ]
    };
    if let Some(accept) = accept {
        for part in accept.split(',') {
            let m = part.split(';').next().unwrap_or_default().trim();
            if let Some(c) = candidates.iter().find(|c| **c == m) {
                return c;
            }
            if m == "application/json" && !graph {
                return "application/sparql-results+json";
            }
            if m == "application/xml" && !graph {
                return "application/sparql-results+xml";
            }
        }
    }
    candidates[if graph { 1 } else { 0 }]
}

fn handle(db: &Db, mut request: Request) {
    let url = request.url().to_string();
    let (path, query_string) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let mut params: Vec<(String, String)> = form_urlencoded::parse(query_string.as_bytes())
        .into_owned()
        .collect();
    let content_type = header(&request, "Content-Type").unwrap_or_default();
    let mut body = Vec::new();
    if request.method() == &Method::Post {
        if let Err(e) = request.as_reader().read_to_end(&mut body) {
            return respond(request, 400, "text/plain", e.to_string());
        }
    }
    let form = content_type.starts_with("application/x-www-form-urlencoded");
    if form {
        params.extend(form_urlencoded::parse(&body).into_owned());
    }
    let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let all = |k: &str| {
        params
            .iter()
            .filter(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
            .collect::<Vec<_>>()
    };
    let result: Result<(u16, String, String), String> = (|| match (request.method(), path) {
        (Method::Get | Method::Post, "/query") => {
            let query = match get("query") {
                Some(q) => q,
                None if content_type.starts_with("application/sparql-query") => {
                    String::from_utf8(body.clone()).map_err(|e| e.to_string())?
                }
                None => return Err("missing query".into()),
            };
            let out = db
                .query_at(
                    &query,
                    &all("default-graph-uri"),
                    &all("named-graph-uri"),
                    get("version").as_deref(),
                )
                .map_err(|e| e.to_string())?;
            let graph = matches!(out, oxilite_core::QueryOutput::Graph(_));
            let media = negotiate(header(&request, "Accept").as_deref(), graph);
            let text =
                oxilite_core::json::output_to_format(&out, media).map_err(|e| e.to_string())?;
            Ok((200, media.to_string(), text))
        }
        (Method::Post, "/update") => {
            let update = match get("update") {
                Some(u) => u,
                None if content_type.starts_with("application/sparql-update") => {
                    String::from_utf8(body.clone()).map_err(|e| e.to_string())?
                }
                None => return Err("missing update".into()),
            };
            db.update(&update, &all("using-graph-uri"))
                .map_err(|e| e.to_string())?;
            Ok((204, "text/plain".into(), String::new()))
        }
        (Method::Post, "/store") => {
            let format = parse_format(&content_type)?;
            let graph = get("graph");
            let bulk = params.iter().any(|(n, _)| n == "no_transaction");
            db.load(format, &body, graph.as_deref(), bulk)
                .map_err(|e| e.to_string())?;
            Ok((204, "text/plain".into(), String::new()))
        }
        (Method::Get, "/") => Ok((
            200,
            "text/plain".into(),
            "oxilite SPARQL endpoint: /query, /update, /store\n".into(),
        )),
        _ => Ok((404, "text/plain".into(), "not found\n".into())),
    })();
    match result {
        Ok((status, media, text)) => respond(request, status, &media, text),
        Err(e) => respond(request, 400, "text/plain", e),
    }
}
