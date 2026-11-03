/// Errors of N3 reasoning.
#[derive(Debug, thiserror::Error)]
pub enum N3Error {
    /// The rule text does not parse.
    #[error("N3 syntax error at line {line}: {message}")]
    Parse { line: usize, message: String },
    /// A limit stopped the run, or a builtin failed, and the caller asked for complete results.
    #[error("N3 reasoning is incomplete: {0}")]
    Incomplete(String),
    /// An inference fuse (`{ … } => false`) fired: the data violates the rules.
    #[error("N3 inference fuse fired: {0}")]
    Fuse(String),
    #[error(transparent)]
    Store(#[from] oxilite_core::Error),
}

pub type Result<T> = std::result::Result<T, N3Error>;
