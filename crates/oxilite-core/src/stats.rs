//! Statistics used by the join-order planner.
//!
//! Stats are refreshed explicitly (`optimize()`, or after a bulk load) rather than on every
//! write: on D1 every index entry written is billed, and a per-write counter would be a hot
//! row. Stale stats only degrade plan quality, never correctness.
//!
// @lat: [[architecture#Query planner#Statistics]]

use crate::encoding::rdf_type_id;
use crate::error::Result;
use crate::sql::{col, expect_len, Capabilities, Request, Response, Statement};
use std::collections::HashMap;

/// Per-predicate statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PredicateStats {
    pub triples: f64,
    pub distinct_subjects: f64,
    pub distinct_objects: f64,
}

/// Planner statistics loaded from `stats_pred` / `stats_class`.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Stats {
    /// `true` when `optimize()` has been run at least once.
    pub available: bool,
    pub total: f64,
    pub predicates: HashMap<i64, PredicateStats>,
    /// Instance count per `rdf:type` class.
    pub classes: HashMap<i64, f64>,
    pub graph_index: bool,
}

fn id_col(caps: &Capabilities, c: &str) -> String {
    if caps.int64_as_text {
        format!("CAST({c} AS TEXT)")
    } else {
        c.into()
    }
}

impl Stats {
    /// Statements that load statistics and store settings.
    pub fn load_request(caps: &Capabilities) -> Request {
        Request::read(vec![
            Statement::new("SELECT key, value FROM oxilite_meta"),
            Statement::new(format!(
                "SELECT {}, triples, distinct_s, distinct_o FROM stats_pred",
                id_col(caps, "p")
            )),
            Statement::new(format!(
                "SELECT {}, instances FROM stats_class",
                id_col(caps, "o")
            )),
        ])
    }

    pub fn from_response(response: &Response) -> Result<Self> {
        expect_len(response, 3)?;
        let mut stats = Self::default();
        for row in &response[0].rows {
            let key = col(row, 0)?.as_str().unwrap_or_default();
            let value = col(row, 1)?.clone().into_string().unwrap_or_default();
            match key {
                "graph_index" => stats.graph_index = value == "1",
                "total" => {
                    stats.total = value.parse().unwrap_or(0.0);
                    stats.available = true;
                }
                _ => {}
            }
        }
        for row in &response[1].rows {
            let (Some(p), Some(t), Some(ds), Some(d_o)) = (
                col(row, 0)?.as_i64(),
                col(row, 1)?.as_f64(),
                col(row, 2)?.as_f64(),
                col(row, 3)?.as_f64(),
            ) else {
                continue;
            };
            stats.predicates.insert(
                p,
                PredicateStats {
                    triples: t,
                    distinct_subjects: ds.max(1.0),
                    distinct_objects: d_o.max(1.0),
                },
            );
        }
        for row in &response[2].rows {
            if let (Some(o), Some(n)) = (col(row, 0)?.as_i64(), col(row, 1)?.as_f64()) {
                stats.classes.insert(o, n);
            }
        }
        Ok(stats)
    }

    /// Statements recomputing statistics (run by `optimize()`).
    pub fn refresh_request() -> Request {
        Request::atomic(vec![
            "DELETE FROM stats_pred".into(),
            "INSERT INTO stats_pred(p, triples, distinct_s, distinct_o) \
             SELECT p, COUNT(*), COUNT(DISTINCT s), COUNT(DISTINCT o) FROM quads GROUP BY p"
                .into(),
            "DELETE FROM stats_class".into(),
            Statement::new(format!(
                "INSERT INTO stats_class(o, instances) SELECT o, COUNT(*) FROM quads WHERE p = {} GROUP BY o",
                rdf_type_id()
            )),
            "INSERT OR REPLACE INTO oxilite_meta(key, value) SELECT 'total', CAST(COUNT(*) AS TEXT) FROM quads"
                .into(),
        ])
    }
}
