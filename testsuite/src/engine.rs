//! The oxilite side of the harness: store variants that every test runs on.
//!
// @lat: [[test-plan#Oxigraph compatibility harness]]

use anyhow::{Context, Result};
use oxigraph::model::Dataset;
use oxilite::sparql::QueryResults;
use oxilite::QueryOptions;
use spargebra::algebra::GraphPattern;
use spargebra::{Query, Update};
use std::fmt;

/// A configuration oxilite is tested in. Plans and backends must never change results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// rusqlite, no statistics (heuristic planner).
    Rusqlite,
    /// rusqlite after `optimize()` (statistics-driven planner).
    RusqliteOptimized,
    /// rusqlite, SQLite chooses the join order.
    RusqliteSqlitePlanner,
    /// A system `libsqlite3` loaded at runtime (no UDFs).
    Dylib,
}

impl fmt::Display for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rusqlite => "rusqlite",
            Self::RusqliteOptimized => "rusqlite-optimized",
            Self::RusqliteSqlitePlanner => "rusqlite-sqlite-planner",
            Self::Dylib => "dylib",
        })
    }
}

/// Variants to run, from `OXILITE_VARIANTS` (comma separated) or all available ones.
pub fn variants() -> Vec<Variant> {
    let all = [
        Variant::Rusqlite,
        Variant::RusqliteOptimized,
        Variant::RusqliteSqlitePlanner,
        Variant::Dylib,
    ];
    let wanted = std::env::var("OXILITE_VARIANTS").ok();
    all.into_iter()
        .filter(|v| {
            wanted
                .as_ref()
                .is_none_or(|w| w.split(',').any(|x| x.trim() == v.to_string()))
        })
        .filter(|v| *v != Variant::Dylib || oxilite::dylib::find_system_library().is_some())
        .collect()
}

enum Backend {
    Rusqlite(oxilite::store::Store),
    Dylib(oxilite::store::Store<oxilite::dylib::DylibBackend>),
}

macro_rules! with_store {
    ($self:expr, $s:ident => $e:expr) => {
        match &$self.backend {
            Backend::Rusqlite($s) => $e,
            Backend::Dylib($s) => $e,
        }
    };
}

/// An oxilite store loaded with a dataset.
pub struct OxiliteEngine {
    backend: Backend,
    options: QueryOptions,
}

impl OxiliteEngine {
    pub fn new(variant: Variant, dataset: &Dataset) -> Result<Self> {
        let backend = match variant {
            Variant::Dylib => Backend::Dylib(oxilite::store::Store::open_with_library(
                oxilite::dylib::find_system_library().context("no system SQLite")?,
                ":memory:",
            )?),
            _ => Backend::Rusqlite(oxilite::store::Store::new()?),
        };
        let me = Self {
            backend,
            options: QueryOptions {
                sqlite_planner: variant == Variant::RusqliteSqlitePlanner,
                ..QueryOptions::default()
            },
        };
        with_store!(me, s => s.extend(dataset.iter().map(|q| q.into_owned())))?;
        if variant == Variant::RusqliteOptimized {
            with_store!(me, s => s.optimize())?;
        }
        Ok(me)
    }

    pub fn query(&self, query: &Query) -> Result<QueryResults<'static>> {
        Ok(with_store!(self, s => s.query_opt(query, self.options.clone()))?)
    }

    pub fn update(&self, update: &Update) -> Result<()> {
        Ok(with_store!(self, s => s.update(update))?)
    }

    pub fn explain(&self, query: &Query) -> String {
        with_store!(self, s => s.explain_opt(query, &self.options))
            .unwrap_or_else(|e| format!("-- explain failed: {e}"))
    }

    pub fn dataset(&self) -> Result<Dataset> {
        Ok(with_store!(self, s => s.iter().collect::<Result<Dataset, _>>())?)
    }
}

/// Does the query use SERVICE (out of scope for oxilite)?
pub fn uses_service(query: &Query) -> bool {
    fn walk(p: &GraphPattern) -> bool {
        match p {
            GraphPattern::Service { .. } => true,
            GraphPattern::Join { left, right }
            | GraphPattern::LeftJoin { left, right, .. }
            | GraphPattern::Union { left, right }
            | GraphPattern::Minus { left, right } => walk(left) || walk(right),
            GraphPattern::Filter { inner, .. }
            | GraphPattern::Graph { inner, .. }
            | GraphPattern::Extend { inner, .. }
            | GraphPattern::OrderBy { inner, .. }
            | GraphPattern::Project { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. }
            | GraphPattern::Group { inner, .. } => walk(inner),
            _ => false,
        }
    }
    match query {
        Query::Select { pattern, .. }
        | Query::Construct { pattern, .. }
        | Query::Describe { pattern, .. }
        | Query::Ask { pattern, .. } => walk(pattern),
    }
}
