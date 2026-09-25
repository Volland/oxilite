//! Datalog over oxilite: recursive rules with stratified negation, constraints and
//! aggregation, compiled to SQL.
//!
//! SPARQL and openCypher both stop at the same wall — recursion beyond a property path — and
//! the only rules oxilite has are the fixed RDFS/OWL-RL set hard-coded in the core. This crate
//! makes a rule a first-class thing: named, composable, and evaluated where the data is, which
//! on Cloudflare D1 means inside the database rather than over the network.
//!
//! ```text
//! @prefix ex: <http://example.org/> .
//!
//! ancestor(?x, ?y) :- ex:parent(?x, ?y).
//! ancestor(?x, ?z) :- ex:parent(?x, ?y), ancestor(?y, ?z).
//!
//! adult(?x, ?y) :- ancestor(?x, ?y), ex:age(?y, ?a), ?a >= 18.
//! orphan(?x)     :- ex:Person(?x), not ancestor(?x, _).
//!
//! ?- adult(?x, ?y).
//! ```
//!
//! A program is parsed, checked (safety and stratification), and compiled to one SQL
//! statement whenever its recursion is linear — the same one-round-trip invariant SPARQL and
//! Cypher hold.
//!
// @lat: [[architecture#Datalog frontend]]
#![forbid(unsafe_code)]

pub mod ast;
pub mod error;
pub mod exec;
#[cfg(feature = "json")]
pub mod json;
pub mod lexer;
pub mod materialize;
pub mod parser;
pub mod program;
pub mod sql;

pub use ast::{Program, Rule};
pub use error::{DatalogError, Result};
pub use exec::{DatalogJob, DatalogResult};
pub use materialize::{plan as materialize_plan, MaterializeJob, MaterializeStats};
pub use parser::parse;
pub use program::{Analysis, Shape, Stratum};
pub use sql::{Compiled, Fixpoint, FixpointPhase, Options};

use ast::{At, Atom, BodyItem};
use oxilite_core::sql::Capabilities;

/// Parses, checks and compiles a program.
pub fn compile(src: &str, caps: &Capabilities, options: &Options) -> Result<Compiled> {
    let program = parse(src)?;
    let analysis = program::analyse(&program)?;
    sql::compile(&program, &analysis, caps, options)
}

/// The version a program reads: the `as_of` option, else its `@version` directive.
pub fn version_of(src: &str, options: &Options) -> Result<Option<String>> {
    if options.as_of.is_some() {
        return Ok(options.as_of.clone());
    }
    Ok(parse(src)?.version)
}

/// Every version a program names: its `as_of` / `@version` first (if any), then the `at "REF"`
/// of its atoms, deduplicated.
pub fn version_refs(src: &str, options: &Options) -> Result<(Option<String>, Vec<String>)> {
    let program = parse(src)?;
    let whole = options.as_of.clone().or(program.version.clone());
    let mut refs: Vec<String> = Vec::new();
    let mut push = |a: &Atom| {
        if let Some(At::Version(r)) = &a.at {
            if !refs.contains(r) {
                refs.push(r.clone());
            }
        }
    };
    for rule in &program.rules {
        for item in &rule.body {
            if let BodyItem::Atom(a) | BodyItem::Negated(a) = item {
                push(a);
            }
        }
    }
    if let Some(g) = &program.goal {
        push(&g.atom);
    }
    Ok((whole, refs))
}

/// Prepares a program for execution.
pub fn prepare(src: &str, caps: &Capabilities, options: &Options) -> Result<DatalogJob> {
    let compiled = compile(src, caps, options)?;
    Ok(DatalogJob::new(compiled, caps.clone()))
}

/// Describes how a program runs: its strata, the strategy chosen for each recursive
/// component, and the SQL it compiles to.
pub fn explain(src: &str, caps: &Capabilities, options: &Options) -> Result<String> {
    let program = parse(src)?;
    let analysis = program::analyse(&program)?;
    let mut out = String::from("-- oxilite datalog\n");
    for (i, stratum) in analysis.strata.iter().enumerate() {
        out.push_str(&format!(
            "-- stratum {i}: {} [{}]\n",
            stratum
                .preds
                .iter()
                .map(ast::Pred::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            match stratum.shape {
                Shape::NonRecursive => "not recursive",
                Shape::Linear => "linear recursion, one WITH RECURSIVE member",
                Shape::LinearMutual => "mutual recursion, one tagged WITH RECURSIVE member",
                Shape::NonLinear => "non-linear recursion",
            }
        ));
    }
    match sql::compile(&program, &analysis, caps, options) {
        Ok(compiled) => {
            for note in &compiled.notes {
                out.push_str(&format!("-- {note}\n"));
            }
            out.push_str(&compiled.sql);
            out.push('\n');
        }
        Err(e) => out.push_str(&format!("-- cannot compile: {e}\n")),
    }
    Ok(out)
}
