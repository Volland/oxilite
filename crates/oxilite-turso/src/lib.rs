//! Turso backend for oxilite.
//!
//! [Turso](https://github.com/tursodatabase/turso) is SQLite rewritten in Rust: the same SQL and
//! file format, in-process, no C toolchain, plus vector types, vector distances and index
//! methods. oxilite only ever emits SQL, so Turso is one more backend behind the sans-IO
//! contract, and it is the backend where vector indexes (`oxilite_core::vector`) work.
//!
//! ```
//! use oxilite_turso::TursoBackend;
//! use oxilite_core::{Request, SyncBackend};
//!
//! let backend = TursoBackend::memory()?;
//! assert!(backend.capabilities().vectors);
//! let r = backend.execute(&Request::read(vec![
//!     "SELECT vector_distance_cos(vector32('[1,0]'), vector32('[0,1]'))".into(),
//! ]))?;
//! assert_eq!(r[0].rows[0][0].as_f64(), Some(1.0));
//! # Ok::<_, oxilite_core::Error>(())
//! ```
//!
//! Two DDL forms Turso rejects are adapted without changing their meaning (see [`adapt`]); the
//! FTS5 text index is not available. The sync API runs Turso's async API to completion on the
//! calling thread: Turso's local IO needs no runtime.
//!
// @lat: [[architecture#Backends#Turso]]

use futures::executor::block_on;
use futures::lock::{Mutex, MutexGuard};
use oxilite_core::sql::{Capabilities, Mode, Request, Response, ResultSet, SqlValue, Statement};
use oxilite_core::{AsyncBackend, Error, Result, SyncBackend};
use std::borrow::Cow;
use std::path::Path;

/// A Turso database used as an oxilite backend.
pub struct TursoBackend {
    // Keeps the database open for the connection's lifetime.
    _db: turso::Database,
    conn: Mutex<turso::Connection>,
    caps: Capabilities,
}

fn err(e: turso::Error) -> Error {
    Error::backend(e)
}

impl TursoBackend {
    /// An in-memory database.
    pub fn memory() -> Result<Self> {
        Self::build(turso::Builder::new_local(":memory:"))
    }

    /// A database file (created if missing).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::build(turso::Builder::new_local(&path_str(path.as_ref())?))
    }

    /// A database file opened read-only.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        Self::build(turso::Builder::new_local(&path_str(path.as_ref())?).read_only(true))
    }

    /// A database configured by the caller (encryption, IO…). Index methods are enabled here.
    pub fn build(builder: turso::Builder) -> Result<Self> {
        let db = block_on(builder.experimental_index_method(true).build()).map_err(err)?;
        Self::from_database(db)
    }

    /// Wraps an open database.
    pub fn from_database(db: turso::Database) -> Result<Self> {
        let conn = db.connect().map_err(err)?;
        conn.busy_timeout(std::time::Duration::from_secs(30))
            .map_err(err)?;
        Ok(Self {
            _db: db,
            conn: Mutex::new(conn),
            caps: Capabilities {
                name: "turso".into(),
                vectors: true,
                vector_index_methods: true,
                ..Capabilities::native()
            },
        })
    }

    /// The connection, for the sync API. Requests are serialized: an async lock, because the
    /// async API holds it across the awaits of a whole request.
    fn conn(&self) -> MutexGuard<'_, turso::Connection> {
        block_on(self.conn.lock())
    }

    /// Runs a closure with the underlying connection.
    pub fn with_connection<T>(&self, f: impl FnOnce(&turso::Connection) -> T) -> T {
        f(&self.conn())
    }
}

fn path_str(p: &Path) -> Result<String> {
    p.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::Other(format!("{} is not a UTF-8 path", p.display())))
}

/// Adapts oxilite's SQL to Turso.
///
/// - Turso indexes no `WITHOUT ROWID` table and rejects `WITHOUT ROWID, STRICT`: such a table
///   becomes a `STRICT` rowid table, whose composite `PRIMARY KEY` is then a unique index, so
///   its covering indexes can be created and every access path remains.
/// - FTS5 does not exist: the text index is refused with an unsupported error.
pub fn adapt(sql: &str) -> Result<Cow<'_, str>> {
    if sql.contains("USING fts5") {
        return Err(Error::unsupported(
            "the full-text index (StoreOptions::text_index, FTS5) is not available on Turso",
        ));
    }
    if sql.contains("WITHOUT ROWID, STRICT") {
        return Ok(Cow::Owned(sql.replace("WITHOUT ROWID, STRICT", "STRICT")));
    }
    Ok(Cow::Borrowed(sql))
}

fn value(v: turso::Value) -> SqlValue {
    match v {
        turso::Value::Null => SqlValue::Null,
        turso::Value::Integer(i) => SqlValue::Integer(i),
        turso::Value::Real(f) => SqlValue::Real(f),
        turso::Value::Text(t) => SqlValue::Text(t),
        // oxilite never selects blobs but vector columns: hex keeps them lossless.
        turso::Value::Blob(b) => SqlValue::Text(b.iter().map(|x| format!("{x:02x}")).collect()),
    }
}

fn param(v: &SqlValue) -> turso::Value {
    match v {
        SqlValue::Null => turso::Value::Null,
        SqlValue::Integer(i) => turso::Value::Integer(*i),
        SqlValue::Real(f) => turso::Value::Real(*f),
        SqlValue::Text(t) => turso::Value::Text(t.clone()),
    }
}

async fn run(conn: &turso::Connection, s: &Statement) -> Result<ResultSet> {
    let sql = adapt(&s.sql)?;
    let mut stmt = conn.prepare(sql.as_ref()).await.map_err(err)?;
    let params: Vec<turso::Value> = s.params.iter().map(param).collect();
    let n = stmt.column_count();
    if n == 0 {
        let changes = stmt.execute(params).await.map_err(err)?;
        return Ok(ResultSet {
            rows: Vec::new(),
            changes,
        });
    }
    let mut rows = Vec::new();
    let mut q = stmt.query(params).await.map_err(err)?;
    while let Some(row) = q.next().await.map_err(err)? {
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(value(row.get_value(i).map_err(err)?));
        }
        rows.push(out);
    }
    Ok(ResultSet { rows, changes: 0 })
}

async fn batch(conn: &turso::Connection, sql: &str) -> Result<()> {
    conn.execute_batch(sql).await.map_err(err)
}

async fn execute(conn: &turso::Connection, request: &Request) -> Result<Response> {
    match request.mode {
        Mode::Read => {
            let mut out = Vec::with_capacity(request.statements.len());
            for s in &request.statements {
                out.push(run(conn, s).await?);
            }
            Ok(out)
        }
        Mode::Atomic => {
            batch(conn, "SAVEPOINT oxilite_request").await?;
            let mut out = Vec::with_capacity(request.statements.len());
            for s in &request.statements {
                match run(conn, s).await {
                    Ok(r) => out.push(r),
                    Err(e) => {
                        let _ = batch(conn, "ROLLBACK TO oxilite_request; RELEASE oxilite_request")
                            .await;
                        return Err(e);
                    }
                }
            }
            batch(conn, "RELEASE oxilite_request").await?;
            Ok(out)
        }
    }
}

impl SyncBackend for TursoBackend {
    fn execute(&self, request: &Request) -> Result<Response> {
        let conn = self.conn();
        block_on(execute(&conn, request))
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn begin(&self) -> Result<()> {
        let conn = self.conn();
        block_on(batch(&conn, "SAVEPOINT oxilite_tx"))
    }

    fn commit(&self) -> Result<()> {
        let conn = self.conn();
        block_on(batch(&conn, "RELEASE oxilite_tx"))
    }

    fn rollback(&self) -> Result<()> {
        let conn = self.conn();
        block_on(batch(&conn, "ROLLBACK TO oxilite_tx; RELEASE oxilite_tx"))
    }
}

impl AsyncBackend for TursoBackend {
    async fn execute(&self, request: &Request) -> Result<Response> {
        let conn = self.conn.lock().await;
        execute(&conn, request).await
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapts_ddl() {
        assert_eq!(
            adapt("CREATE TABLE t (a INTEGER, PRIMARY KEY (a)) WITHOUT ROWID, STRICT").unwrap(),
            "CREATE TABLE t (a INTEGER, PRIMARY KEY (a)) STRICT"
        );
        assert!(adapt("CREATE VIRTUAL TABLE f USING fts5(x)")
            .unwrap_err()
            .is_unsupported());
    }

    // @lat: [[tests#Turso#Failed atomic request changes nothing]]
    #[test]
    fn atomic_requests_roll_back() {
        let b = TursoBackend::memory().unwrap();
        SyncBackend::execute(
            &b,
            &Request::atomic(vec!["CREATE TABLE t (x INTEGER PRIMARY KEY)".into()]),
        )
        .unwrap();
        let r = SyncBackend::execute(
            &b,
            &Request::atomic(vec![
                "INSERT INTO t VALUES (1)".into(),
                "INSERT INTO t VALUES (1)".into(),
            ]),
        );
        assert!(r.is_err());
        let rows = SyncBackend::execute(&b, &Request::read(vec!["SELECT COUNT(*) FROM t".into()]))
            .unwrap();
        assert_eq!(rows[0].rows[0][0], SqlValue::Integer(0));
    }

    #[test]
    fn nested_savepoints() {
        let b = TursoBackend::memory().unwrap();
        SyncBackend::execute(
            &b,
            &Request::atomic(vec!["CREATE TABLE t (x INTEGER PRIMARY KEY)".into()]),
        )
        .unwrap();
        b.begin().unwrap();
        SyncBackend::execute(
            &b,
            &Request::atomic(vec!["INSERT INTO t VALUES (1)".into()]),
        )
        .unwrap();
        b.rollback().unwrap();
        let rows = SyncBackend::execute(&b, &Request::read(vec!["SELECT COUNT(*) FROM t".into()]))
            .unwrap();
        assert_eq!(rows[0].rows[0][0], SqlValue::Integer(0));
    }
}
