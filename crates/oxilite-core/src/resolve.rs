//! Batched id → term resolution.
//!
//! Query results come back as ids. Inline ids and query constants decode locally; all other
//! distinct ids are resolved together with `SELECT … FROM terms WHERE id IN (…)`, which is
//! cheaper than joining `terms` for every row and column and costs one round-trip.
//!
// @lat: [[architecture#SPARQL to SQL compiler]]

use crate::encoding::{decode_inline, decode_row, make_triple, tag_of, Tag, DEFAULT_GRAPH_ID};
use crate::error::{Error, Result};
use crate::sql::{col, Capabilities, Request, Response, Statement};
use oxrdf::Term;
use std::collections::{BTreeSet, HashMap};

const CHUNK: usize = 400;

#[derive(Debug, Clone, Copy)]
enum Lookup {
    Terms,
    Triples,
}

/// Resolves ids to terms, possibly over several round-trips (nested triple terms).
#[derive(Debug, Default)]
pub struct TermResolver {
    known: HashMap<i64, Term>,
    pending: BTreeSet<i64>,
    triples: HashMap<i64, [i64; 3]>,
    in_flight: Vec<Lookup>,
}

impl TermResolver {
    pub fn with_constants(constants: HashMap<i64, Term>) -> Self {
        Self {
            known: constants,
            ..Self::default()
        }
    }

    /// Registers an id that must be decoded.
    pub fn want(&mut self, id: i64) {
        if id == DEFAULT_GRAPH_ID || self.known.contains_key(&id) || self.triples.contains_key(&id)
        {
            return;
        }
        if let Some(t) = decode_inline(id) {
            self.known.insert(id, t);
            return;
        }
        self.pending.insert(id);
    }

    fn id_col(caps: &Capabilities, c: &str) -> String {
        if caps.int64_as_text {
            format!("CAST({c} AS TEXT)")
        } else {
            c.into()
        }
    }

    /// The next lookup request, if anything is unresolved.
    pub fn request(&mut self, caps: &Capabilities) -> Option<Request> {
        if self.pending.is_empty() {
            return None;
        }
        let pending = std::mem::take(&mut self.pending);
        let (triples, terms): (Vec<i64>, Vec<i64>) = pending
            .into_iter()
            .partition(|id| tag_of(*id) == Some(Tag::Triple));
        let mut stmts = Vec::new();
        self.in_flight.clear();
        for chunk in terms.chunks(CHUNK) {
            stmts.push(Statement::new(format!(
                "SELECT {}, lex, dt, lang, dir FROM terms WHERE id IN ({})",
                Self::id_col(caps, "id"),
                join(chunk)
            )));
            self.in_flight.push(Lookup::Terms);
        }
        for chunk in triples.chunks(CHUNK) {
            stmts.push(Statement::new(format!(
                "SELECT {}, {}, {}, {} FROM triple_terms WHERE id IN ({})",
                Self::id_col(caps, "id"),
                Self::id_col(caps, "s"),
                Self::id_col(caps, "p"),
                Self::id_col(caps, "o"),
                join(chunk)
            )));
            self.in_flight.push(Lookup::Triples);
        }
        Some(Request::read(stmts))
    }

    /// Absorbs the response to [`Self::request`].
    pub fn absorb(&mut self, response: Response) -> Result<()> {
        let kinds = std::mem::take(&mut self.in_flight);
        for (kind, rs) in kinds.into_iter().zip(response) {
            for row in rs.rows {
                let id = col(&row, 0)?
                    .as_i64()
                    .ok_or_else(|| Error::corrupted("bad id in lookup"))?;
                match kind {
                    Lookup::Terms => {
                        let mut it = row.into_iter().skip(1);
                        let lex = it.next().and_then(|v| v.into_string()).unwrap_or_default();
                        let dt = it.next().and_then(|v| v.into_string());
                        let lang = it.next().and_then(|v| v.into_string());
                        let dir = it.next().and_then(|v| v.as_i64());
                        self.known.insert(id, decode_row(id, lex, dt, lang, dir)?);
                    }
                    Lookup::Triples => {
                        let get = |i| {
                            col(&row, i)?
                                .as_i64()
                                .ok_or_else(|| Error::corrupted("bad triple term row"))
                        };
                        let parts = [get(1)?, get(2)?, get(3)?];
                        self.triples.insert(id, parts);
                        for p in parts {
                            self.want(p);
                        }
                    }
                }
            }
        }
        self.assemble();
        Ok(())
    }

    fn assemble(&mut self) {
        loop {
            let ready: Vec<i64> = self
                .triples
                .iter()
                .filter(|(_, parts)| parts.iter().all(|p| self.known.contains_key(p)))
                .map(|(id, _)| *id)
                .collect();
            if ready.is_empty() {
                return;
            }
            for id in ready {
                let [s, p, o] = self.triples.remove(&id).expect("present");
                if let Ok(t) = make_triple(
                    self.known[&s].clone(),
                    self.known[&p].clone(),
                    self.known[&o].clone(),
                ) {
                    self.known.insert(id, t.into());
                }
            }
        }
    }

    /// Is there nothing left to fetch?
    pub fn is_complete(&self) -> bool {
        self.pending.is_empty()
    }

    /// Decodes an id previously registered with [`Self::want`].
    pub fn get(&self, id: i64) -> Result<Term> {
        self.known
            .get(&id)
            .cloned()
            .or_else(|| decode_inline(id))
            .ok_or_else(|| Error::corrupted(format!("term {id} not found in the dictionary")))
    }
}

pub(crate) fn join(ids: &[i64]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Convenience: registers every id of a response column set and returns them.
pub(crate) fn ids_of(response: &Response, i: usize) -> Vec<i64> {
    response
        .get(i)
        .map(|rs| {
            rs.rows
                .iter()
                .filter_map(|r| r.first().and_then(|v| v.as_i64()))
                .collect()
        })
        .unwrap_or_default()
}
