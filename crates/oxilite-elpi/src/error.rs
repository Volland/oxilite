/// Errors of ELPI programs.
#[derive(Debug, thiserror::Error)]
pub enum ElpiError {
    /// The program does not parse or type-check.
    #[error("ELPI program error: {0}")]
    Program(String),
    /// A runtime error (an unknown predicate, a failed host call, a type error in the goal).
    #[error("ELPI run error: {0}")]
    Run(String),
    /// The run took more resolution steps than `Options::fuel`.
    #[error("ELPI run out of fuel: {0}")]
    OutOfFuel(String),
    /// The host builtins returned more rows than `Options::max_rows`.
    #[error("ELPI run read too many rows: {0}")]
    Budget(String),
    /// This build has no engine (the `native` feature is off).
    #[error("{0}")]
    Unsupported(String),
    #[error(transparent)]
    Store(#[from] oxilite_core::Error),
}

pub type Result<T> = std::result::Result<T, ElpiError>;
