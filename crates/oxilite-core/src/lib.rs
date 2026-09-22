//! oxilite core: an I/O-free SPARQL-to-SQLite compiler.
//!
//! Every operation is a [`job::Job`] that produces SQL [`sql::Request`]s and consumes
//! [`sql::Response`]s, so the same code drives in-process SQLite, a dynamically loaded
//! `libsqlite3`, Cloudflare D1, or a JavaScript host.

pub mod compiler;
pub mod encoding;
pub mod error;
pub mod fallback;
pub mod job;
pub mod ops;
pub mod query;
pub mod resolve;
pub mod schema;
pub mod sql;
pub mod stats;
pub mod update;
pub mod writer;

pub use compiler::QueryOptions;
pub use error::{Error, Result};
pub use job::{run_async, run_sync, AsyncBackend, Job, Step, SyncBackend};
pub use query::{compile_query, CompiledQuery, QueryJob, QueryOutput};
pub use schema::StoreOptions;
pub use sql::{Capabilities, Mode, Request, Response, ResultSet, SqlValue, Statement};
pub use stats::Stats;
