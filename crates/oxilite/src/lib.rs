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
mod partial;
pub mod store;

pub use async_store::AsyncStore;
pub use oxilite_core as core;
pub use oxilite_core::{
    AsyncBackend, Capabilities, Error, QueryOptions, Result, StoreOptions, SyncBackend,
};

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
