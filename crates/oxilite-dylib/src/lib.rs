//! oxilite backend that loads a SQLite shared library from a path at runtime.
//!
//! Only the stable SQLite C API functions oxilite needs are resolved, so any SQLite ≥ 3.37
//! build works (system `libsqlite3`, a vendor-provided library, SQLCipher, …). No SQLite is
//! linked at build time.
//!
// @lat: [[architecture#Backends#Dynamic libsqlite3]]

#![allow(clippy::missing_transmute_annotations, clippy::type_complexity)]

use libloading::Library;
use oxilite_core::sql::{Capabilities, Mode, Request, Response, ResultSet, SqlValue};
use oxilite_core::{Error, Result, SyncBackend};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::ptr;
use std::sync::Mutex;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_INTEGER: c_int = 1;
const SQLITE_FLOAT: c_int = 2;
const SQLITE_NULL: c_int = 5;
const SQLITE_OPEN_READONLY: c_int = 0x0000_0001;
const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
const SQLITE_OPEN_URI: c_int = 0x0000_0040;
const SQLITE_OPEN_NOMUTEX: c_int = 0x0000_8000;

type Db = *mut c_void;
type Stmt = *mut c_void;

#[allow(clippy::type_complexity)]
struct Api {
    open_v2: unsafe extern "C" fn(*const c_char, *mut Db, c_int, *const c_char) -> c_int,
    close_v2: unsafe extern "C" fn(Db) -> c_int,
    prepare_v2:
        unsafe extern "C" fn(Db, *const c_char, c_int, *mut Stmt, *mut *const c_char) -> c_int,
    step: unsafe extern "C" fn(Stmt) -> c_int,
    finalize: unsafe extern "C" fn(Stmt) -> c_int,
    column_count: unsafe extern "C" fn(Stmt) -> c_int,
    column_type: unsafe extern "C" fn(Stmt, c_int) -> c_int,
    column_int64: unsafe extern "C" fn(Stmt, c_int) -> i64,
    column_double: unsafe extern "C" fn(Stmt, c_int) -> f64,
    column_text: unsafe extern "C" fn(Stmt, c_int) -> *const u8,
    column_bytes: unsafe extern "C" fn(Stmt, c_int) -> c_int,
    errmsg: unsafe extern "C" fn(Db) -> *const c_char,
    changes: unsafe extern "C" fn(Db) -> c_int,
    exec: unsafe extern "C" fn(
        Db,
        *const c_char,
        *const c_void,
        *mut c_void,
        *mut *mut c_char,
    ) -> c_int,
    busy_timeout: unsafe extern "C" fn(Db, c_int) -> c_int,
    libversion_number: unsafe extern "C" fn() -> c_int,
}

/// A SQLite shared library loaded at runtime.
pub struct SqliteLibrary {
    api: Api,
    _lib: Library,
}

impl SqliteLibrary {
    /// Loads the library and resolves the needed symbols.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // SAFETY: loading a library runs its initializers; the caller chose a SQLite library.
        let lib = unsafe { Library::new(path) }.map_err(|e| {
            Error::backend(format!(
                "cannot load SQLite library {}: {e}",
                path.display()
            ))
        })?;
        macro_rules! sym {
            ($name:literal) => {{
                // SAFETY: the symbol has the documented SQLite C signature.
                let s = unsafe { lib.get::<*const c_void>(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| Error::backend(format!("{} is missing symbol {}: {e}", path.display(), $name)))?;
                // SAFETY: transmuting a function pointer obtained from the library to its known signature.
                unsafe { std::mem::transmute::<*const c_void, _>(*s) }
            }};
        }
        let api = Api {
            open_v2: sym!("sqlite3_open_v2"),
            close_v2: sym!("sqlite3_close_v2"),
            prepare_v2: sym!("sqlite3_prepare_v2"),
            step: sym!("sqlite3_step"),
            finalize: sym!("sqlite3_finalize"),
            column_count: sym!("sqlite3_column_count"),
            column_type: sym!("sqlite3_column_type"),
            column_int64: sym!("sqlite3_column_int64"),
            column_double: sym!("sqlite3_column_double"),
            column_text: sym!("sqlite3_column_text"),
            column_bytes: sym!("sqlite3_column_bytes"),
            errmsg: sym!("sqlite3_errmsg"),
            changes: sym!("sqlite3_changes"),
            exec: sym!("sqlite3_exec"),
            busy_timeout: sym!("sqlite3_busy_timeout"),
            libversion_number: sym!("sqlite3_libversion_number"),
        };
        Ok(Self { api, _lib: lib })
    }

    /// `sqlite3_libversion_number()`.
    pub fn version_number(&self) -> i32 {
        // SAFETY: no arguments.
        unsafe { (self.api.libversion_number)() }
    }
}

struct Conn {
    db: Db,
}

// SAFETY: the connection is only used behind a Mutex (opened with SQLITE_OPEN_NOMUTEX).
unsafe impl Send for Conn {}

/// Backend over a dynamically loaded SQLite.
pub struct DylibBackend {
    lib: SqliteLibrary,
    conn: Mutex<Conn>,
    caps: Capabilities,
}

impl DylibBackend {
    /// Loads `library` and opens (or creates) `database` (`":memory:"` for in-memory).
    pub fn open(library: impl AsRef<Path>, database: &str) -> Result<Self> {
        Self::open_with_flags(
            library,
            database,
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
        )
    }

    /// Loads `library` and opens `database` read-only.
    pub fn open_read_only(library: impl AsRef<Path>, database: &str) -> Result<Self> {
        Self::open_with_flags(library, database, SQLITE_OPEN_READONLY)
    }

    fn open_with_flags(library: impl AsRef<Path>, database: &str, flags: c_int) -> Result<Self> {
        let lib = SqliteLibrary::load(library)?;
        let version = lib.version_number();
        if version < 3_037_000 {
            return Err(Error::backend(format!(
                "SQLite library version {version} is too old, oxilite needs 3.37 or newer"
            )));
        }
        let name = CString::new(database).map_err(|e| Error::backend(e.to_string()))?;
        let mut db: Db = ptr::null_mut();
        // SAFETY: valid C string and out pointer.
        let rc = unsafe {
            (lib.api.open_v2)(
                name.as_ptr(),
                &mut db,
                flags | SQLITE_OPEN_URI | SQLITE_OPEN_NOMUTEX,
                ptr::null(),
            )
        };
        if rc != SQLITE_OK {
            let msg = if db.is_null() {
                format!("error code {rc}")
            } else {
                errmsg(&lib, db)
            };
            if !db.is_null() {
                // SAFETY: db was allocated by open_v2.
                unsafe { (lib.api.close_v2)(db) };
            }
            return Err(Error::backend(format!("cannot open {database}: {msg}")));
        }
        // SAFETY: db is open.
        unsafe { (lib.api.busy_timeout)(db, 30_000) };
        let me = Self {
            caps: Capabilities {
                name: format!("dylib (SQLite {version})"),
                ..Capabilities::native()
            },
            lib,
            conn: Mutex::new(Conn { db }),
        };
        if flags & SQLITE_OPEN_READWRITE != 0 && database != ":memory:" {
            me.exec("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL")?;
        }
        Ok(me)
    }

    fn exec(&self, sql: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        exec(&self.lib, conn.db, sql)
    }
}

fn errmsg(lib: &SqliteLibrary, db: Db) -> String {
    // SAFETY: errmsg returns a NUL-terminated string owned by SQLite.
    unsafe { CStr::from_ptr((lib.api.errmsg)(db)) }
        .to_string_lossy()
        .into_owned()
}

fn exec(lib: &SqliteLibrary, db: Db, sql: &str) -> Result<()> {
    let c = CString::new(sql).map_err(|e| Error::backend(e.to_string()))?;
    // SAFETY: valid db and C string; no callback.
    let rc = unsafe {
        (lib.api.exec)(
            db,
            c.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if rc != SQLITE_OK {
        return Err(Error::backend(errmsg(lib, db)));
    }
    Ok(())
}

fn run(lib: &SqliteLibrary, db: Db, sql: &str) -> Result<ResultSet> {
    let c = CString::new(sql).map_err(|e| Error::backend(e.to_string()))?;
    let mut stmt: Stmt = ptr::null_mut();
    // SAFETY: valid db and statement out pointer.
    let rc = unsafe { (lib.api.prepare_v2)(db, c.as_ptr(), -1, &mut stmt, ptr::null_mut()) };
    if rc != SQLITE_OK {
        return Err(Error::backend(errmsg(lib, db)));
    }
    if stmt.is_null() {
        return Ok(ResultSet::default());
    }
    struct Finalize<'a>(&'a SqliteLibrary, Stmt);
    impl Drop for Finalize<'_> {
        fn drop(&mut self) {
            // SAFETY: the statement was prepared and is finalized once.
            unsafe { (self.0.api.finalize)(self.1) };
        }
    }
    let _guard = Finalize(lib, stmt);
    // SAFETY: stmt is a valid prepared statement for all calls below.
    let n = unsafe { (lib.api.column_count)(stmt) };
    let mut rows = Vec::new();
    loop {
        let rc = unsafe { (lib.api.step)(stmt) };
        match rc {
            SQLITE_ROW => {
                let mut row = Vec::with_capacity(n as usize);
                for i in 0..n {
                    row.push(match unsafe { (lib.api.column_type)(stmt, i) } {
                        SQLITE_NULL => SqlValue::Null,
                        SQLITE_INTEGER => {
                            SqlValue::Integer(unsafe { (lib.api.column_int64)(stmt, i) })
                        }
                        SQLITE_FLOAT => SqlValue::Real(unsafe { (lib.api.column_double)(stmt, i) }),
                        _ => {
                            let p = unsafe { (lib.api.column_text)(stmt, i) };
                            let len = unsafe { (lib.api.column_bytes)(stmt, i) } as usize;
                            if p.is_null() {
                                SqlValue::Text(String::new())
                            } else {
                                let bytes = unsafe { std::slice::from_raw_parts(p, len) };
                                SqlValue::Text(String::from_utf8_lossy(bytes).into_owned())
                            }
                        }
                    });
                }
                rows.push(row);
            }
            SQLITE_DONE => break,
            _ => return Err(Error::backend(errmsg(lib, db))),
        }
    }
    let changes = if n == 0 {
        // SAFETY: valid db.
        u64::try_from(unsafe { (lib.api.changes)(db) }).unwrap_or(0)
    } else {
        0
    };
    Ok(ResultSet { rows, changes })
}

impl SyncBackend for DylibBackend {
    fn execute(&self, request: &Request) -> Result<Response> {
        if request.statements.iter().any(|s| !s.params.is_empty()) {
            return Err(Error::unsupported("bound parameters on the dylib backend"));
        }
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let db = conn.db;
        match request.mode {
            Mode::Read => request
                .statements
                .iter()
                .map(|s| run(&self.lib, db, &s.sql))
                .collect(),
            Mode::Atomic => {
                exec(&self.lib, db, "SAVEPOINT oxilite_request")?;
                let r: Result<Response> = request
                    .statements
                    .iter()
                    .map(|s| run(&self.lib, db, &s.sql))
                    .collect();
                match r {
                    Ok(r) => {
                        exec(&self.lib, db, "RELEASE oxilite_request")?;
                        Ok(r)
                    }
                    Err(e) => {
                        let _ = exec(
                            &self.lib,
                            db,
                            "ROLLBACK TO oxilite_request; RELEASE oxilite_request",
                        );
                        Err(e)
                    }
                }
            }
        }
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn begin(&self) -> Result<()> {
        self.exec("SAVEPOINT oxilite_tx")
    }

    fn commit(&self) -> Result<()> {
        self.exec("RELEASE oxilite_tx")
    }

    fn rollback(&self) -> Result<()> {
        self.exec("ROLLBACK TO oxilite_tx; RELEASE oxilite_tx")
    }
}

impl Drop for DylibBackend {
    fn drop(&mut self) {
        let conn = self
            .conn
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !conn.db.is_null() {
            // SAFETY: closes the connection opened in open_with_flags.
            unsafe { (self.lib.api.close_v2)(conn.db) };
            conn.db = ptr::null_mut();
        }
    }
}

/// Well-known locations of a system SQLite library, for tests and examples.
pub fn system_library_candidates() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &[
            "/opt/homebrew/opt/sqlite/lib/libsqlite3.dylib",
            "/usr/local/opt/sqlite/lib/libsqlite3.dylib",
            "/usr/lib/libsqlite3.dylib",
        ]
    } else if cfg!(windows) {
        &["sqlite3.dll"]
    } else {
        &[
            "libsqlite3.so.0",
            "/usr/lib/x86_64-linux-gnu/libsqlite3.so.0",
            "/usr/lib/libsqlite3.so",
        ]
    }
}

/// The library named by `OXILITE_SQLITE_LIBRARY`, or the first loadable system SQLite.
pub fn find_system_library() -> Option<&'static str> {
    if let Ok(p) = std::env::var("OXILITE_SQLITE_LIBRARY") {
        return Some(Box::leak(p.into_boxed_str()));
    }
    system_library_candidates()
        .iter()
        .copied()
        .find(|p| SqliteLibrary::load(p).is_ok_and(|l| l.version_number() >= 3_037_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[tests#Backends#Dylib loads a system SQLite]]
    #[test]
    fn dylib_loads_a_system_sqlite() {
        let Some(lib) = find_system_library() else {
            eprintln!("no system SQLite ≥ 3.37 found, skipping");
            return;
        };
        let b = DylibBackend::open(lib, ":memory:").unwrap();
        let r = b
            .execute(&Request::read(vec!["SELECT 1 + 1, 'x', 2.5, NULL".into()]))
            .unwrap();
        assert_eq!(
            r[0].rows[0],
            vec![
                SqlValue::Integer(2),
                SqlValue::Text("x".into()),
                SqlValue::Real(2.5),
                SqlValue::Null
            ]
        );
        b.execute(&Request::atomic(vec![
            "CREATE TABLE t (x INTEGER PRIMARY KEY)".into(),
        ]))
        .unwrap();
        assert!(b
            .execute(&Request::atomic(vec![
                "INSERT INTO t VALUES (1)".into(),
                "INSERT INTO t VALUES (1)".into()
            ]))
            .is_err());
        let r = b
            .execute(&Request::read(vec!["SELECT COUNT(*) FROM t".into()]))
            .unwrap();
        assert_eq!(r[0].rows[0][0], SqlValue::Integer(0));
    }

    // @lat: [[tests#Backends#Invalid library is reported]]
    #[test]
    fn invalid_library_is_reported() {
        let e = DylibBackend::open("/nonexistent/libsqlite3.so", ":memory:")
            .err()
            .unwrap();
        assert!(e.to_string().contains("cannot load SQLite library"), "{e}");
    }
}
