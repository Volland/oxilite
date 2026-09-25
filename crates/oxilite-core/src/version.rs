//! Optional versioning: a store clock, an immutable change log, and time travel.
//!
//! A store chooses a [`Versioning`] level. `Off` (the default) is the plain store and costs
//! nothing. `Stamped` adds a monotonic store clock: every atomic write opens one *tick* (a row of
//! `ticks`, with its wall time, author and message), and every quad records the tick that added
//! it in `quads.t`. `Log` adds the immutable change log `quad_log`, written by triggers on
//! `quads`, from which any past state is reconstructed (`as_of`).
//!
//! The tick is opened by the store, not by the writers: [`prepare`] prepends one statement to
//! every atomic request that writes `quads`, and [`VersionedBackend`] / [`Versioned`] apply it to
//! every request of a store, so no writer can forget it.
//!
// @lat: [[architecture#Versioning]]

use crate::error::{Error, Result};
use crate::job::{AsyncBackend, Job, Step, SyncBackend};
use crate::sql::{
    col, expect_len, sql_opt_str, sql_str, Capabilities, Mode, Request, Response, Statement,
};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::RwLock;

/// How much history a store keeps. Levels nest: each keeps everything the lower ones keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Versioning {
    /// No history (the default): the plain store, with no extra table, column or statement.
    #[default]
    Off,
    /// A monotonic store clock: one tick per atomic write, and the tick that added each quad.
    Stamped,
    /// An immutable change log: every effective change, and any past state on request.
    Log,
}

impl Versioning {
    pub const ALL: [Versioning; 3] = [Versioning::Off, Versioning::Stamped, Versioning::Log];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Stamped => "stamped",
            Self::Log => "log",
        }
    }

    fn index(self) -> u8 {
        self as u8
    }

    fn from_index(i: u8) -> Self {
        Self::ALL[usize::from(i).min(2)]
    }
}

impl fmt::Display for Versioning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Versioning {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "off" | "none" => Ok(Self::Off),
            "stamped" => Ok(Self::Stamped),
            "log" => Ok(Self::Log),
            _ => Err(Error::Other(format!(
                "unknown versioning level `{s}` (expected off, stamped or log)"
            ))),
        }
    }
}

/// Whether a change log exists, and whether it is still recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum History {
    /// No change log.
    #[default]
    None,
    /// The log records every change (level `log`).
    Live,
    /// The log stopped recording at a freeze (the level was lowered); it still answers as-of
    /// queries up to the freeze.
    Frozen,
}

impl History {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Live => "live",
            Self::Frozen => "frozen",
        }
    }
}

/// The versioning state recorded in `oxilite_meta` (read with the statistics at open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, rename_all = "camelCase"))]
pub struct VersionState {
    pub level: Versioning,
    pub history: History,
    /// `quads.t` exists (it survives a soft downgrade to `off`).
    pub stamp_column: bool,
    /// The optional index on `quads.t` (fast "added since").
    pub stamp_index: bool,
    /// The optional `(p,o,s,g,tx)` and `(o,s,p,g,tx)` log indexes (fast as-of patterns).
    pub as_of_index: bool,
}

impl VersionState {
    /// Absorbs one `oxilite_meta` entry.
    pub fn absorb(&mut self, key: &str, value: &str) {
        match key {
            "versioning" => self.level = value.parse().unwrap_or_default(),
            "history" => {
                self.history = match value {
                    "live" => History::Live,
                    "frozen" => History::Frozen,
                    _ => History::None,
                }
            }
            "stamp_column" => self.stamp_column = value == "1",
            "stamp_index" => self.stamp_index = value == "1",
            "as_of_index" => self.as_of_index = value == "1",
            _ => {}
        }
    }

    /// Is there a clock (`ticks`) to read?
    pub fn has_ticks(&self) -> bool {
        self.level >= Versioning::Stamped || self.stamp_column || self.history != History::None
    }
}

/// Who made a write, and why. Recorded on the write's tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct CommitInfo {
    pub author: Option<String>,
    pub message: Option<String>,
}

/// Options of a level change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, rename_all = "camelCase"))]
pub struct LevelChange {
    /// Create (`Some(true)`) or drop (`Some(false)`) the as-of log indexes.
    pub as_of_index: Option<bool>,
    /// Create or drop the index on `quads.t`.
    pub stamp_index: Option<bool>,
    /// Allow a downgrade to delete data (the change log, the ticks, the stamp column).
    pub allow_loss: bool,
    /// Recorded on the tick of the change.
    pub author: Option<String>,
    pub message: Option<String>,
}

/// Kinds of ticks. Ticks of kind 0 are ordinary writes; the others mark level changes and the
/// boundaries of the recorded history.
pub mod kind {
    /// An atomic write.
    pub const WRITE: i64 = 0;
    /// History starts here (upgrade to `log`): the log holds the whole store at this tick.
    pub const GENESIS: i64 = 1;
    /// History stopped recording (downgrade from `log`); as-of works up to this tick.
    pub const FREEZE: i64 = 2;
    /// History resumed after a freeze: the gap's net changes are recorded on this tick.
    pub const RESUME: i64 = 3;
    /// History was deleted (downgrade with `allow_loss`); nothing before it is queryable.
    pub const DROPPED: i64 = 4;
    /// Another level change (stamping started or stopped, indexes changed).
    pub const LEVEL: i64 = 5;
    /// A purge: quads removed from the store and from the history.
    pub const PURGE: i64 = 6;

    pub fn name(k: i64) -> &'static str {
        match k {
            WRITE => "write",
            GENESIS => "genesis",
            FREEZE => "freeze",
            RESUME => "resume",
            DROPPED => "dropped",
            LEVEL => "level",
            PURGE => "purge",
            _ => "unknown",
        }
    }
}

/// SQL: the current tick.
pub const CURRENT_TICK: &str = "(SELECT max(t) FROM ticks)";

/// SQL: now, in seconds since the epoch (works on every SQLite version and on D1).
pub const NOW: &str = "((julianday('now') - 2440587.5) * 86400.0)";

/// The statement opening a new tick.
pub fn tick_statement(k: i64, author: Option<&str>, message: Option<&str>) -> Statement {
    Statement::new(format!(
        "INSERT INTO ticks(t, time, kind, author, message) SELECT coalesce(max(t), 0) + 1, {NOW}, {k}, {}, {} FROM ticks",
        sql_opt_str(author),
        sql_opt_str(message)
    ))
}

/// Does this statement write `quads`? Every writer of the asserted quads spells its statement
/// with one of these two prefixes (see `writer` and `update`); a test holds them to it.
pub fn writes_quads(s: &Statement) -> bool {
    s.sql.starts_with("INSERT OR IGNORE INTO quads(") || s.sql.starts_with("DELETE FROM quads ")
}

fn opens_own_tick(r: &Request) -> bool {
    r.statements
        .iter()
        .take(PREFIX_LEN)
        .any(|s| s.sql.starts_with("INSERT INTO ticks("))
}

/// The statements [`prepare`] puts in front of a write: the terms of the tick's time, author and
/// message, then the tick itself.
pub const PREFIX_LEN: usize = 2;

/// The statements opening a write tick: the tick's time (read from the host's clock), author and
/// message become terms, so the history can be queried as RDF (`<oxilite:history>`), and the
/// tick row records their ids.
pub fn write_tick_statements(info: &CommitInfo) -> Vec<Statement> {
    let now = oxsdatatypes::DateTime::now().to_string();
    let xsd_dt = "http://www.w3.org/2001/XMLSchema#dateTime";
    let secs = crate::encoding::timestamp(&now, xsd_dt).unwrap_or_default();
    let mut rows = crate::encoding::EncodedRows::default();
    let time = oxrdf::Literal::new_typed_literal(now, oxrdf::NamedNode::new_unchecked(xsd_dt));
    let time_id = rows.term(time.as_ref().into());
    let mut lit = |v: &Option<String>| {
        v.as_deref().map_or_else(
            || "NULL".to_owned(),
            |v| {
                rows.term(oxrdf::LiteralRef::new_simple_literal(v).into())
                    .to_string()
            },
        )
    };
    let (author_id, message_id) = (lit(&info.author), lit(&info.message));
    let mut out = crate::writer::term_statements(&rows, &Capabilities::native());
    out.truncate(1);
    out.push(Statement::new(format!(
        "INSERT INTO ticks(t, time, kind, author, message, time_id, author_id, message_id) \
         SELECT coalesce(max(t), 0) + 1, {}, {}, {}, {}, {time_id}, {author_id}, {message_id} FROM ticks",
        crate::sql::sql_f64(secs),
        kind::WRITE,
        sql_opt_str(info.author.as_deref()),
        sql_opt_str(info.message.as_deref()),
    )));
    out
}

/// The request with a tick opened first, when the level requires one and the request writes
/// quads. `None`: run the request unchanged. The first [`PREFIX_LEN`] results belong to the
/// tick ([`strip`]).
pub fn prepare(request: &Request, level: Versioning, info: &CommitInfo) -> Option<Request> {
    if level < Versioning::Stamped
        || request.mode != Mode::Atomic
        || opens_own_tick(request)
        || !request.statements.iter().any(writes_quads)
    {
        return None;
    }
    let mut statements = write_tick_statements(info);
    debug_assert_eq!(statements.len(), PREFIX_LEN);
    statements.extend(request.statements.iter().cloned());
    Some(Request {
        statements,
        mode: request.mode,
    })
}

/// Removes the results of the statements added by [`prepare`].
pub fn strip(mut response: Response) -> Response {
    response.drain(..PREFIX_LEN.min(response.len()));
    response
}

/// The capabilities a store at `level` hands to its jobs: writers stamp quads, and one
/// statement of every request is reserved for the tick.
pub fn effective_caps(caps: &Capabilities, level: Versioning) -> Capabilities {
    let mut c = caps.clone();
    c.versioning = level;
    if level >= Versioning::Stamped {
        c.max_statements = c.max_statements.saturating_sub(PREFIX_LEN).max(1);
    }
    c
}

/// A job whose atomic writes open a tick (for hosts that run requests themselves, such as the
/// JavaScript drivers).
pub struct Versioned<J> {
    inner: J,
    level: Versioning,
    info: CommitInfo,
    pending: bool,
}

impl<J> Versioned<J> {
    pub fn new(inner: J, level: Versioning, info: CommitInfo) -> Self {
        Self {
            inner,
            level,
            info,
            pending: false,
        }
    }
}

impl<J: Job> Job for Versioned<J> {
    type Output = J::Output;
    fn step(&mut self, response: Option<Response>) -> Result<Step<J::Output>> {
        let response = if std::mem::take(&mut self.pending) {
            response.map(strip)
        } else {
            response
        };
        match self.inner.step(response)? {
            Step::Execute(r) => match prepare(&r, self.level, &self.info) {
                Some(p) => {
                    self.pending = true;
                    Ok(Step::Execute(p))
                }
                None => Ok(Step::Execute(r)),
            },
            done => Ok(done),
        }
    }
}

/// A backend adapter that opens a tick for every write (one per interactive transaction), and
/// hands out capabilities matching the store's level. Stores wrap their backend in it.
pub struct VersionedBackend<B> {
    inner: B,
    caps: [Capabilities; 3],
    level: AtomicU8,
    info: RwLock<CommitInfo>,
    in_transaction: AtomicBool,
    tick_open: AtomicBool,
}

impl<B> VersionedBackend<B> {
    pub fn new(inner: B, caps: &Capabilities, level: Versioning) -> Self {
        Self {
            inner,
            caps: Versioning::ALL.map(|l| effective_caps(caps, l)),
            level: AtomicU8::new(level.index()),
            info: RwLock::new(CommitInfo::default()),
            in_transaction: AtomicBool::new(false),
            tick_open: AtomicBool::new(false),
        }
    }

    pub fn inner(&self) -> &B {
        &self.inner
    }

    pub fn level(&self) -> Versioning {
        Versioning::from_index(self.level.load(Ordering::SeqCst))
    }

    pub fn set_level(&self, level: Versioning) {
        self.level.store(level.index(), Ordering::SeqCst);
    }

    /// Author and message recorded on the ticks of later writes (until changed).
    pub fn set_commit_info(&self, info: CommitInfo) {
        *self
            .info
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = info;
    }

    pub fn commit_info(&self) -> CommitInfo {
        self.info
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn prepared(&self, request: &Request) -> Option<Request> {
        if self.in_transaction.load(Ordering::SeqCst) && self.tick_open.load(Ordering::SeqCst) {
            return None;
        }
        let p = prepare(request, self.level(), &self.commit_info())?;
        if self.in_transaction.load(Ordering::SeqCst) {
            self.tick_open.store(true, Ordering::SeqCst);
        }
        Some(p)
    }
}

impl<B: SyncBackend> SyncBackend for VersionedBackend<B> {
    fn execute(&self, request: &Request) -> Result<Response> {
        match self.prepared(request) {
            Some(p) => self.inner.execute(&p).map(strip),
            None => self.inner.execute(request),
        }
    }
    fn capabilities(&self) -> &Capabilities {
        &self.caps[usize::from(self.level.load(Ordering::SeqCst))]
    }
    fn begin(&self) -> Result<()> {
        self.inner.begin()?;
        self.in_transaction.store(true, Ordering::SeqCst);
        self.tick_open.store(false, Ordering::SeqCst);
        Ok(())
    }
    fn commit(&self) -> Result<()> {
        self.in_transaction.store(false, Ordering::SeqCst);
        self.tick_open.store(false, Ordering::SeqCst);
        self.inner.commit()
    }
    fn rollback(&self) -> Result<()> {
        self.in_transaction.store(false, Ordering::SeqCst);
        self.tick_open.store(false, Ordering::SeqCst);
        self.inner.rollback()
    }
}

impl<B: AsyncBackend> AsyncBackend for VersionedBackend<B> {
    async fn execute(&self, request: &Request) -> Result<Response> {
        match self.prepared(request) {
            Some(p) => self.inner.execute(&p).await.map(strip),
            None => self.inner.execute(request).await,
        }
    }
    fn capabilities(&self) -> &Capabilities {
        &self.caps[usize::from(self.level.load(Ordering::SeqCst))]
    }
}

// ------------------------------------------------------------------------------------ schema

fn meta(entries: &[(&str, &str)]) -> Statement {
    Statement::new(format!(
        "INSERT OR REPLACE INTO oxilite_meta(key, value) VALUES {}",
        entries
            .iter()
            .map(|(k, v)| format!("({}, {})", sql_str(k), sql_str(v)))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

const PURGING: &str = "EXISTS (SELECT 1 FROM oxilite_meta WHERE key = 'purging')";

/// The clock: `ticks`, immutable except during a purge.
fn clock_ddl() -> Vec<Statement> {
    [
        // `time_id`, `author_id`, `message_id`: the terms of a write tick (see `write_tick_statements`).
        "CREATE TABLE IF NOT EXISTS ticks (t INTEGER PRIMARY KEY, time REAL NOT NULL, kind INTEGER NOT NULL DEFAULT 0, author TEXT, message TEXT, time_id INTEGER, author_id INTEGER, message_id INTEGER) STRICT".to_owned(),
        // Boundaries of the recorded history, found without scanning the write ticks.
        "CREATE INDEX IF NOT EXISTS ticks_boundary ON ticks(t) WHERE kind > 0".to_owned(),
        format!("CREATE TRIGGER IF NOT EXISTS ticks_immutable_update BEFORE UPDATE ON ticks WHEN NOT {PURGING} BEGIN SELECT RAISE(ABORT, 'oxilite: history is immutable'); END"),
        format!("CREATE TRIGGER IF NOT EXISTS ticks_immutable_delete BEFORE DELETE ON ticks WHEN NOT {PURGING} BEGIN SELECT RAISE(ABORT, 'oxilite: history is immutable'); END"),
    ]
    .into_iter()
    .map(Statement::new)
    .collect()
}

/// The change log and its triggers. A change of the tick being written may be undone within
/// the same tick (a quad added then removed leaves no trace); earlier ticks are immutable.
fn log_tables_ddl() -> Vec<Statement> {
    let past = format!("OLD.tx < {CURRENT_TICK} AND NOT {PURGING}");
    [
        "CREATE TABLE IF NOT EXISTS quad_log (s INTEGER NOT NULL, p INTEGER NOT NULL, o INTEGER NOT NULL, g INTEGER NOT NULL, tx INTEGER NOT NULL, op INTEGER NOT NULL, PRIMARY KEY (s, p, o, g, tx)) WITHOUT ROWID, STRICT".to_owned(),
        // The changes of one commit (log listings, diffs, change feeds).
        "CREATE INDEX IF NOT EXISTS quad_log_tx ON quad_log(tx)".to_owned(),
        "CREATE TABLE IF NOT EXISTS commits (tx INTEGER PRIMARY KEY) STRICT".to_owned(),
        format!("CREATE TRIGGER IF NOT EXISTS quad_log_immutable_update BEFORE UPDATE ON quad_log WHEN {past} BEGIN SELECT RAISE(ABORT, 'oxilite: history is immutable'); END"),
        format!("CREATE TRIGGER IF NOT EXISTS quad_log_immutable_delete BEFORE DELETE ON quad_log WHEN {past} BEGIN SELECT RAISE(ABORT, 'oxilite: history is immutable'); END"),
        format!("CREATE TRIGGER IF NOT EXISTS commits_immutable_delete BEFORE DELETE ON commits WHEN NOT {PURGING} BEGIN SELECT RAISE(ABORT, 'oxilite: history is immutable'); END"),
    ]
    .into_iter()
    .map(Statement::new)
    .collect()
}

/// The triggers recording every effective change of `quads` in the log.
fn capture_ddl() -> Vec<Statement> {
    let same = |r: &str, x: &str| {
        format!("{r}.s = {x}.s AND {r}.p = {x}.p AND {r}.o = {x}.o AND {r}.g = {x}.g")
    };
    let ins_same = same("l", "NEW");
    let del_same = same("l", "OLD");
    // SQLite forbids aliases on the target of a DELETE inside a trigger (D1 enforces it).
    let plain = |x: &str| format!("s = {x}.s AND p = {x}.p AND o = {x}.o AND g = {x}.g");
    let (ins_plain, del_plain) = (plain("NEW"), plain("OLD"));
    vec![
        Statement::new(format!(
            "CREATE TRIGGER IF NOT EXISTS quads_log_insert AFTER INSERT ON quads WHEN NOT {PURGING} BEGIN \
             INSERT INTO quad_log(s, p, o, g, tx, op) SELECT NEW.s, NEW.p, NEW.o, NEW.g, {CURRENT_TICK}, 1 \
               WHERE NOT EXISTS (SELECT 1 FROM quad_log l WHERE {ins_same} AND l.tx = {CURRENT_TICK} AND l.op = 0); \
             DELETE FROM quad_log WHERE {ins_plain} AND tx = {CURRENT_TICK} AND op = 0; \
             INSERT OR IGNORE INTO commits(tx) SELECT {CURRENT_TICK}; \
             END"
        )),
        Statement::new(format!(
            "CREATE TRIGGER IF NOT EXISTS quads_log_delete AFTER DELETE ON quads WHEN NOT {PURGING} BEGIN \
             INSERT INTO quad_log(s, p, o, g, tx, op) SELECT OLD.s, OLD.p, OLD.o, OLD.g, {CURRENT_TICK}, 0 \
               WHERE NOT EXISTS (SELECT 1 FROM quad_log l WHERE {del_same} AND l.tx = {CURRENT_TICK} AND l.op = 1); \
             DELETE FROM quad_log WHERE {del_plain} AND tx = {CURRENT_TICK} AND op = 1; \
             INSERT OR IGNORE INTO commits(tx) SELECT {CURRENT_TICK}; \
             END"
        )),
    ]
}

fn as_of_index_ddl(on: bool) -> Vec<Statement> {
    if on {
        vec![
            "CREATE INDEX IF NOT EXISTS quad_log_posg ON quad_log(p, o, s, g, tx)".into(),
            "CREATE INDEX IF NOT EXISTS quad_log_ospg ON quad_log(o, s, p, g, tx)".into(),
        ]
    } else {
        vec![
            "DROP INDEX IF EXISTS quad_log_posg".into(),
            "DROP INDEX IF EXISTS quad_log_ospg".into(),
        ]
    }
}

fn stamp_index_ddl(on: bool) -> Statement {
    if on {
        "CREATE INDEX IF NOT EXISTS quads_t ON quads(t)".into()
    } else {
        "DROP INDEX IF EXISTS quads_t".into()
    }
}

/// The statements moving a store from `from` to level `to` (one level at a time, in order), then
/// applying the index options of `change`. One atomic request; also the body of a D1 migration.
///
/// Upgrades never lose data; the upgrade to `log` records the whole store as its genesis
/// commit (or, after a freeze, the net changes of the gap). Downgrades keep data unless
/// `allow_loss`: the log freezes, and the clock stops.
pub fn change_statements(
    from: &VersionState,
    to: Versioning,
    change: &LevelChange,
) -> Result<Vec<Statement>> {
    let mut state = *from;
    let mut out = Vec::new();
    let author = change.author.as_deref();
    let msg = |default: &str| Some(change.message.clone().unwrap_or_else(|| default.to_owned()));
    while state.level < to {
        match state.level {
            Versioning::Off => {
                out.extend(clock_ddl());
                if !state.stamp_column {
                    out.push("ALTER TABLE quads ADD COLUMN t INTEGER NOT NULL DEFAULT 0".into());
                }
                out.push(tick_statement(
                    kind::LEVEL,
                    author,
                    msg("versioning: stamped").as_deref(),
                ));
                out.push(meta(&[("versioning", "stamped"), ("stamp_column", "1")]));
                state.level = Versioning::Stamped;
                state.stamp_column = true;
            }
            Versioning::Stamped => {
                out.extend(log_tables_ddl());
                if state.history == History::Frozen {
                    // The gap's net changes, against the state the log froze at.
                    let freeze =
                        format!("(SELECT max(t) FROM ticks WHERE kind = {})", kind::FREEZE);
                    let frozen = as_of_sql(&freeze);
                    out.push(tick_statement(
                        kind::RESUME,
                        author,
                        msg("history resumed").as_deref(),
                    ));
                    out.push(Statement::new(format!(
                        "INSERT INTO quad_log(s, p, o, g, tx, op) SELECT f.s, f.p, f.o, f.g, {CURRENT_TICK}, 0 FROM {frozen} f \
                         WHERE NOT EXISTS (SELECT 1 FROM quads q WHERE q.s = f.s AND q.p = f.p AND q.o = f.o AND q.g = f.g)"
                    )));
                    out.push(Statement::new(format!(
                        "INSERT INTO quad_log(s, p, o, g, tx, op) SELECT q.s, q.p, q.o, q.g, {CURRENT_TICK}, 1 FROM quads q \
                         WHERE NOT EXISTS (SELECT 1 FROM {frozen} f WHERE q.s = f.s AND q.p = f.p AND q.o = f.o AND q.g = f.g)"
                    )));
                } else {
                    out.push(tick_statement(
                        kind::GENESIS,
                        author,
                        msg("history starts").as_deref(),
                    ));
                    out.push(Statement::new(format!(
                        "INSERT INTO quad_log(s, p, o, g, tx, op) SELECT s, p, o, g, {CURRENT_TICK}, 1 FROM quads"
                    )));
                }
                out.push(format!("INSERT OR IGNORE INTO commits(tx) SELECT {CURRENT_TICK}").into());
                out.extend(capture_ddl());
                out.push(meta(&[("versioning", "log"), ("history", "live")]));
                state.level = Versioning::Log;
                state.history = History::Live;
            }
            Versioning::Log => unreachable!(),
        }
    }
    while state.level > to {
        match state.level {
            Versioning::Log => {
                out.push("DROP TRIGGER IF EXISTS quads_log_insert".into());
                out.push("DROP TRIGGER IF EXISTS quads_log_delete".into());
                if change.allow_loss {
                    out.push("DROP TABLE IF EXISTS quad_log".into());
                    out.push("DROP TABLE IF EXISTS commits".into());
                    out.push(tick_statement(
                        kind::DROPPED,
                        author,
                        msg("history deleted").as_deref(),
                    ));
                    out.push(meta(&[
                        ("versioning", "stamped"),
                        ("history", "none"),
                        ("as_of_index", "0"),
                    ]));
                    state.history = History::None;
                    state.as_of_index = false;
                } else {
                    out.push(tick_statement(
                        kind::FREEZE,
                        author,
                        msg("history frozen").as_deref(),
                    ));
                    out.push(meta(&[("versioning", "stamped"), ("history", "frozen")]));
                    state.history = History::Frozen;
                }
                state.level = Versioning::Stamped;
            }
            Versioning::Stamped => {
                if change.allow_loss {
                    if state.history != History::None {
                        return Err(Error::Other(
                            "the frozen history needs the clock: delete it first (lower the level to `stamped` with allow_loss from `log`)".into(),
                        ));
                    }
                    out.push("DROP INDEX IF EXISTS quads_t".into());
                    out.push("ALTER TABLE quads DROP COLUMN t".into());
                    out.push("DROP TABLE IF EXISTS ticks".into());
                    out.push(meta(&[
                        ("versioning", "off"),
                        ("stamp_column", "0"),
                        ("stamp_index", "0"),
                    ]));
                    state.stamp_column = false;
                    state.stamp_index = false;
                } else {
                    out.push(tick_statement(
                        kind::LEVEL,
                        author,
                        msg("versioning: off").as_deref(),
                    ));
                    out.push(meta(&[("versioning", "off")]));
                }
                state.level = Versioning::Off;
            }
            Versioning::Off => unreachable!(),
        }
    }
    if let Some(on) = change.as_of_index {
        if on && state.history == History::None {
            return Err(Error::Other(
                "the as-of index needs a change log (level `log`)".into(),
            ));
        }
        if on != state.as_of_index {
            out.extend(as_of_index_ddl(on));
            out.push(meta(&[("as_of_index", if on { "1" } else { "0" })]));
        }
    }
    if let Some(on) = change.stamp_index {
        if on && !state.stamp_column {
            return Err(Error::Other(
                "the stamp index needs the store clock (level `stamped`)".into(),
            ));
        }
        if on != state.stamp_index {
            out.push(stamp_index_ddl(on));
            out.push(meta(&[("stamp_index", if on { "1" } else { "0" })]));
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------- as-of

/// SQL: the `(s, p, o, g)` table of the store as it was at tick `t` (a SQL expression): the
/// quads whose latest logged change at or before `t` is an addition.
pub fn as_of_sql(t: &str) -> String {
    format!(
        "(SELECT l.s AS s, l.p AS p, l.o AS o, l.g AS g FROM quad_log l WHERE l.tx <= {t} AND l.op = 1 \
         AND NOT EXISTS (SELECT 1 FROM quad_log r WHERE r.s = l.s AND r.p = l.p AND r.o = l.o AND r.g = l.g AND r.tx > l.tx AND r.tx <= {t}))"
    )
}

/// A version of the store, as users name it.
#[derive(Debug, Clone, PartialEq)]
pub enum VersionRef {
    /// The latest tick (`HEAD`, `main`).
    Head,
    /// `n` commits before the latest (`HEAD~n`, `main~n`, `~n`).
    Back(u64),
    /// A tick number (`#42`, `42`).
    Tick(i64),
    /// The state at an instant (`@2026-09-01T12:00:00Z`, `HEAD@…`, `main@…`), in seconds since
    /// the epoch.
    Time(f64),
}

impl FromStr for VersionRef {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let bad = || {
            Error::Other(format!(
                "invalid version `{s}` (expected HEAD, HEAD~n, a tick number such as #42, or @<xsd:dateTime>)"
            ))
        };
        let rest = s
            .strip_prefix("HEAD")
            .or_else(|| s.strip_prefix("main"))
            .unwrap_or(s);
        if rest.is_empty() {
            return Ok(Self::Head);
        }
        if let Some(n) = rest.strip_prefix('~') {
            return n.parse().map(Self::Back).map_err(|_| bad());
        }
        if let Some(t) = rest.strip_prefix('@') {
            let dt = if t.len() == 10 {
                "http://www.w3.org/2001/XMLSchema#date"
            } else {
                "http://www.w3.org/2001/XMLSchema#dateTime"
            };
            return crate::encoding::timestamp(t, dt)
                .map(Self::Time)
                .ok_or_else(bad);
        }
        let n = rest.strip_prefix('#').unwrap_or(rest);
        n.parse().map(Self::Tick).map_err(|_| bad())
    }
}

impl fmt::Display for VersionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Head => f.write_str("HEAD"),
            Self::Back(n) => write!(f, "HEAD~{n}"),
            Self::Tick(t) => write!(f, "#{t}"),
            Self::Time(t) => write!(f, "@{}", format_time(*t)),
        }
    }
}

fn ref_sql(r: &VersionRef) -> String {
    match r {
        VersionRef::Head => "SELECT max(t) FROM ticks".to_owned(),
        VersionRef::Back(n) => {
            format!("SELECT tx FROM commits ORDER BY tx DESC LIMIT 1 OFFSET {n}")
        }
        VersionRef::Tick(t) => format!("SELECT t FROM ticks WHERE t = {t}"),
        VersionRef::Time(x) => format!(
            "SELECT max(t) FROM ticks WHERE time <= {}",
            crate::sql::sql_f64(*x)
        ),
    }
}

/// Resolves version references to ticks, checking that each lies inside the recorded history.
pub fn resolve_job(refs: Vec<VersionRef>, state: VersionState) -> ResolveJob {
    ResolveJob {
        refs,
        state,
        sent: false,
    }
}

pub struct ResolveJob {
    refs: Vec<VersionRef>,
    state: VersionState,
    sent: bool,
}

impl Job for ResolveJob {
    type Output = Vec<i64>;
    fn step(&mut self, response: Option<Response>) -> Result<Step<Vec<i64>>> {
        if self.state.history == History::None {
            return Err(Error::Other(
                "this store keeps no history: raise its versioning level to `log` for as-of queries".into(),
            ));
        }
        if self.refs.is_empty() {
            return Ok(Step::Done(Vec::new()));
        }
        if !std::mem::replace(&mut self.sent, true) {
            let b = format!(
                "kind IN ({}, {}, {}, {})",
                kind::GENESIS,
                kind::FREEZE,
                kind::RESUME,
                kind::DROPPED
            );
            return Ok(Step::Execute(Request::read(
                self.refs
                    .iter()
                    .map(|r| {
                        Statement::new(format!(
                            "WITH r(t) AS ({}) SELECT r.t, (SELECT b.t FROM ticks b WHERE {b} AND b.t <= r.t ORDER BY b.t DESC LIMIT 1), \
                             (SELECT b.kind FROM ticks b WHERE {b} AND b.t <= r.t ORDER BY b.t DESC LIMIT 1) FROM r",
                            ref_sql(r)
                        ))
                    })
                    .collect(),
            )));
        }
        let response = response.unwrap_or_default();
        expect_len(&response, self.refs.len())?;
        let mut out = Vec::with_capacity(self.refs.len());
        for (r, rs) in self.refs.iter().zip(&response) {
            let row = rs.rows.first();
            let t = row.and_then(|row| col(row, 0).ok()?.as_i64());
            let Some(t) = t else {
                return Err(Error::Other(format!("unknown version {r}")));
            };
            let row = row.expect("row with a tick");
            let boundary = col(row, 1)?.as_i64();
            let k = col(row, 2)?.as_i64();
            match (boundary, k) {
                (Some(_), Some(kind::GENESIS | kind::RESUME)) => {}
                (Some(b), Some(kind::FREEZE)) if t == b => {}
                (Some(b), Some(kind::FREEZE)) => {
                    return Err(Error::Other(format!(
                        "version {r} (#{t}) falls in a gap: history was frozen at #{b} and not recorded after it"
                    )))
                }
                _ => {
                    return Err(Error::Other(format!(
                        "version {r} (#{t}) is before the recorded history"
                    )))
                }
            }
            out.push(t);
        }
        Ok(Step::Done(out))
    }
}

/// `YYYY-MM-DDThh:mm:ss.sssZ` for seconds since the epoch.
pub fn format_time(secs: f64) -> String {
    let ms = (secs * 1000.0).round() as i64;
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

// ------------------------------------------------------------------------------- history jobs

fn id_col(caps: &Capabilities, c: &str) -> String {
    if caps.int64_as_text {
        format!("CAST({c} AS TEXT)")
    } else {
        c.to_owned()
    }
}

fn opt_i64(v: &crate::sql::SqlValue) -> Option<i64> {
    v.as_i64()
}

fn opt_string(v: &crate::sql::SqlValue) -> Option<String> {
    v.clone().into_string()
}

/// The versioning state of a store and where its clock and history stand.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct VersionStatus {
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub state: VersionState,
    /// The latest tick.
    pub head: Option<i64>,
    /// Wall time of the latest tick (seconds since the epoch).
    pub head_time: Option<f64>,
    /// Where the recorded history (re)starts: the latest genesis or resume tick.
    pub genesis: Option<i64>,
    /// The latest freeze, while the history is frozen.
    pub frozen_at: Option<i64>,
    /// Commits in the change log.
    pub commits: Option<u64>,
}

/// Reads the [`VersionStatus`].
pub fn status_job(state: VersionState) -> crate::job::OneShot<VersionStatus> {
    let mut stmts = Vec::new();
    if state.has_ticks() {
        stmts.push(Statement::new(format!(
            "SELECT max(t), (SELECT time FROM ticks WHERE t = (SELECT max(t) FROM ticks)), \
             (SELECT max(t) FROM ticks WHERE kind IN ({}, {})), (SELECT max(t) FROM ticks WHERE kind = {}) FROM ticks",
            kind::GENESIS,
            kind::RESUME,
            kind::FREEZE
        )));
    }
    if state.history != History::None {
        stmts.push("SELECT count(*) FROM commits".into());
    }
    crate::job::OneShot::new(Request::read(stmts), move |r: Response| {
        let mut s = VersionStatus {
            state,
            ..Default::default()
        };
        let mut rs = r.iter();
        if state.has_ticks() {
            if let Some(row) = rs.next().and_then(|x| x.rows.first()) {
                s.head = opt_i64(col(row, 0)?);
                s.head_time = col(row, 1)?.as_f64();
                s.genesis = opt_i64(col(row, 2)?);
                let freeze = opt_i64(col(row, 3)?);
                if state.history == History::Frozen {
                    s.frozen_at = freeze;
                }
            }
        }
        if state.history != History::None {
            if let Some(row) = rs.next().and_then(|x| x.rows.first()) {
                s.commits = opt_i64(col(row, 0)?).map(|n| n as u64);
            }
        }
        Ok(s)
    })
}

/// One entry of the history: a commit, or a level change.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct CommitRecord {
    pub tick: i64,
    /// Seconds since the epoch.
    pub time: f64,
    /// `write`, `genesis`, `freeze`, `resume`, `dropped`, `level` or `purge`.
    pub kind: String,
    pub author: Option<String>,
    pub message: Option<String>,
    /// Quads added and removed (known while a change log exists).
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

/// The latest `limit` entries of the history, newest first.
pub fn log_job(
    state: VersionState,
    limit: usize,
) -> Result<crate::job::OneShot<Vec<CommitRecord>>> {
    if !state.has_ticks() {
        return Err(Error::Other(
            "this store keeps no history: raise its versioning level to `stamped` or `log`".into(),
        ));
    }
    let sql = if state.history == History::None {
        format!("SELECT t, time, kind, author, message, NULL, NULL FROM ticks ORDER BY t DESC LIMIT {limit}")
    } else {
        let filter = if state.level == Versioning::Log {
            "WHERE k.kind <> 0 OR EXISTS (SELECT 1 FROM commits c WHERE c.tx = k.t)"
        } else {
            ""
        };
        format!(
            "SELECT k.t, k.time, k.kind, k.author, k.message, \
             (SELECT count(*) FROM quad_log l WHERE l.tx = k.t AND l.op = 1), \
             (SELECT count(*) FROM quad_log l WHERE l.tx = k.t AND l.op = 0) \
             FROM ticks k {filter} ORDER BY k.t DESC LIMIT {limit}"
        )
    };
    Ok(crate::job::OneShot::new(
        Request::read(vec![Statement::new(sql)]),
        |r: Response| {
            expect_len(&r, 1)?;
            r[0].rows
                .iter()
                .map(|row| {
                    Ok(CommitRecord {
                        tick: opt_i64(col(row, 0)?).unwrap_or_default(),
                        time: col(row, 1)?.as_f64().unwrap_or_default(),
                        kind: kind::name(opt_i64(col(row, 2)?).unwrap_or_default()).to_owned(),
                        author: opt_string(col(row, 3)?),
                        message: opt_string(col(row, 4)?),
                        added: opt_i64(col(row, 5)?).map(|n| n as u64),
                        removed: opt_i64(col(row, 6)?).map(|n| n as u64),
                    })
                })
                .collect()
        },
    ))
}

/// A quad added or removed at a tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub tick: i64,
    pub added: bool,
    pub quad: oxrdf::Quad,
}

/// Runs `sql` (rows `tick, op, s, p, o, g`) and resolves the terms.
pub struct ChangeJob {
    sql: Option<String>,
    caps: Capabilities,
    resolver: crate::resolve::TermResolver,
    rows: Vec<(i64, bool, [i64; 4])>,
    started: bool,
}

impl ChangeJob {
    fn new(sql: String, caps: &Capabilities) -> Self {
        Self {
            sql: Some(sql),
            caps: caps.clone(),
            resolver: crate::resolve::TermResolver::default(),
            rows: Vec::new(),
            started: false,
        }
    }
}

impl Job for ChangeJob {
    type Output = Vec<Change>;
    fn step(&mut self, response: Option<Response>) -> Result<Step<Vec<Change>>> {
        if let Some(sql) = self.sql.take() {
            return Ok(Step::Execute(Request::read(vec![Statement::new(sql)])));
        }
        let response = response.unwrap_or_default();
        if self.started {
            self.resolver.absorb(response)?;
        } else {
            self.started = true;
            for rs in response {
                for row in rs.rows {
                    let ids: Vec<i64> = row
                        .iter()
                        .filter_map(crate::sql::SqlValue::as_i64)
                        .collect();
                    let [t, op, s, p, o, g] = ids[..] else {
                        return Err(Error::corrupted("bad change row"));
                    };
                    for id in [s, p, o] {
                        self.resolver.want(id);
                    }
                    if g != crate::encoding::DEFAULT_GRAPH_ID {
                        self.resolver.want(g);
                    }
                    self.rows.push((t, op == 1, [s, p, o, g]));
                }
            }
        }
        if let Some(r) = self.resolver.request(&self.caps) {
            return Ok(Step::Execute(r));
        }
        let out = self
            .rows
            .iter()
            .map(|(t, added, [s, p, o, g])| {
                let gname = if *g == crate::encoding::DEFAULT_GRAPH_ID {
                    oxrdf::GraphName::DefaultGraph
                } else {
                    crate::encoding::to_graph_name(*g, Some(self.resolver.get(*g)?))?
                };
                Ok(Change {
                    tick: *t,
                    added: *added,
                    quad: crate::encoding::make_quad(
                        self.resolver.get(*s)?,
                        self.resolver.get(*p)?,
                        self.resolver.get(*o)?,
                        gname,
                    )?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Step::Done(out))
    }
}

/// The changes recorded after tick `after` up to tick `until` (inclusive), in order. With a
/// change log: every addition and removal. With only the clock: the quads present now that were
/// added in that range (removals leave no trace).
pub fn changes_job(
    state: VersionState,
    after: i64,
    until: Option<i64>,
    caps: &Capabilities,
) -> Result<ChangeJob> {
    let upto = until
        .map(|u| format!(" AND {{t}} <= {u}"))
        .unwrap_or_default();
    let cols = |t: &str, op: &str| {
        format!(
            "{}, {op}, {}, {}, {}, {}",
            id_col(caps, t),
            id_col(caps, "s"),
            id_col(caps, "p"),
            id_col(caps, "o"),
            id_col(caps, "g")
        )
    };
    let sql = if state.history != History::None {
        format!(
            "SELECT {} FROM quad_log WHERE tx > {after}{} ORDER BY tx, op",
            cols("tx", "op"),
            upto.replace("{t}", "tx")
        )
    } else if state.stamp_column {
        format!(
            "SELECT {} FROM quads WHERE t > {after}{} ORDER BY t",
            cols("t", "1"),
            upto.replace("{t}", "t")
        )
    } else {
        return Err(Error::Other(
            "this store keeps no history: raise its versioning level to `stamped` or `log`".into(),
        ));
    };
    Ok(ChangeJob::new(sql, caps))
}

/// The net difference between two versions (ticks): quads added and removed from `from` to
/// `to`, each reported at tick `to`.
pub fn diff_job(state: VersionState, from: i64, to: i64, caps: &Capabilities) -> Result<ChangeJob> {
    if state.history == History::None {
        return Err(Error::Other(
            "this store keeps no history: raise its versioning level to `log` to compare versions"
                .into(),
        ));
    }
    let (a, b) = (as_of_sql(&from.to_string()), as_of_sql(&to.to_string()));
    let cols = |x: &str| {
        format!(
            "{}, {}, {}, {}",
            id_col(caps, &format!("{x}.s")),
            id_col(caps, &format!("{x}.p")),
            id_col(caps, &format!("{x}.o")),
            id_col(caps, &format!("{x}.g"))
        )
    };
    let not_in = |x: &str, other: &str| {
        format!("NOT EXISTS (SELECT 1 FROM {other} y WHERE y.s = {x}.s AND y.p = {x}.p AND y.o = {x}.o AND y.g = {x}.g)")
    };
    let sql = format!(
        "SELECT {to}, 1, {} FROM {b} x WHERE {} UNION ALL SELECT {to}, 0, {} FROM {a} x WHERE {}",
        cols("x"),
        not_in("x", &a),
        cols("x"),
        not_in("x", &b)
    );
    Ok(ChangeJob::new(sql, caps))
}

/// Removes the quads matching the pattern (term ids; `None` matches anything) from the store and
/// from the whole history, recording a purge tick that names no removed content. For erasure
/// requests: the only operation that rewrites history.
pub fn purge_request(
    state: VersionState,
    pattern: [Option<i64>; 4],
    author: Option<&str>,
    reason: Option<&str>,
) -> Request {
    let w = ["s", "p", "o", "g"]
        .iter()
        .zip(pattern)
        .filter_map(|(c, v)| v.map(|v| format!("{c} = {v}")))
        .collect::<Vec<_>>();
    let w = if w.is_empty() {
        "1".to_owned()
    } else {
        w.join(" AND ")
    };
    let mut s = Vec::new();
    if state.has_ticks() {
        s.push(tick_statement(
            kind::PURGE,
            author,
            Some(reason.unwrap_or("purge")),
        ));
    }
    s.push("INSERT OR REPLACE INTO oxilite_meta(key, value) VALUES ('purging', '1')".into());
    s.push(Statement::new(format!("DELETE FROM quads WHERE {w}")));
    if state.history != History::None {
        s.push(Statement::new(format!("DELETE FROM quad_log WHERE {w}")));
    }
    s.push("DELETE FROM oxilite_meta WHERE key = 'purging'".into());
    Request::atomic(s)
}

/// Rejects version options a query cannot honour: an unresolved version, a store without a
/// change log, and inferences or reasoning, which exist only for the current state.
pub fn check_query_options(stats: &crate::Stats, options: &crate::QueryOptions) -> Result<()> {
    if options.as_of.is_some() && options.as_of_tick.is_none() {
        return Err(Error::Other(
            "the as-of version was not resolved before compiling".into(),
        ));
    }
    if options.as_of_tick.is_none() && options.versions.is_empty() {
        return Ok(());
    }
    if stats.version.history == History::None {
        return Err(Error::Other(
            "this store keeps no history: raise its versioning level to `log` for as-of queries"
                .into(),
        ));
    }
    if options.include_inferred || options.reasoning != crate::reason::Reasoning::None {
        return Err(Error::Other(
            "inferences and reasoning describe the current state only; they cannot be combined with an as-of version".into(),
        ));
    }
    Ok(())
}

/// The version references a query names: its `as_of` option, then each
/// `SERVICE <oxilite:version/REF>` of its pattern (deduplicated, in order).
pub fn query_version_refs(query: &spargebra::Query, options: &crate::QueryOptions) -> Vec<String> {
    // A version already resolved (by a Cypher statement, say) is not resolved again.
    let mut out: Vec<String> = if options.as_of_tick.is_none() {
        options.as_of.iter().cloned().collect()
    } else {
        Vec::new()
    };
    let pattern = match query {
        spargebra::Query::Select { pattern, .. }
        | spargebra::Query::Construct { pattern, .. }
        | spargebra::Query::Describe { pattern, .. }
        | spargebra::Query::Ask { pattern, .. } => pattern,
    };
    collect_services(pattern, &mut out);
    let mut seen = std::collections::HashSet::new();
    out.retain(|r| {
        r != HISTORY_MARKER && !options.versions.contains_key(r) && seen.insert(r.clone())
    });
    out
}

/// Does the query read the history graph? Such queries must compile: the fallback evaluator
/// would see an empty named graph.
pub fn query_reads_history(query: &spargebra::Query) -> bool {
    let mut out = Vec::new();
    let pattern = match query {
        spargebra::Query::Select { pattern, .. }
        | spargebra::Query::Construct { pattern, .. }
        | spargebra::Query::Describe { pattern, .. }
        | spargebra::Query::Ask { pattern, .. } => pattern,
    };
    collect_services(pattern, &mut out);
    out.iter().any(|r| r == HISTORY_MARKER)
}

const HISTORY_MARKER: &str = "\u{1}history";

fn collect_services(p: &spargebra::algebra::GraphPattern, out: &mut Vec<String>) {
    use spargebra::algebra::GraphPattern as G;
    if let G::Graph {
        name: spargebra::term::NamedNodePattern::NamedNode(n),
        ..
    } = p
    {
        if n.as_str() == crate::compiler::HISTORY_GRAPH {
            out.push(HISTORY_MARKER.to_owned());
        }
    }
    match p {
        G::Service { name, inner, .. } => {
            if let spargebra::term::NamedNodePattern::NamedNode(n) = name {
                if let Some(r) = n.as_str().strip_prefix(crate::compiler::VERSION_SERVICE) {
                    out.push(r.to_owned());
                }
            }
            collect_services(inner, out);
        }
        G::Join { left, right }
        | G::LeftJoin { left, right, .. }
        | G::Minus { left, right }
        | G::Union { left, right } => {
            collect_services(left, out);
            collect_services(right, out);
        }
        G::Filter { inner, .. }
        | G::Graph { inner, .. }
        | G::Extend { inner, .. }
        | G::OrderBy { inner, .. }
        | G::Project { inner, .. }
        | G::Distinct { inner }
        | G::Reduced { inner }
        | G::Slice { inner, .. }
        | G::Group { inner, .. } => collect_services(inner, out),
        _ => {}
    }
}

/// Resolves the versions a query names and records their ticks in the options.
pub fn resolve_query_job(
    refs: Vec<String>,
    state: VersionState,
    options: crate::QueryOptions,
) -> Result<ResolveQueryJob> {
    let parsed = refs
        .iter()
        .map(|r| r.parse::<VersionRef>())
        .collect::<Result<Vec<_>>>()?;
    Ok(ResolveQueryJob {
        inner: resolve_job(parsed, state),
        refs,
        options: Some(options),
    })
}

pub struct ResolveQueryJob {
    inner: ResolveJob,
    refs: Vec<String>,
    options: Option<crate::QueryOptions>,
}

impl Job for ResolveQueryJob {
    type Output = crate::QueryOptions;
    fn step(&mut self, response: Option<Response>) -> Result<Step<crate::QueryOptions>> {
        match self.inner.step(response)? {
            Step::Execute(r) => Ok(Step::Execute(r)),
            Step::Done(ticks) => {
                let mut o = self.options.take().expect("resolved once");
                for (r, t) in self.refs.iter().zip(&ticks) {
                    if o.as_of.as_deref() == Some(r.as_str()) {
                        o.as_of_tick = Some(*t);
                    }
                    o.versions.insert(r.clone(), *t);
                }
                Ok(Step::Done(o))
            }
        }
    }
}

/// Changes the level of a store (one atomic request), then reloads its statistics.
pub fn level_change_job(
    state: &VersionState,
    to: Versioning,
    change: &LevelChange,
    caps: &Capabilities,
) -> Result<LevelChangeJob> {
    let statements = change_statements(state, to, change)?;
    Ok(LevelChangeJob {
        change: (!statements.is_empty()).then(|| Request::atomic(statements)),
        load: Some(crate::Stats::load_request(caps)),
    })
}

pub struct LevelChangeJob {
    change: Option<Request>,
    load: Option<Request>,
}

impl Job for LevelChangeJob {
    type Output = crate::Stats;
    fn step(&mut self, response: Option<Response>) -> Result<Step<crate::Stats>> {
        if let Some(r) = self.change.take() {
            return Ok(Step::Execute(r));
        }
        if let Some(r) = self.load.take() {
            return Ok(Step::Execute(r));
        }
        crate::Stats::from_response(&response.unwrap_or_default()).map(Step::Done)
    }
}

// ---------------------------------------------------------------------------- history as data

/// The vocabulary of the history graph (`GRAPH <oxilite:history>`).
pub mod vocab {
    /// `?commit oxl:added <<( s p o )>>`: a triple the commit added.
    pub const ADDED: &str = "https://oxilite.dev/ns#added";
    /// `?commit oxl:removed <<( s p o )>>`: a triple the commit removed.
    pub const REMOVED: &str = "https://oxilite.dev/ns#removed";
    pub const ACTIVITY: &str = "http://www.w3.org/ns/prov#Activity";
    pub const STARTED: &str = "http://www.w3.org/ns/prov#startedAtTime";
    pub const AGENT: &str = "http://www.w3.org/ns/prov#wasAssociatedWith";
    pub const INFORMED_BY: &str = "http://www.w3.org/ns/prov#wasInformedBy";
    pub const COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
    pub const TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
}

/// SQL condition: the ticks shown as commits in the history (alias `k`): with a change log, the
/// ticks that changed something plus level changes; with only the clock, every tick.
fn activity_filter(state: &VersionState, k: &str) -> String {
    if state.history == History::None {
        "1".to_owned()
    } else {
        format!("({k}.kind <> 0 OR EXISTS (SELECT 1 FROM commits c WHERE c.tx = {k}.t))")
    }
}

/// SQL: the `(s, p, o, g)` table of the history graph's commit descriptions. A commit is its
/// tick as an inline `xsd:integer`; its time, author and message are the terms its tick
/// recorded; `prov:wasInformedBy` links it to the commit before.
pub fn history_sql(state: &VersionState) -> Result<String> {
    if !state.has_ticks() {
        return Err(Error::Other(
            "this store keeps no history: raise its versioning level to `stamped` or `log`".into(),
        ));
    }
    let int = crate::compiler::expr_int_base();
    let id = crate::encoding::named_node_id;
    let f = activity_filter(state, "k");
    let fj = activity_filter(state, "j");
    let arm = |p: &str, o: &str, extra: &str| {
        format!(
            "SELECT k.t + {int} AS s, {} AS p, {o} AS o, 0 AS g FROM ticks k WHERE {f}{extra}",
            id(p)
        )
    };
    let parts = vec![
        arm(vocab::TYPE, &id(vocab::ACTIVITY).to_string(), ""),
        arm(vocab::STARTED, "k.time_id", " AND k.time_id IS NOT NULL"),
        arm(vocab::AGENT, "k.author_id", " AND k.author_id IS NOT NULL"),
        arm(
            vocab::COMMENT,
            "k.message_id",
            " AND k.message_id IS NOT NULL",
        ),
        arm(
            vocab::INFORMED_BY,
            &format!("(SELECT max(j.t) FROM ticks j WHERE j.t < k.t AND {fj}) + {int}"),
            &format!(" AND EXISTS (SELECT 1 FROM ticks j WHERE j.t < k.t AND {fj})"),
        ),
    ];
    Ok(format!("({})", crate::sql::union_all(parts, 5)))
}

/// SQL: the Datalog relation `commit(c, parent, time, author)`: every commit (its tick as an
/// inline integer), the commit before it, and its time and author terms.
pub fn commits_rel_sql(state: &VersionState) -> String {
    let int = crate::compiler::expr_int_base();
    let (f, fj) = (activity_filter(state, "k"), activity_filter(state, "j"));
    format!(
        "(SELECT k.t + {int} AS c, (SELECT max(j.t) FROM ticks j WHERE j.t < k.t AND {fj}) + {int} AS parent, \
         k.time_id AS time, k.author_id AS author FROM ticks k WHERE {f})"
    )
}

/// SQL: the Datalog relation `added(s, p, o, g, c)` or `removed(…)` from the change log.
pub fn changes_rel_sql(added: bool) -> String {
    let int = crate::compiler::expr_int_base();
    format!(
        "(SELECT s, p, o, g, tx + {int} AS c FROM quad_log WHERE op = {})",
        u8::from(added)
    )
}

/// SQL: the Datalog relation `branch(name, c)`: `"main"` (term id `main`) and the latest tick.
pub fn branches_rel_sql(main: i64) -> String {
    let int = crate::compiler::expr_int_base();
    format!("(SELECT {main} AS name, max(t) + {int} AS c FROM ticks)")
}
