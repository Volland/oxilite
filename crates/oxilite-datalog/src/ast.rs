//! The Datalog abstract syntax tree.
//!
//! A program is a list of rules over RDF terms, optionally ending in a goal. Atoms are either
//! an IRI predicate applied to two arguments (one triple pattern), a derived predicate, or the
//! built-in `triple/3` and `triple/4` forms.
//!
// @lat: [[architecture#Datalog frontend]]

use crate::error::Span;
use oxrdf::{NamedNode, Term};

/// How a body or head atom names its relation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Pred {
    /// An IRI applied to arguments. Binary is a predicate — `ex:parent(?x, ?y)` is the triple
    /// pattern `?x ex:parent ?y`. Unary is a class — `ex:Person(?x)` is `?x rdf:type
    /// ex:Person`, which is how RDF says it and how a rule wants to read.
    Edb(NamedNode),
    /// A relation defined by rules.
    Idb(String),
    /// `triple(?s, ?p, ?o)` over the default graph, or `triple(?s, ?p, ?o, ?g)`.
    Triple { graph: bool },
}

impl Pred {
    /// The arity every atom of this relation must have.
    pub fn arity(&self) -> Option<usize> {
        match self {
            Self::Triple { graph: false } => Some(3),
            Self::Triple { graph: true } => Some(4),
            // An IRI atom takes one argument or two, and a derived relation takes whatever
            // its rules give it, so neither has a fixed arity.
            Self::Edb(_) | Self::Idb(_) => None,
        }
    }

    /// Is this relation stored rather than derived?
    pub fn is_extensional(&self) -> bool {
        !matches!(self, Self::Idb(_))
    }
}

impl std::fmt::Display for Pred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Edb(n) => write!(f, "<{}>", n.as_str()),
            Self::Idb(n) => f.write_str(n),
            Self::Triple { graph: false } => f.write_str("triple/3"),
            Self::Triple { graph: true } => f.write_str("triple/4"),
        }
    }
}

/// An argument of an atom.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Var(String),
    /// `_`: a variable that occurs exactly once and is never projected.
    Wildcard,
    Const(Term),
}

/// A relation applied to arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Atom {
    pub pred: Pred,
    pub args: Vec<Arg>,
    pub span: Span,
}

/// Aggregate functions allowed in a rule head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFn {
    Count,
    CountDistinct,
    Sum,
    Min,
    Max,
    Avg,
    Sample,
    GroupConcat,
}

impl AggFn {
    pub fn name(self) -> &'static str {
        match self {
            Self::Count | Self::CountDistinct => "COUNT",
            Self::Sum => "SUM",
            Self::Min => "MIN",
            Self::Max => "MAX",
            Self::Avg => "AVG",
            Self::Sample => "SAMPLE",
            Self::GroupConcat => "GROUP_CONCAT",
        }
    }
}

/// A head argument: a plain term, or an aggregate over a body variable.
#[derive(Debug, Clone, PartialEq)]
pub enum HeadArg {
    Plain(Arg),
    Agg { func: AggFn, var: String },
}

/// A rule head.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    pub pred: Pred,
    pub args: Vec<HeadArg>,
    pub span: Span,
}

impl Head {
    /// Does this head aggregate?
    pub fn aggregates(&self) -> bool {
        self.args.iter().any(|a| matches!(a, HeadArg::Agg { .. }))
    }
}

/// Binary operators in a constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
}

impl BinOp {
    /// Does this operator compare rather than compute?
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge
        )
    }

    /// Does this operator produce a number from numbers?
    pub fn is_arithmetic(self) -> bool {
        matches!(self, Self::Add | Self::Sub | Self::Mul | Self::Div)
    }
}

/// A constraint expression, evaluated over the values bound by the positive body atoms.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Var(String),
    Const(Term),
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Not(Box<Expr>),
    Neg(Box<Expr>),
    /// A SPARQL-style function call, e.g. `REGEX`, `STRLEN`, `ABS`.
    Call {
        name: String,
        args: Vec<Expr>,
    },
}

/// One item in a rule body.
#[derive(Debug, Clone, PartialEq)]
pub enum BodyItem {
    Atom(Atom),
    /// `not p(...)`: only legal against a relation of an earlier stratum.
    Negated(Atom),
    Constraint(Expr),
}

/// A rule: `head :- body.`, or a fact when the body is empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub head: Head,
    pub body: Vec<BodyItem>,
}

impl Rule {
    /// The positive atoms of the body, which are what bind variables.
    pub fn positive(&self) -> impl Iterator<Item = &Atom> {
        self.body.iter().filter_map(|i| match i {
            BodyItem::Atom(a) => Some(a),
            _ => None,
        })
    }

    /// The negated atoms of the body.
    pub fn negated(&self) -> impl Iterator<Item = &Atom> {
        self.body.iter().filter_map(|i| match i {
            BodyItem::Negated(a) => Some(a),
            _ => None,
        })
    }

    /// The constraints of the body.
    pub fn constraints(&self) -> impl Iterator<Item = &Expr> {
        self.body.iter().filter_map(|i| match i {
            BodyItem::Constraint(e) => Some(e),
            _ => None,
        })
    }
}

/// The relation a program is asked to return.
#[derive(Debug, Clone, PartialEq)]
pub struct Goal {
    pub atom: Atom,
    /// Extra constraints applied to the goal's bindings.
    pub constraints: Vec<Expr>,
}

/// A parsed program.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub rules: Vec<Rule>,
    pub goal: Option<Goal>,
}

impl Program {
    /// The rules defining a derived predicate.
    pub fn rules_for<'a>(&'a self, pred: &'a Pred) -> impl Iterator<Item = &'a Rule> {
        self.rules.iter().filter(move |r| &r.head.pred == pred)
    }
}

/// Collects the variables of an expression.
pub fn expr_vars(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Var(v) => {
            if !out.contains(v) {
                out.push(v.clone());
            }
        }
        Expr::Const(_) => {}
        Expr::Binary { left, right, .. } => {
            expr_vars(left, out);
            expr_vars(right, out);
        }
        Expr::Not(e) | Expr::Neg(e) => expr_vars(e, out),
        Expr::Call { args, .. } => {
            for a in args {
                expr_vars(a, out);
            }
        }
    }
}

/// Collects the variables of an atom, in argument order.
pub fn atom_vars(a: &Atom, out: &mut Vec<String>) {
    for arg in &a.args {
        if let Arg::Var(v) = arg {
            if !out.contains(v) {
                out.push(v.clone());
            }
        }
    }
}
