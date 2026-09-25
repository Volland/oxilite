//! `oxilite versioning …`: the level of a store, its history, changes and diffs, purges, and
//! the migrations that change the level of a D1 database.
//!
// @lat: [[architecture#Versioning#Command line]]

use crate::db::Db;
use crate::Location;
use clap::Subcommand;
use oxilite::model::{GraphName, NamedNode, NamedOrBlankNode, Term};
use oxilite::version::{Change, History, LevelChange, VersionState, VersionStatus, Versioning};
use oxilite_core::version::{change_statements, format_time};
use std::str::FromStr;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Subcommand)]
pub enum VersioningCommand {
    /// Shows the versioning level, the clock and the recorded history.
    Status {
        #[command(flatten)]
        location: Location,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Changes the versioning level: `off`, `stamped` or `log`. Upgrades keep every quad (the
    /// upgrade to `log` records the whole store as its genesis commit); a downgrade freezes the
    /// history and stops the clock, and deletes them only with `--allow-loss`.
    Set {
        #[command(flatten)]
        location: Location,
        /// The new level.
        level: String,
        /// Create the as-of index (level `log`): fast as-of queries on any pattern.
        #[arg(long, conflicts_with = "no_as_of_index")]
        with_as_of_index: bool,
        /// Drop the as-of index.
        #[arg(long)]
        no_as_of_index: bool,
        /// Create the stamp index: fast "added since" queries.
        #[arg(long, conflicts_with = "no_stamp_index")]
        with_stamp_index: bool,
        /// Drop the stamp index.
        #[arg(long)]
        no_stamp_index: bool,
        /// Allow a downgrade to delete the history, the ticks and the stamp column.
        #[arg(long)]
        allow_loss: bool,
    },
    /// Lists the latest commits and level changes, newest first.
    Log {
        #[command(flatten)]
        location: Location,
        /// How many entries.
        #[arg(long, short = 'n', default_value_t = 20)]
        limit: usize,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Prints the changes after a version, as RDF Patch lines (`A` added, `D` deleted). With
    /// only the clock (`stamped`), the quads added since, removals leave no trace.
    Changes {
        #[command(flatten)]
        location: Location,
        /// Changes after this version: a tick (`42`, `#42`) or a version (`HEAD~3`, `@…`).
        #[arg(long, default_value = "0")]
        since: String,
        /// Up to this version (inclusive; default: now).
        #[arg(long)]
        until: Option<String>,
    },
    /// Prints the net difference between two versions as RDF Patch lines.
    Diff {
        #[command(flatten)]
        location: Location,
        /// The older version (`HEAD~1`, `#42`, `@2026-09-01T00:00:00Z`).
        from: String,
        /// The newer version.
        #[arg(default_value = "HEAD")]
        to: String,
    },
    /// Removes the quads matching a pattern from the store *and from its whole history*, for
    /// erasure requests; the purge is recorded without the removed content. Terms are IRIs
    /// (`<…>` or bare) or N-Triples literals.
    Purge {
        #[command(flatten)]
        location: Location,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        predicate: Option<String>,
        #[arg(long)]
        object: Option<String>,
        #[arg(long)]
        graph: Option<String>,
        /// Why (recorded on the purge commit).
        #[arg(long)]
        reason: String,
        /// Confirm: a purge rewrites history and cannot be undone.
        #[arg(long)]
        yes: bool,
    },
    /// Prints the SQL that changes the versioning level of a D1 database, for
    /// `wrangler d1 migrations` (production D1 schemas change through migrations).
    Migration {
        /// The database's current level.
        #[arg(long)]
        from: String,
        /// The new level.
        #[arg(long)]
        to: String,
        /// The database was stamped before (soft downgrade): `quads.t` already exists.
        #[arg(long)]
        stamp_column_exists: bool,
        /// The database holds a frozen history (an earlier downgrade from `log`).
        #[arg(long)]
        frozen_history: bool,
        /// Create the as-of index.
        #[arg(long)]
        as_of_index: bool,
        /// Create the stamp index.
        #[arg(long)]
        stamp_index: bool,
        /// Allow a downgrade to delete data.
        #[arg(long)]
        allow_loss: bool,
    },
}

pub fn run(command: VersioningCommand) -> Result<()> {
    match command {
        VersioningCommand::Status { location, json } => {
            let status = Db::open(&location)?.versioning()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                print!("{}", render_status(&status));
            }
        }
        VersioningCommand::Set {
            location,
            level,
            with_as_of_index,
            no_as_of_index,
            with_stamp_index,
            no_stamp_index,
            allow_loss,
        } => {
            let db = Db::open(&location)?;
            let flag = |on: bool, off: bool| (on || off).then_some(on);
            let status = db.set_versioning(
                level.parse()?,
                LevelChange {
                    as_of_index: flag(with_as_of_index, no_as_of_index),
                    stamp_index: flag(with_stamp_index, no_stamp_index),
                    allow_loss,
                    author: location.author.clone(),
                    message: location.message.clone(),
                },
            )?;
            print!("{}", render_status(&status));
        }
        VersioningCommand::Log {
            location,
            limit,
            json,
        } => {
            let log = Db::open(&location)?.history(limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&log)?);
                return Ok(());
            }
            for c in log {
                let counts = match (c.added, c.removed) {
                    (Some(a), Some(r)) if c.kind == "write" || a + r > 0 => {
                        format!("  +{a} -{r}")
                    }
                    _ => String::new(),
                };
                let who = c.author.map(|a| format!("  ({a})")).unwrap_or_default();
                let what = c.message.map(|m| format!("  {m}")).unwrap_or_default();
                let kind = if c.kind == "write" {
                    String::new()
                } else {
                    format!("  [{}]", c.kind)
                };
                println!(
                    "#{}  {}{kind}{counts}{what}{who}",
                    c.tick,
                    format_time(c.time)
                );
            }
        }
        VersioningCommand::Changes {
            location,
            since,
            until,
        } => {
            let db = Db::open(&location)?;
            let tick = |v: &str| -> Result<i64> {
                match v.strip_prefix('#').unwrap_or(v).parse::<i64>() {
                    Ok(t) => Ok(t),
                    Err(_) => db.resolve_version(v),
                }
            };
            let after = tick(&since)?;
            let until = until.as_deref().map(tick).transpose()?;
            print_patch(&db.changes(after, until)?);
        }
        VersioningCommand::Diff { location, from, to } => {
            print_patch(&Db::open(&location)?.diff(&from, &to)?);
        }
        VersioningCommand::Purge {
            location,
            subject,
            predicate,
            object,
            graph,
            reason,
            yes,
        } => {
            if !yes {
                return Err("a purge removes data from the whole history and cannot be undone: add --yes to confirm".into());
            }
            if subject.is_none() && predicate.is_none() && object.is_none() && graph.is_none() {
                return Err(
                    "give at least one of --subject, --predicate, --object, --graph".into(),
                );
            }
            let subject = subject
                .map(|s| -> Result<NamedOrBlankNode> { Ok(NamedNode::new(bare_iri(&s))?.into()) })
                .transpose()?;
            let predicate = predicate
                .map(|p| NamedNode::new(bare_iri(&p)))
                .transpose()?;
            let object = object.map(|o| parse_term(&o)).transpose()?;
            let graph = graph
                .map(|g| -> Result<GraphName> { Ok(NamedNode::new(bare_iri(&g))?.into()) })
                .transpose()?;
            Db::open(&location)?.purge((subject, predicate, object, graph), Some(&reason))?;
            eprintln!("purged; the purge is recorded in the history without the removed content");
        }
        VersioningCommand::Migration {
            from,
            to,
            stamp_column_exists,
            frozen_history,
            as_of_index,
            stamp_index,
            allow_loss,
        } => {
            let from_level: Versioning = from.parse()?;
            let state = VersionState {
                level: from_level,
                history: match (from_level, frozen_history) {
                    (Versioning::Log, _) => History::Live,
                    (_, true) => History::Frozen,
                    _ => History::None,
                },
                stamp_column: stamp_column_exists || from_level >= Versioning::Stamped,
                ..Default::default()
            };
            let change = LevelChange {
                as_of_index: as_of_index.then_some(true),
                stamp_index: stamp_index.then_some(true),
                allow_loss,
                ..Default::default()
            };
            println!("-- oxilite: versioning {from} -> {to} (generated by `oxilite versioning migration`)");
            for s in change_statements(&state, to.parse()?, &change)? {
                println!("{};", s.sql);
            }
        }
    }
    Ok(())
}

fn render_status(s: &VersionStatus) -> String {
    let mut out = format!("versioning: {}\n", s.state.level);
    out.push_str(&format!("history:    {}\n", s.state.history.as_str()));
    if let Some(h) = s.head {
        let when = s.head_time.map(format_time).unwrap_or_default();
        out.push_str(&format!("head:       #{h}  {when}\n"));
    }
    if let Some(g) = s.genesis {
        out.push_str(&format!("since:      #{g}\n"));
    }
    if let Some(f) = s.frozen_at {
        out.push_str(&format!("frozen at:  #{f}\n"));
    }
    if let Some(c) = s.commits {
        out.push_str(&format!("commits:    {c}\n"));
    }
    let mut idx = Vec::new();
    if s.state.as_of_index {
        idx.push("as-of");
    }
    if s.state.stamp_index {
        idx.push("stamp");
    }
    if !idx.is_empty() {
        out.push_str(&format!("indexes:    {}\n", idx.join(", ")));
    }
    out
}

/// RDF Patch lines: `A` (added) and `D` (deleted) quads, grouped by tick.
fn print_patch(changes: &[Change]) {
    let mut tick = None;
    for c in changes {
        if tick != Some(c.tick) {
            println!("# #{}", c.tick);
            tick = Some(c.tick);
        }
        println!("{} {} .", if c.added { "A" } else { "D" }, c.quad);
    }
}

fn bare_iri(s: &str) -> String {
    s.trim()
        .strip_prefix('<')
        .and_then(|x| x.strip_suffix('>'))
        .unwrap_or(s.trim())
        .to_owned()
}

fn parse_term(s: &str) -> Result<Term> {
    let s = s.trim();
    if s.starts_with('"') || s.starts_with("_:") || s.starts_with('<') {
        return Ok(Term::from_str(s)?);
    }
    Ok(NamedNode::new(s)?.into())
}
