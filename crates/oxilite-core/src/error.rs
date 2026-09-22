use std::fmt;

/// Result alias used across oxilite.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors raised by oxilite.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The SPARQL query or update could not be parsed.
    #[error(transparent)]
    Syntax(#[from] spargebra::SparqlSyntaxError),
    /// An RDF document could not be parsed.
    #[error(transparent)]
    Parse(#[from] oxrdfio::RdfParseError),
    /// The SQL backend failed.
    #[error("SQL backend error: {0}")]
    Backend(String),
    /// The operation needs something the compiler or backend cannot do.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Two different terms hashed to the same id.
    #[error("term hash collision: {0}")]
    Collision(String),
    /// The database content is not what oxilite expects.
    #[error("corrupted store: {0}")]
    Corrupted(String),
    /// Evaluation by the fallback evaluator failed.
    #[error(transparent)]
    Evaluation(#[from] spareval::QueryEvaluationError),
    /// I/O error while reading or writing RDF.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Other errors.
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn backend(e: impl fmt::Display) -> Self {
        let msg = e.to_string();
        if msg.contains("oxilite: term hash collision") {
            Self::Collision(msg)
        } else {
            Self::Backend(msg)
        }
    }

    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    pub fn corrupted(msg: impl Into<String>) -> Self {
        Self::Corrupted(msg.into())
    }

    /// Is this error a request for the fallback evaluator?
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported(_))
    }
}
