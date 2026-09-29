//! Synalog over oxilite: the Datalog-family language for AI agents, as an optional second rule
//! dialect beside `oxilite-datalog`.
//!
//! [Synalog](https://github.com/SynaLinks/synalog) (a Rust rewrite of Logica) has named
//! arguments, full expressions, head aggregation, functors, `@OrderBy`/`@Limit` and a verifier
//! whose errors are written for a model to read. It compiles to SQL. This crate uses its parser,
//! verifier and compiler unchanged, two ways:
//!
//! - on its own: [`check`] and [`compile_for_engine`], for any engine Synalog supports;
//! - over the triple store: [`compile`] / [`prepare`] present the store as relational tables —
//!   `triples(subject, predicate, object, kind, datatype, lang, graph)` plus the predicate and
//!   class tables a program declares — and make the SQL run on every backend, D1 included.
//!
//! ```text
//! # @table parent <http://example.org/parent>
//!
//! @Recursive(Ancestor, 10);
//! Ancestor(x:, y:) distinct :- parent(subject: x, object: y);
//! Ancestor(x:, y:) distinct :- Ancestor(x:, y: m), parent(subject: m, object: y);
//!
//! Adult(name:) distinct :- triples(subject: p, predicate: "http://example.org/age", object: age),
//!                          triples(subject: p, predicate: "http://example.org/name", object: name),
//!                          age >= 18;
//! ```
//!
// @lat: [[architecture#Synalog frontend]]
#![forbid(unsafe_code)]

pub mod error;
pub mod exec;
#[cfg(feature = "json")]
pub mod json;
pub mod rewrite;
pub mod tables;

pub use error::{Result, SynalogError};
pub use exec::{SynalogJob, SynalogResult};
pub use oxilite_core::sql::SqlValue;
/// The language crate, for callers that need its full API.
pub use synalog;
pub use tables::Table;

use oxilite_core::sql::Capabilities;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use synalog::compiler::universe::{LogicaProgram, Pagination};
use synalog::parser::Json;

/// The engines [`compile_for_engine`] accepts.
pub const ENGINES: &[&str] = synalog::compiler::dialects::SUPPORTED_ENGINES;

/// Options for running a program on the store. The scope defaults match SPARQL's: the default
/// graph only, no inferences, the current state.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Read every graph rather than only the default graph.
    pub union_default_graph: bool,
    /// Also read materialized inferences (`quads_inf`).
    pub include_inferred: bool,
    /// At most this many rows (combined with the program's `@Limit` by taking the smaller).
    pub limit: Option<u64>,
    /// Skip this many rows.
    pub offset: Option<u64>,
    /// Tables declared in code, beside the program's `# @table` / `# @class` pragmas.
    pub tables: Vec<Table>,
    /// Read the store as it was at this version (`HEAD~1`, `#42`, `@2026-09-01T00:00:00Z`).
    /// The store resolves it to [`Self::as_of_tick`] before compiling.
    pub as_of: Option<String>,
    /// The resolved tick of the version read.
    pub as_of_tick: Option<i64>,
}

/// A program compiled for the store.
#[derive(Debug, Clone)]
pub struct Compiled {
    /// The one statement to run.
    pub sql: String,
    /// The predicate's head columns, in order.
    pub columns: Vec<String>,
    /// The store tables the statement reads.
    pub tables: Vec<String>,
}

/// Runs Synalog code, turning a panic into an error: an agent's program must not abort the host.
fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|panic| {
        let why = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown cause".into());
        Err(SynalogError::Compile(format!(
            "the Synalog compiler failed: {why}"
        )))
    })
}

/// Parses and verifies a program.
fn parse_checked(src: &str) -> Result<Json> {
    guarded(|| {
        let parsed = synalog::parser::parse_file(src, None, &[])
            .map_err(|e| SynalogError::Parse(e.to_string()))?;
        let checked = synalog::verifier::validate(&parsed);
        if !checked.is_valid() {
            return Err(SynalogError::Verify(
                checked.errors.iter().map(ToString::to_string).collect(),
            ));
        }
        Ok(parsed)
    })
}

/// Runs the Synalog verifier: `Ok` when the program is valid, else every error it reports.
pub fn check(src: &str) -> Result<()> {
    parse_checked(src).map(|_| ())
}

/// Compiles one predicate for one engine, exactly as Synalog does: the dialect without a store.
pub fn compile_for_engine(
    src: &str,
    predicate: &str,
    engine: &str,
    limit: Option<u64>,
    offset: Option<u64>,
) -> Result<String> {
    if !ENGINES.contains(&engine) {
        return Err(SynalogError::unsupported(format!(
            "engine `{engine}`; Synalog supports {}",
            ENGINES.join(", ")
        )));
    }
    let parsed = parse_checked(src)?;
    guarded(|| {
        let program = program(&parsed, engine)?;
        defined(&program, predicate)?;
        program
            .formatted_predicate_sql_with_pagination(predicate, &Pagination { limit, offset })
            .map_err(|e| SynalogError::Compile(e.to_string()))
    })
}

fn program(parsed: &Json, engine: &str) -> Result<LogicaProgram> {
    LogicaProgram::new_with_engine(parsed, HashMap::new(), HashMap::new(), Some(engine))
        .map_err(|e| SynalogError::Compile(e.to_string()))
}

fn defined(program: &LogicaProgram, predicate: &str) -> Result<()> {
    if program.defined_predicates().contains(predicate) {
        return Ok(());
    }
    let mut defined = program.user_defined_predicates();
    defined.sort();
    Err(SynalogError::UnknownPredicate {
        predicate: predicate.to_owned(),
        defined,
    })
}

/// Compiles one predicate to a statement over the store's tables.
pub fn compile(
    src: &str,
    predicate: &str,
    caps: &Capabilities,
    options: &Options,
) -> Result<Compiled> {
    if options.as_of.is_some() && options.as_of_tick.is_none() {
        return Err(SynalogError::unsupported(
            "as_of must be resolved to a tick by the store before compiling",
        ));
    }
    if options.as_of_tick.is_some() && options.include_inferred {
        return Err(SynalogError::unsupported(
            "inferences describe the current state only; they cannot be combined with as_of",
        ));
    }
    let mut declared = tables::pragmas(src)?;
    declared.extend(options.tables.iter().cloned());
    tables::validate(&declared)?;

    let parsed = parse_checked(src)?;
    let (raw, columns) = guarded(|| {
        let program = program(&parsed, "sqlite")?;
        defined(&program, predicate)?;
        let pagination = Pagination {
            limit: options.limit,
            offset: options.offset,
        };
        let sql = program
            .formatted_predicate_sql_with_pagination(predicate, &pagination)
            .map_err(|e| SynalogError::Compile(e.to_string()))?;
        Ok((sql, program.predicate_columns(predicate)))
    })?;
    let body = rewrite::portable(&raw)?;
    let (sql, tables) = tables::inject(&body, &declared, options);
    rewrite::check_limits(&sql, caps)?;
    Ok(Compiled {
        sql,
        columns,
        tables,
    })
}

/// Compiles a predicate into a job that runs it.
pub fn prepare(
    src: &str,
    predicate: &str,
    caps: &Capabilities,
    options: &Options,
) -> Result<SynalogJob> {
    Ok(SynalogJob::new(compile(src, predicate, caps, options)?))
}
