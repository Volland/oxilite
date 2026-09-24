//! Errors raised while parsing, checking and compiling a Datalog program.
//!
// @lat: [[architecture#Datalog frontend]]

use thiserror::Error;

/// A position in the program text, for parse diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// Everything that can go wrong with a Datalog program.
#[derive(Debug, Error)]
pub enum DatalogError {
    #[error("datalog parse error at {span}: {message}")]
    Parse { span: Span, message: String },

    /// A variable that no positive body atom binds.
    #[error("unsafe rule for `{predicate}`: {variable} is not bound by a positive body atom")]
    Unsafe { predicate: String, variable: String },

    /// Negation or aggregation inside a recursive component.
    #[error("{kind} is not stratified: it occurs inside the recursive component {component:?}")]
    Unstratified {
        kind: &'static str,
        component: Vec<String>,
    },

    /// A rule head that has no RDF form, when materializing.
    #[error("cannot materialize `{predicate}`: a rule head must be an IRI predicate with two arguments, or triple/3 or triple/4")]
    NotTripleShaped { predicate: String },

    /// Two rules give the same predicate different arities.
    #[error("predicate `{predicate}` is used with arity {found} and with arity {expected}")]
    Arity {
        predicate: String,
        expected: usize,
        found: usize,
    },

    /// An undefined prefix in a CURIE.
    #[error("undefined prefix `{0}:`")]
    UnknownPrefix(String),

    /// The program has no goal to answer.
    #[error("the program has no goal: add a line of the form `?- p(?x, ?y).`")]
    NoGoal,

    /// A goal naming a predicate that no rule defines.
    #[error("the goal names `{0}`, which no rule defines")]
    UnknownGoal(String),

    /// A component that kept deriving new facts past the configured bound.
    #[error("the component {component} did not reach a fixpoint within {rounds} rounds; raise Options::max_iterations if the program really needs more")]
    IterationLimit { component: String, rounds: usize },

    /// Something the SQL backend cannot do.
    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error(transparent)]
    Store(#[from] oxilite_core::Error),
}

impl DatalogError {
    pub(crate) fn parse(span: Span, message: impl Into<String>) -> Self {
        Self::Parse {
            span,
            message: message.into(),
        }
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }
}

pub type Result<T> = std::result::Result<T, DatalogError>;
