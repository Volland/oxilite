//! Greedy, statistics-driven join ordering for basic graph patterns.
//!
// @lat: [[architecture#Query planner]]

use crate::encoding::rdf_type_id;
use crate::stats::Stats;
use std::collections::HashSet;

/// A position of a triple pattern after constant encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pos {
    Const(i64),
    Var(usize),
}

impl Pos {
    fn is_bound(self, bound: &HashSet<usize>) -> bool {
        match self {
            Self::Const(_) => true,
            Self::Var(v) => bound.contains(&v),
        }
    }

    fn var(self) -> Option<usize> {
        match self {
            Self::Var(v) => Some(v),
            Self::Const(_) => None,
        }
    }
}

/// Estimated number of matches of a pattern given the currently bound variables.
pub(crate) fn estimate(tp: &[Pos; 3], bound: &HashSet<usize>, stats: &Stats) -> f64 {
    let [s, p, o] = *tp;
    let (sb, pb, ob) = (s.is_bound(bound), p.is_bound(bound), o.is_bound(bound));
    if stats.available {
        let total = stats.total.max(1.0);
        if let Pos::Const(pid) = p {
            let Some(ps) = stats.predicates.get(&pid) else {
                // Unknown predicate: no triple uses it (as of the last optimize()).
                return 0.5;
            };
            let mut c = ps.triples;
            if sb {
                c /= ps.distinct_subjects;
            }
            if ob {
                match o {
                    Pos::Const(oid) if pid == rdf_type_id() => {
                        c = stats.classes.get(&oid).copied().unwrap_or(0.5);
                        if sb {
                            c = c.min(1.0);
                        }
                    }
                    Pos::Const(oid) => {
                        c = stats
                            .pairs
                            .get(&(pid, oid))
                            .copied()
                            .unwrap_or(c / ps.distinct_objects);
                        if sb {
                            c = c.min(1.0);
                        }
                    }
                    _ => c /= ps.distinct_objects,
                }
            }
            return c.max(0.5);
        }
        let mut c = total;
        if pb {
            c /= stats.predicates.len().max(1) as f64;
        }
        if sb {
            c /= total.sqrt().max(1.0) * 4.0;
        }
        if ob {
            c /= total.sqrt().max(1.0);
        }
        return c.max(0.5);
    }
    // Heuristics: subject ≫ object ≫ predicate selectivity.
    let mut c = 1.0e7;
    if sb {
        c /= 1.0e5;
    }
    if ob {
        c /= 1.0e4;
    }
    if pb {
        c /= 1.0e2;
        if let (Pos::Const(pid), Pos::Const(_)) = (p, o) {
            if pid == rdf_type_id() {
                // Class membership is a weak filter.
                c *= 50.0;
            }
        }
    }
    c
}

/// Orders triple patterns: cheapest first, then cheapest connected one, and so on.
pub(crate) fn order(
    patterns: &[[Pos; 3]],
    pre_bound: &HashSet<usize>,
    stats: &Stats,
) -> Vec<usize> {
    let mut bound = pre_bound.clone();
    let mut remaining: Vec<usize> = (0..patterns.len()).collect();
    let mut out = Vec::with_capacity(patterns.len());
    while !remaining.is_empty() {
        let connected: Vec<usize> = remaining
            .iter()
            .copied()
            .filter(|i| {
                patterns[*i]
                    .iter()
                    .any(|p| p.var().is_some_and(|v| bound.contains(&v)))
            })
            .collect();
        let candidates = if connected.is_empty() || out.is_empty() && bound.is_empty() {
            &remaining
        } else {
            &connected
        };
        let best = *candidates
            .iter()
            .min_by(|a, b| {
                estimate(&patterns[**a], &bound, stats)
                    .total_cmp(&estimate(&patterns[**b], &bound, stats))
                    .then(a.cmp(b))
            })
            .expect("non-empty");
        out.push(best);
        remaining.retain(|i| *i != best);
        for p in patterns[best] {
            if let Some(v) = p.var() {
                bound.insert(v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::named_node_id;
    use crate::stats::PredicateStats;

    // @lat: [[tests#Planner#Rare predicate first]]
    #[test]
    fn rare_predicate_first() {
        let ty = rdf_type_id();
        let common = named_node_id("http://ex/Common");
        let rare = named_node_id("http://ex/rare");
        let mut stats = Stats {
            available: true,
            total: 1010.0,
            ..Stats::default()
        };
        stats.predicates.insert(
            ty,
            PredicateStats {
                triples: 1000.0,
                distinct_subjects: 1000.0,
                distinct_objects: 1.0,
            },
        );
        stats.predicates.insert(
            rare,
            PredicateStats {
                triples: 10.0,
                distinct_subjects: 10.0,
                distinct_objects: 10.0,
            },
        );
        stats.classes.insert(common, 1000.0);
        let patterns = [
            [Pos::Var(0), Pos::Const(ty), Pos::Const(common)],
            [Pos::Var(0), Pos::Const(rare), Pos::Var(1)],
        ];
        assert_eq!(order(&patterns, &HashSet::new(), &stats), vec![1, 0]);
    }

    // @lat: [[tests#Planner#Connected patterns are preferred]]
    #[test]
    fn connected_patterns_are_preferred() {
        let p = named_node_id("http://ex/p");
        let q = named_node_id("http://ex/q");
        // chain ?a p ?b . ?c q ?d . ?b q ?c : after the first pattern, the next must share a var.
        let patterns = [
            [Pos::Var(0), Pos::Const(p), Pos::Var(1)],
            [Pos::Var(2), Pos::Const(q), Pos::Var(3)],
            [Pos::Var(1), Pos::Const(q), Pos::Var(2)],
        ];
        let ord = order(&patterns, &HashSet::new(), &Stats::default());
        let mut bound = HashSet::new();
        for (n, i) in ord.iter().enumerate() {
            let vars: Vec<usize> = patterns[*i].iter().filter_map(|p| p.var()).collect();
            if n > 0 {
                assert!(vars.iter().any(|v| bound.contains(v)), "{ord:?}");
            }
            bound.extend(vars);
        }
    }

    // @lat: [[tests#Planner#Frequent values are not selective]]
    #[test]
    fn frequent_values_are_not_selective() {
        // ?x country ex:US (a value 90% of subjects share) vs ?x tag ?t (10 triples in all):
        // the average (triples / distinct objects) would rank the country pattern first.
        let country = named_node_id("http://ex/country");
        let us = named_node_id("http://ex/US");
        let tag = named_node_id("http://ex/tag");
        let mut stats = Stats {
            available: true,
            total: 1010.0,
            ..Stats::default()
        };
        stats.predicates.insert(
            country,
            PredicateStats {
                triples: 1000.0,
                distinct_subjects: 1000.0,
                distinct_objects: 100.0,
            },
        );
        stats.predicates.insert(
            tag,
            PredicateStats {
                triples: 10.0,
                distinct_subjects: 10.0,
                distinct_objects: 10.0,
            },
        );
        let patterns = [
            [Pos::Var(0), Pos::Const(country), Pos::Const(us)],
            [Pos::Var(0), Pos::Const(tag), Pos::Var(1)],
        ];
        assert_eq!(order(&patterns, &HashSet::new(), &stats), vec![0, 1]);
        stats.pairs.insert((country, us), 900.0);
        assert_eq!(order(&patterns, &HashSet::new(), &stats), vec![1, 0]);
    }

    // @lat: [[tests#Planner#Heuristics without statistics]]
    #[test]
    fn heuristics_without_statistics() {
        let p = named_node_id("http://ex/p");
        let s = named_node_id("http://ex/s");
        let patterns = [
            [Pos::Var(0), Pos::Const(p), Pos::Var(1)],
            [Pos::Const(s), Pos::Var(2), Pos::Var(0)],
        ];
        assert_eq!(
            order(&patterns, &HashSet::new(), &Stats::default()),
            vec![1, 0]
        );
    }
}
