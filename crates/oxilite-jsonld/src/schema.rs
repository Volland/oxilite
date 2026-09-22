//! Tables of JSON-LD document storage, created on demand (stores that never use JSON-LD do
//! not have them).
//!
// @lat: [[architecture#JSON-LD documents#Tables]]

use crate::options::MetadataIndexes;
use oxilite_core::{Request, Statement};

/// Version of the JSON-LD tables, recorded as `jsonld_schema` in `oxilite_meta`.
pub const JSONLD_SCHEMA_VERSION: &str = "1";

/// Idempotent DDL of the JSON-LD tables and the chosen metadata indexes.
pub fn create_schema(indexes: &MetadataIndexes) -> Request {
    Request::atomic(schema_statements(indexes))
}

/// The DDL statements (also for `wrangler d1 migrations`, see [`schema_sql`]).
pub fn schema_statements(indexes: &MetadataIndexes) -> Vec<Statement> {
    let mut s: Vec<Statement> = vec![
        // One row per document: the verbatim JSON plus metadata columns filled by profiles
        // (credentials: issuer, subject, types, validity). Generic documents leave them NULL,
        // which the partial indexes skip.
        "CREATE TABLE IF NOT EXISTS jsonld_documents (\
            key TEXT PRIMARY KEY, \
            graph INTEGER NOT NULL, \
            doc TEXT NOT NULL, \
            sha256 TEXT NOT NULL, \
            profile TEXT NOT NULL, \
            issuer TEXT, \
            subject TEXT, \
            types TEXT, \
            valid_from REAL, \
            valid_until REAL, \
            refs TEXT, \
            stored_at REAL NOT NULL) STRICT"
            .into(),
        // Graphs a document owns (its target graph and the graphs it defines); replace and
        // remove clear exactly these. UNIQUE(g): a graph has one owner.
        "CREATE TABLE IF NOT EXISTS jsonld_graphs (\
            key TEXT NOT NULL, g INTEGER NOT NULL, \
            PRIMARY KEY (key, g), UNIQUE (g)) WITHOUT ROWID, STRICT"
            .into(),
        // Persisted remote contexts, so documents convert offline (D1, wasm).
        "CREATE TABLE IF NOT EXISTS jsonld_contexts (iri TEXT PRIMARY KEY, doc TEXT NOT NULL) STRICT"
            .into(),
    ];
    if indexes.issuer {
        s.push("CREATE INDEX IF NOT EXISTS jsonld_issuer ON jsonld_documents(issuer, valid_until) WHERE issuer IS NOT NULL".into());
    }
    if indexes.subject {
        s.push("CREATE INDEX IF NOT EXISTS jsonld_subject ON jsonld_documents(subject) WHERE subject IS NOT NULL".into());
    }
    if indexes.valid_until {
        s.push("CREATE INDEX IF NOT EXISTS jsonld_valid_until ON jsonld_documents(valid_until) WHERE valid_until IS NOT NULL".into());
    }
    s.push(
        format!(
            "INSERT OR IGNORE INTO oxilite_meta(key, value) VALUES ('jsonld_schema', '{JSONLD_SCHEMA_VERSION}')"
        )
        .into(),
    );
    s
}

/// The core schema plus the JSON-LD tables, as one SQL script.
pub fn schema_sql(options: &oxilite_core::StoreOptions, indexes: &MetadataIndexes) -> String {
    oxilite_core::schema::schema_sql_with(options, &schema_statements(indexes))
}
