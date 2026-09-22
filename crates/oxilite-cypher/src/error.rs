//! Errors of the Cypher frontend.

use thiserror::Error;

pub type Result<T, E = CypherError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum CypherError {
    /// The query text is not valid Cypher.
    #[error("Cypher syntax error at offset {pos}: {message}")]
    Syntax { pos: usize, message: String },
    /// The query is valid but uses something oxilite does not implement.
    #[error("unsupported Cypher: {0}")]
    Unsupported(String),
    /// A semantic error (unknown variable, wrong kind, missing parameter…).
    #[error("Cypher semantic error: {0}")]
    Semantic(String),
    /// A runtime error raised while evaluating the query (type errors, constraint violations).
    #[error("Cypher runtime error: {0}")]
    Runtime(String),
    /// A write would leave the graph violating a SHACL shape.
    #[error("SHACL violation: {0}")]
    ShapeViolation(String),
    /// An error of the underlying store.
    #[error(transparent)]
    Store(#[from] oxilite_core::Error),
}

impl CypherError {
    pub(crate) fn syntax(pos: usize, message: impl Into<String>) -> Self {
        Self::Syntax {
            pos,
            message: message.into(),
        }
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }

    pub(crate) fn semantic(message: impl Into<String>) -> Self {
        Self::Semantic(message.into())
    }

    pub(crate) fn runtime(message: impl Into<String>) -> Self {
        Self::Runtime(message.into())
    }
}
