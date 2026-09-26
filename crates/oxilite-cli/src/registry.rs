//! `oxilite registry …`: which named graphs of a store hold an ontology, SHACL shapes or a ShEx
//! schema and which graphs they apply to (RDF in `<oxilite:schema>`), and the SHACL shape index
//! compiled from them.
//!
// @lat: [[architecture#Command line and HTTP endpoint#Schema registry commands]]

use crate::db::{parse_graph, Db};
use crate::Location;
use clap::Subcommand;
use oxilite::io::RdfFormat;
use oxilite::model::{GraphName, NamedNode};
use oxilite::schema::{RegisteredGraph, Registration, SchemaRole, ShapeIndex};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Subcommand)]
pub enum RegistryCommand {
    /// Installs or refreshes the system graphs in an existing store: the oxilite vocabulary in
    /// `<oxilite:vocabulary>` and the registry's own description in `<oxilite:schema>`.
    Init {
        #[command(flatten)]
        location: Location,
    },
    /// Lists the registered schema graphs.
    List {
        #[command(flatten)]
        location: Location,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Registers GRAPH (an IRI, or DEFAULT) as an ontology, a SHACL shapes graph or a ShEx
    /// schema. Its triples stay where they are; the first ontology registered scopes reasoning
    /// to the registered ontologies, the first shapes graph scopes the shape index.
    Register {
        #[command(flatten)]
        location: Location,
        /// The graph: an IRI (optionally in angle brackets) or DEFAULT.
        graph: String,
        /// What the graph holds: ontology, shacl or shex.
        #[arg(long, value_parser = parse_role)]
        role: SchemaRole,
        /// Load this RDF file into the graph first, and record its SHA-256.
        #[arg(long)]
        file: Option<String>,
        /// The file's format (default: from its extension).
        #[arg(long, requires = "file")]
        format: Option<String>,
        /// The `owl:Ontology` IRI, when it differs from the graph name.
        #[arg(long)]
        iri: Option<String>,
        /// A version to pin (`owl:versionIRI`, a tag…).
        #[arg(long)]
        version: Option<String>,
        /// An `owl:imports` target to record (repeatable; recorded, not loaded).
        #[arg(long = "import")]
        imports: Vec<String>,
        /// A graph the schema applies to (repeatable; an IRI, DEFAULT or ALL). Without it, the
        /// schema applies to every graph.
        #[arg(long = "applies-to", value_name = "GRAPH")]
        applies_to: Vec<String>,
        /// Register the graph inactive: hidden like schema, but contributing nothing.
        #[arg(long)]
        inactive: bool,
    },
    /// Sets the graphs a registered schema applies to (IRIs, DEFAULT; ALL for every graph),
    /// keeping everything else it records.
    Map {
        #[command(flatten)]
        location: Location,
        graph: String,
        /// The target graphs.
        #[arg(long = "to", value_name = "GRAPH", required = true, num_args = 1..)]
        to: Vec<String>,
    },
    /// Makes a registered graph contribute again.
    Activate {
        #[command(flatten)]
        location: Location,
        graph: String,
    },
    /// Stops a registered graph contributing, keeping it registered.
    Deactivate {
        #[command(flatten)]
        location: Location,
        graph: String,
    },
    /// Removes a registration; the graph's triples stay.
    Unregister {
        #[command(flatten)]
        location: Location,
        graph: String,
    },
    /// Removes a registration and every triple of its graph, in one atomic request.
    Drop {
        #[command(flatten)]
        location: Location,
        graph: String,
    },
    /// Prints the SHACL property shapes compiled from the registered shapes graphs.
    Shapes {
        #[command(flatten)]
        location: Location,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
}

fn parse_role(s: &str) -> std::result::Result<SchemaRole, String> {
    s.parse().map_err(|e: oxilite::core::Error| e.to_string())
}

pub fn run(command: RegistryCommand) -> Result<()> {
    match command {
        RegistryCommand::Init { location } => {
            if Db::open(&location)?.install_system_graphs()? {
                eprintln!("installed the system graphs <oxilite:schema> and <oxilite:vocabulary>");
            } else {
                eprintln!("the system graphs are already current");
            }
        }
        RegistryCommand::List { location, json } => {
            let entries = Db::open(&location)?.schema_graphs()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries_json(&entries))?);
            } else if entries.is_empty() {
                eprintln!("no schema graph registered: every graph contributes axioms and shapes");
            } else {
                for e in &entries {
                    println!("{}", describe(e));
                }
            }
        }
        RegistryCommand::Register {
            location,
            graph,
            role,
            file,
            format,
            iri,
            version,
            imports,
            applies_to,
            inactive,
        } => {
            let db = Db::open(&location)?;
            let graph = parse_graph(&graph)?;
            let mut registration = Registration::new();
            if let Some(f) = &file {
                let data = std::fs::read(f)?;
                let format = match &format {
                    Some(m) => crate::parse_format(m)?,
                    None => RdfFormat::from_extension(f.rsplit('.').next().unwrap_or_default())
                        .ok_or_else(|| format!("unknown format of {f}"))?,
                };
                let target = match &graph {
                    GraphName::NamedNode(n) => Some(n.as_str()),
                    _ => None,
                };
                db.load(format, &data, target, true)?;
                registration.sha256 = Some(sha256_hex(&data));
            }
            registration.iri = iri.map(NamedNode::new).transpose()?;
            registration.version = version;
            registration.imports = imports
                .into_iter()
                .map(NamedNode::new)
                .collect::<std::result::Result<_, _>>()?;
            registration.applies_to = targets(&applies_to)?;
            registration.active = !inactive;
            db.register_schema_graph(&graph, role, &registration)?;
            eprintln!("registered {} as {}", graph_text(&graph), role.name());
        }
        RegistryCommand::Map {
            location,
            graph,
            to,
        } => {
            let db = Db::open(&location)?;
            let g = parse_graph(&graph)?;
            remap(&db, &g, targets(&to)?)?;
        }
        RegistryCommand::Activate { location, graph } => {
            set_active(&location, &graph, true)?;
        }
        RegistryCommand::Deactivate { location, graph } => {
            set_active(&location, &graph, false)?;
        }
        RegistryCommand::Unregister { location, graph } => {
            let g = parse_graph(&graph)?;
            if !Db::open(&location)?.unregister_schema_graph(&g)? {
                return Err(format!("{} is not registered", graph_text(&g)).into());
            }
        }
        RegistryCommand::Drop { location, graph } => {
            let g = parse_graph(&graph)?;
            let n = Db::open(&location)?.drop_schema_graph(&g)?;
            eprintln!("dropped {} ({n} quads)", graph_text(&g));
        }
        RegistryCommand::Shapes { location, json } => {
            let index = Db::open(&location)?.shape_index()?;
            if json {
                let v = oxilite::core::json::shape_index_to_json(&index);
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                print!("{}", render_shapes(&index));
            }
        }
    }
    Ok(())
}

/// Target graphs: IRIs or DEFAULT; ALL (alone or among others) means every graph.
pub fn targets(args: &[String]) -> Result<Vec<GraphName>> {
    if args.iter().any(|a| a.eq_ignore_ascii_case("all")) {
        return Ok(Vec::new());
    }
    args.iter().map(|a| parse_graph(a)).collect()
}

/// Re-registers `graph` with new targets, keeping its role and what it records.
pub fn remap(db: &Db, graph: &GraphName, applies_to: Vec<GraphName>) -> Result<()> {
    let Some(e) = db.schema_graphs()?.into_iter().find(|e| &e.graph == graph) else {
        return Err(format!("{} is not registered", graph_text(graph)).into());
    };
    let mut r = e.registration;
    r.applies_to = applies_to;
    db.register_schema_graph(graph, e.role, &r)
}

fn set_active(location: &Location, graph: &str, active: bool) -> Result<()> {
    let g = parse_graph(graph)?;
    if !Db::open(location)?.set_schema_graph_active(&g, active)? {
        return Err(format!("{} is not registered", graph_text(&g)).into());
    }
    Ok(())
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn graph_text(g: &GraphName) -> String {
    match g {
        GraphName::DefaultGraph => "DEFAULT".into(),
        g => g.to_string(),
    }
}

pub fn entries_json(entries: &[RegisteredGraph]) -> serde_json::Value {
    serde_json::Value::Array(
        entries
            .iter()
            .map(|e| oxilite::core::json::schema_graph_to_json(&e.to_entry()))
            .collect(),
    )
}

/// One line per registration: graph, role, state and what was recorded.
fn describe(e: &RegisteredGraph) -> String {
    let r = &e.registration;
    let mut line = format!(
        "{}\t{}\t{}",
        graph_text(&e.graph),
        e.role.name(),
        if r.active { "active" } else { "inactive" }
    );
    if r.applies_to.is_empty() {
        line.push_str("\tapplies to all graphs");
    } else {
        let t: Vec<String> = r.applies_to.iter().map(graph_text).collect();
        line.push_str(&format!("\tapplies to {}", t.join(" ")));
    }
    if let Some(v) = &r.version {
        line.push_str(&format!("\tversion {v}"));
    }
    if let Some(i) = &r.iri {
        line.push_str(&format!("\tiri {i}"));
    }
    if let Some(h) = &r.sha256 {
        line.push_str(&format!("\tsha256 {}", &h[..h.len().min(12)]));
    }
    for i in &r.imports {
        line.push_str(&format!("\timports {i}"));
    }
    line
}

/// One line per target class and path, with its constraints.
pub fn render_shapes(index: &ShapeIndex) -> String {
    let mut out = String::new();
    for (target, paths) in &index.by_class {
        for (path, s) in paths {
            out.push_str(&format!("{target}\t{path}"));
            for c in shape_constraints(s) {
                out.push('\t');
                out.push_str(&c);
            }
            out.push('\n');
        }
    }
    out
}

/// The constraints of a compiled property shape, as `name value` strings.
pub fn shape_constraints(s: &oxilite::schema::PropertyShape) -> Vec<String> {
    let mut c = Vec::new();
    if let Some(d) = &s.datatype {
        c.push(format!("datatype {d}"));
    }
    if let Some(m) = s.min {
        c.push(format!("minCount {m}"));
    }
    if let Some(m) = s.max {
        c.push(format!("maxCount {m}"));
    }
    if let Some(p) = &s.pattern {
        c.push(format!("pattern {p:?}"));
    }
    if !s.values_in.is_empty() {
        let v: Vec<String> = s.values_in.iter().map(ToString::to_string).collect();
        c.push(format!("in ({})", v.join(" ")));
    }
    if s.relationship {
        c.push("relationship".into());
    }
    c
}
