//! Database schema.
//!
// @lat: [[architecture#Storage schema]]

use crate::encoding::{Tag, INT_OFFSET, PAYLOAD_BITS};
use crate::sql::{Request, Statement};

/// Current schema version stored in `oxilite_meta`.
pub const SCHEMA_VERSION: &str = "1";

/// Options chosen when a store is created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreOptions {
    /// Create the optional `quads_gspo` index (fast `GRAPH <g> { ?s ?p ?o }`, `CLEAR GRAPH`).
    pub graph_index: bool,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self { graph_index: true }
    }
}

/// DDL statements creating (idempotently) the oxilite schema.
pub fn create_schema(options: &StoreOptions) -> Request {
    let mut s = vec![
        "CREATE TABLE IF NOT EXISTS oxilite_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT",
        // Hashed terms. `id` is the rowid alias: the fastest possible key.
        "CREATE TABLE IF NOT EXISTS terms (\
            id INTEGER PRIMARY KEY, \
            lex TEXT NOT NULL, \
            dt TEXT, \
            lang TEXT, \
            dir INTEGER, \
            num REAL, \
            nt INTEGER, \
            ts REAL) STRICT",
        "CREATE INDEX IF NOT EXISTS terms_num ON terms(num) WHERE num IS NOT NULL",
        "CREATE INDEX IF NOT EXISTS terms_ts ON terms(ts) WHERE ts IS NOT NULL",
        // Detects xxh3 collisions atomically: aborts the whole batch.
        "CREATE TRIGGER IF NOT EXISTS terms_collision BEFORE INSERT ON terms \
         WHEN EXISTS (SELECT 1 FROM terms t WHERE t.id = NEW.id AND \
            (t.lex IS NOT NEW.lex OR t.dt IS NOT NEW.dt OR t.lang IS NOT NEW.lang OR t.dir IS NOT NEW.dir)) \
         BEGIN SELECT RAISE(ABORT, 'oxilite: term hash collision'); END",
        "CREATE TABLE IF NOT EXISTS triple_terms (\
            id INTEGER PRIMARY KEY, s INTEGER NOT NULL, p INTEGER NOT NULL, o INTEGER NOT NULL, vk TEXT NOT NULL, sk TEXT NOT NULL) STRICT",
        // The quad table is its own clustered SPOG index; secondary indexes contain every
        // column, so every triple-pattern scan is index-only.
        "CREATE TABLE IF NOT EXISTS quads (\
            s INTEGER NOT NULL, p INTEGER NOT NULL, o INTEGER NOT NULL, g INTEGER NOT NULL DEFAULT 0, \
            PRIMARY KEY (s, p, o, g)) WITHOUT ROWID, STRICT",
        "CREATE INDEX IF NOT EXISTS quads_posg ON quads(p, o, s, g)",
        "CREATE INDEX IF NOT EXISTS quads_ospg ON quads(o, s, p, g)",
        "CREATE TABLE IF NOT EXISTS graphs (id INTEGER PRIMARY KEY) STRICT",
        "CREATE TABLE IF NOT EXISTS stats_pred (\
            p INTEGER PRIMARY KEY, triples INTEGER NOT NULL, distinct_s INTEGER NOT NULL, distinct_o INTEGER NOT NULL) STRICT",
        "CREATE TABLE IF NOT EXISTS stats_class (o INTEGER PRIMARY KEY, instances INTEGER NOT NULL) STRICT",
        // Staging table for SPARQL UPDATE (DELETE/INSERT … WHERE) inside one atomic batch.
        "CREATE TABLE IF NOT EXISTS update_buffer (\
            op INTEGER NOT NULL, s INTEGER NOT NULL, p INTEGER NOT NULL, o INTEGER NOT NULL, g INTEGER NOT NULL) STRICT",
        // Assertions inside atomic batches: inserting a non-NULL value aborts the batch with a
        // "CHECK constraint failed: <name>" error naming the violated SPARQL condition.
        "CREATE TABLE IF NOT EXISTS oxilite_guard (\
            graph_does_not_exist INTEGER CHECK (graph_does_not_exist IS NULL), \
            graph_already_exists INTEGER CHECK (graph_already_exists IS NULL)) STRICT",
    ]
    .into_iter()
    .map(Statement::from)
    .collect::<Vec<_>>();
    if options.graph_index {
        s.push("CREATE INDEX IF NOT EXISTS quads_gspo ON quads(g, s, p, o)".into());
    }
    s.push(
        format!(
            "INSERT OR IGNORE INTO oxilite_meta(key, value) VALUES ('schema_version', '{SCHEMA_VERSION}'), ('graph_index', '{}'), ('int_offset', '{INT_OFFSET}'), ('payload_bits', '{PAYLOAD_BITS}'), ('integer_tag', '{}')",
            u8::from(options.graph_index),
            Tag::Integer as u8
        )
        .into(),
    );
    Request::atomic(s)
}
