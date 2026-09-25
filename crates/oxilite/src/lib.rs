//! oxilite: an Oxigraph-compatible RDF database and SPARQL engine that uses SQLite as its
//! storage engine.
//!
//! The module layout mirrors Oxigraph (`model`, `io`, `sparql`, `store`), so code written
//! for `oxigraph::store::Store` works with `oxilite::store::Store` after changing the crate
//! name. [`AsyncStore`] offers the same API over async backends such as Cloudflare D1.
//!
//! ```
//! use oxilite::model::*;
//! use oxilite::sparql::QueryResults;
//! use oxilite::store::Store;
//!
//! let store = Store::new()?;
//! let ex = NamedNodeRef::new("http://example.com")?;
//! store.insert(QuadRef::new(ex, ex, ex, GraphNameRef::DefaultGraph))?;
//! if let QueryResults::Solutions(mut solutions) = store.query("SELECT ?s WHERE { ?s ?p ?o }")? {
//!     assert_eq!(solutions.next().unwrap()?.get("s"), Some(&ex.into_owned().into()));
//! }
//! # Result::<_, Box<dyn std::error::Error>>::Ok(())
//! ```

mod async_store;
mod common;
#[cfg(feature = "cypher")]
mod cypher_store;
#[cfg(feature = "datalog")]
mod datalog_store;
#[cfg(feature = "jsonld")]
mod jsonld_store;
mod partial;
mod schema_store;
pub mod store;
#[cfg(feature = "vc")]
mod vc_store;
mod version_store;

pub use async_store::AsyncStore;
pub use oxilite_core as core;
pub use oxilite_core::{
    AsyncBackend, Capabilities, Error, QueryOptions, Result, StoreOptions, SyncBackend,
};
pub use schema_store::{RegisteredGraph, Registration};

/// The schema registry: ontologies and shapes graphs declared as such, and the compiled shape
/// index they feed.
pub mod schema {
    pub use crate::schema_store::{RegisteredGraph, Registration};
    pub use oxilite_core::registry::{SchemaGraph, SchemaRole};
    pub use oxilite_core::shapes::{PropertyShape, ShapeIndex};
}

/// Optional versioning: the store clock, the immutable change log and time travel (see
/// `StoreOptions::versioning`, `Store::set_versioning`, `QueryOptions::as_of`).
pub mod version {
    pub use oxilite_core::version::{
        Change, CommitInfo, CommitRecord, History, LevelChange, VersionRef, VersionState,
        VersionStatus, Versioning,
    };
}

/// The same `Store` as [`store::Store`] (blocking API).
pub mod blocking {
    pub use crate::store::*;
}

/// RDF data model (re-export of `oxrdf`, like `oxigraph::model`).
pub mod model {
    pub use oxrdf::*;
}

/// RDF parsers and serializers (re-export of `oxrdfio`, like `oxigraph::io`).
pub mod io {
    pub use oxrdfio::*;
}

/// SPARQL types (like `oxigraph::sparql`).
pub mod sparql {
    pub use oxilite_core::reason::Reasoning;
    pub use oxilite_core::QueryOptions;
    pub use oxrdf::{Variable, VariableNameParseError};
    pub use spareval::{
        QueryEvaluationError, QueryExplanation, QueryResults, QuerySolution, QuerySolutionIter,
        QueryTripleIter,
    };
    pub use spargebra::{Query, SparqlParser, SparqlSyntaxError, Update};

    /// Errors raised by SPARQL updates.
    pub type UpdateEvaluationError = oxilite_core::Error;

    /// SPARQL result formats (re-export of `sparesults`).
    pub mod results {
        pub use sparesults::*;
    }
}

/// Datalog over the dataset: recursive rules with stratified negation (`Store::datalog`).
#[cfg(feature = "datalog")]
pub mod datalog {
    pub use oxilite_datalog::*;
}

/// openCypher over the property-graph view of the dataset (`Store::cypher`).
#[cfg(feature = "cypher")]
pub mod cypher {
    pub use oxilite_cypher::*;
}

/// JSON-LD documents: verbatim storage, one named graph per document (`Store::jsonld`).
#[cfg(feature = "jsonld")]
pub mod jsonld {
    pub use crate::jsonld_store::{AsyncJsonLdStore, JsonLdStore};
    pub use oxilite_jsonld::*;
}

/// Verifiable Credentials: stored under their id, RDF in a graph per credential
/// (`Store::credentials`).
#[cfg(feature = "vc")]
pub mod vc {
    pub use crate::vc_store::{AsyncCredentialStore, CredentialStore};
    pub use oxilite_vc::*;
}

/// The in-process SQLite backend (bundled SQLite, UDFs).
#[cfg(feature = "rusqlite")]
pub mod rusqlite {
    pub use oxilite_rusqlite::*;
}

/// The backend loading a user-supplied `libsqlite3` at runtime.
#[cfg(feature = "dylib")]
pub mod dylib {
    pub use oxilite_dylib::*;
}

/// The Cloudflare D1 backend (Rust Workers, `wasm32`).
#[cfg(feature = "d1")]
pub mod d1 {
    pub use oxilite_d1::*;
}
