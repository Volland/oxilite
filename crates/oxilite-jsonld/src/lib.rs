//! JSON-LD documents in oxilite: each document is stored verbatim in a keyed table and its
//! RDF in a named graph (by default the document's `id`), queryable with SPARQL.
//!
//! This crate is sans-IO like `oxilite-core`: operations are [`oxilite_core::Job`]s that
//! produce SQL requests. The `oxilite` crate (feature `jsonld`) drives them from
//! `Store::jsonld()` and `AsyncStore::jsonld()`.
//!
// @lat: [[architecture#JSON-LD documents]]

pub mod convert;
pub mod error;
pub mod jobs;
#[cfg(feature = "json")]
pub mod json;
pub mod loader;
pub mod options;
pub mod schema;

pub use convert::{blank_prefix, Converted};
pub use error::{JsonLdError, Result};
pub use jobs::*;
pub use json_ld::Loader;
#[cfg(feature = "network")]
pub use loader::http_fetcher;
pub use loader::NoContexts;
pub use options::{
    ContextFetcher, GraphStrategy, JsonLdOptions, KeyStrategy, MetadataIndexes, MissingKey,
    ProcessingMode, RdfDirection,
};
