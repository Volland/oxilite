//! Errors raised while checking, compiling and running a Synalog program.
//!
// @lat: [[architecture#Synalog frontend]]

use thiserror::Error;

/// Everything that can go wrong with a Synalog program.
#[derive(Debug, Error)]
pub enum SynalogError {
    /// The program does not parse.
    #[error("synalog parse error: {0}")]
    Parse(String),

    /// The Synalog verifier rejected the program; every message it produced, in order.
    #[error("synalog verification failed: {}", .0.join("; "))]
    Verify(Vec<String>),

    /// The Synalog compiler rejected the program.
    #[error("synalog compile error: {0}")]
    Compile(String),

    /// A `# @table` / `# @class` pragma that cannot be read.
    #[error("line {line}: {message}")]
    Pragma { line: usize, message: String },

    /// The predicate to run is not defined by the program.
    #[error("the program does not define `{predicate}`; it defines: {}", .defined.join(", "))]
    UnknownPredicate {
        predicate: String,
        defined: Vec<String>,
    },

    /// Something the store (or the backend) cannot run.
    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error(transparent)]
    Store(#[from] oxilite_core::Error),
}

impl SynalogError {
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }
}

pub type Result<T> = std::result::Result<T, SynalogError>;
