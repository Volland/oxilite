//! Errors raised by JSON-LD document storage.

/// Result alias of this crate.
pub type Result<T, E = JsonLdError> = std::result::Result<T, E>;

/// Why a JSON-LD operation failed. A failed write never changes the store.
#[derive(Debug, thiserror::Error)]
pub enum JsonLdError {
    /// The input is not JSON.
    #[error("invalid JSON: {0}")]
    Json(String),
    /// JSON-LD processing failed; `code` is the JSON-LD API error code (e.g. `invalid local context`).
    #[error("JSON-LD error ({code}): {message}")]
    JsonLd { code: String, message: String },
    /// A remote context is neither registered, bundled, persisted nor fetchable.
    #[error("loading remote context failed: {0}")]
    ContextNotFound(String),
    /// The key strategy found no key and the missing-key policy is `Reject`.
    #[error("the document has no key ({0})")]
    MissingKey(String),
    /// The target graph name is not an absolute IRI.
    #[error("invalid graph name: {0}")]
    InvalidGraphName(String),
    /// A named graph written by the document already belongs to another document.
    #[error("a graph of this document is already owned by another document: {0}")]
    GraphOwned(String),
    /// The document's single atomic request exceeds the backend's limits.
    #[error("document too large for one atomic request: {statements} statements (backend limit {limit})")]
    DocumentTooLarge { statements: usize, limit: usize },
    /// The document is valid JSON-LD but not valid for the profile (e.g. not a credential).
    #[error("invalid document: {0}")]
    Invalid(String),
    /// Storage error.
    #[error(transparent)]
    Store(#[from] oxilite_core::Error),
}

impl JsonLdError {
    /// Maps backend errors of this crate's statements back to meaningful errors.
    pub fn from_store(e: oxilite_core::Error) -> Self {
        let msg = e.to_string();
        if msg.contains("jsonld_graphs.g") {
            Self::GraphOwned(msg)
        } else {
            Self::Store(e)
        }
    }

    /// The JSON-LD error code, when this is a JSON-LD processing error.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::JsonLd { code, .. } => Some(code),
            Self::ContextNotFound(_) => Some("loading remote context failed"),
            _ => None,
        }
    }
}
