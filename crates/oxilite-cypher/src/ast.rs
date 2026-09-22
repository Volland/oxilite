//! Abstract syntax of the supported openCypher subset.
//!
//! The shapes follow the openCypher 9 grammar; pattern and clause types are kept close to
//! GQL so the AST can grow into it.

/// A query: one or more single queries combined with `UNION` / `UNION ALL`.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub parts: Vec<SingleQuery>,
    /// `true` for `UNION ALL` between `parts[i]` and `parts[i + 1]`.
    pub union_all: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SingleQuery {
    pub clauses: Vec<Clause>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    Match {
        optional: bool,
        patterns: Vec<PatternPart>,
        where_: Option<Expr>,
    },
    Unwind {
        expr: Expr,
        alias: String,
    },
    With(Projection),
    Return(Projection),
    Create(Vec<PatternPart>),
    Merge {
        pattern: PatternPart,
        on_create: Vec<SetItem>,
        on_match: Vec<SetItem>,
    },
    Set(Vec<SetItem>),
    Remove(Vec<RemoveItem>),
    Delete {
        detach: bool,
        exprs: Vec<Expr>,
    },
    Call {
        procedure: String,
        args: Vec<Expr>,
        /// `YIELD` items (procedure column, alias); `None` means every column.
        yields: Option<Vec<(String, Option<String>)>>,
        where_: Option<Expr>,
    },
}

impl Clause {
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            Self::Create(_)
                | Self::Merge { .. }
                | Self::Set(_)
                | Self::Remove(_)
                | Self::Delete { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub distinct: bool,
    /// `*`: every variable in scope.
    pub star: bool,
    pub items: Vec<ProjectionItem>,
    pub order: Vec<(Expr, bool)>,
    pub skip: Option<Expr>,
    pub limit: Option<Expr>,
    /// `WITH … WHERE` (always `None` for `RETURN`).
    pub where_: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionItem {
    pub expr: Expr,
    pub alias: Option<String>,
    /// The item's source text, used as the column name when there is no alias.
    pub text: String,
}

impl ProjectionItem {
    pub fn name(&self) -> String {
        match (&self.alias, &self.expr) {
            (Some(a), _) => a.clone(),
            (None, Expr::Var(v)) => v.clone(),
            _ => self.text.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortest {
    One,
    All,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternPart {
    /// `p = …`
    pub var: Option<String>,
    pub shortest: Option<Shortest>,
    pub element: PatternElement,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternElement {
    pub start: NodePattern,
    pub chain: Vec<(RelPattern, NodePattern)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NodePattern {
    pub var: Option<String>,
    pub labels: Vec<String>,
    pub props: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// `-->`
    Right,
    /// `<--`
    Left,
    /// `--`
    Both,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelPattern {
    pub var: Option<String>,
    pub types: Vec<String>,
    pub props: Option<Expr>,
    pub dir: Direction,
    /// `*min..max`; `None` for a single relationship.
    pub length: Option<(Option<u32>, Option<u32>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetItem {
    /// `n.key = value`
    Property {
        var: String,
        key: String,
        value: Expr,
    },
    /// `n = {map}`
    Replace { var: String, value: Expr },
    /// `n += {map}`
    Merge { var: String, value: Expr },
    /// `n:Label:Other`
    Labels { var: String, labels: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RemoveItem {
    Property { var: String, key: String },
    Labels { var: String, labels: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    Xor,
    And,
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
    Mod,
    Pow,
    StartsWith,
    EndsWith,
    Contains,
    Regex,
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    Plus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantifier {
    All,
    Any,
    None,
    Single,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Param(String),
    Var(String),
    List(Vec<Expr>),
    Map(Vec<(String, Expr)>),
    Prop(Box<Expr>, String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    IsNull(Box<Expr>, bool),
    /// `n:Label` in an expression.
    HasLabels(Box<Expr>, Vec<String>),
    /// A function call; `name` is lower-cased and may be namespaced (`db.labels`).
    Func {
        name: String,
        distinct: bool,
        args: Vec<Expr>,
    },
    CountStar,
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        else_: Option<Box<Expr>>,
    },
    /// `[x IN list WHERE pred | expr]`
    ListComp {
        var: String,
        list: Box<Expr>,
        filter: Option<Box<Expr>>,
        map: Option<Box<Expr>>,
    },
    /// `all(x IN list WHERE pred)` and friends.
    Quantified {
        q: Quantifier,
        var: String,
        list: Box<Expr>,
        pred: Box<Expr>,
    },
    /// `reduce(acc = init, x IN list | expr)`
    Reduce {
        acc: String,
        init: Box<Expr>,
        var: String,
        list: Box<Expr>,
        expr: Box<Expr>,
    },
    Index(Box<Expr>, Box<Expr>),
    Slice(Box<Expr>, Option<Box<Expr>>, Option<Box<Expr>>),
    /// A pattern used as a predicate: `WHERE (a)-[:T]->()`.
    Pattern(Box<PatternElement>),
    /// `EXISTS { MATCH … WHERE … }` or `EXISTS { pattern }`.
    Exists(Vec<PatternPart>, Option<Box<Expr>>),
    /// `[(a)-->(b) WHERE … | expr]`
    PatternComp {
        /// `p = …`
        var: Option<String>,
        pattern: Box<PatternElement>,
        filter: Option<Box<Expr>>,
        map: Box<Expr>,
    },
    /// `n {.name, .age, total: expr}`
    MapProjection {
        var: String,
        items: Vec<MapProjItem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MapProjItem {
    Prop(String),
    AllProps,
    Var(String),
    Entry(String, Expr),
}

impl Expr {
    /// Calls `f` on every sub-expression, depth first (including `self`).
    pub fn walk(&self, f: &mut dyn FnMut(&Expr)) {
        f(self);
        match self {
            Self::List(items) => items.iter().for_each(|e| e.walk(f)),
            Self::Map(items) => items.iter().for_each(|(_, e)| e.walk(f)),
            Self::Prop(e, _) | Self::Unary(_, e) | Self::IsNull(e, _) | Self::HasLabels(e, _) => {
                e.walk(f)
            }
            Self::Binary(_, a, b) | Self::Index(a, b) => {
                a.walk(f);
                b.walk(f);
            }
            Self::Func { args, .. } => args.iter().for_each(|e| e.walk(f)),
            Self::Case {
                operand,
                whens,
                else_,
            } => {
                if let Some(o) = operand {
                    o.walk(f);
                }
                for (w, t) in whens {
                    w.walk(f);
                    t.walk(f);
                }
                if let Some(e) = else_ {
                    e.walk(f);
                }
            }
            Self::ListComp {
                list, filter, map, ..
            } => {
                list.walk(f);
                if let Some(x) = filter {
                    x.walk(f);
                }
                if let Some(x) = map {
                    x.walk(f);
                }
            }
            Self::Quantified { list, pred, .. } => {
                list.walk(f);
                pred.walk(f);
            }
            Self::Reduce {
                init, list, expr, ..
            } => {
                init.walk(f);
                list.walk(f);
                expr.walk(f);
            }
            Self::Slice(a, b, c) => {
                a.walk(f);
                if let Some(b) = b {
                    b.walk(f);
                }
                if let Some(c) = c {
                    c.walk(f);
                }
            }
            Self::PatternComp { filter, map, .. } => {
                if let Some(x) = filter {
                    x.walk(f);
                }
                map.walk(f);
            }
            Self::MapProjection { items, .. } => {
                for i in items {
                    if let MapProjItem::Entry(_, e) = i {
                        e.walk(f);
                    }
                }
            }
            _ => {}
        }
    }

    /// Replaces variables (outside comprehension scopes) with `f`'s result.
    #[allow(clippy::borrowed_box)]
    pub fn map_vars(&self, f: &dyn Fn(&str) -> Option<Expr>) -> Expr {
        let m = |e: &Expr| e.map_vars(f);
        let b = |e: &Box<Expr>| Box::new(e.map_vars(f));
        match self {
            Self::Var(v) => f(v).unwrap_or_else(|| self.clone()),
            Self::List(items) => Self::List(items.iter().map(m).collect()),
            Self::Map(items) => Self::Map(items.iter().map(|(k, e)| (k.clone(), m(e))).collect()),
            Self::Prop(e, k) => Self::Prop(b(e), k.clone()),
            Self::Unary(op, e) => Self::Unary(*op, b(e)),
            Self::Binary(op, x, y) => Self::Binary(*op, b(x), b(y)),
            Self::IsNull(e, n) => Self::IsNull(b(e), *n),
            Self::HasLabels(e, l) => Self::HasLabels(b(e), l.clone()),
            Self::Func {
                name,
                distinct,
                args,
            } => Self::Func {
                name: name.clone(),
                distinct: *distinct,
                args: args.iter().map(m).collect(),
            },
            Self::Case {
                operand,
                whens,
                else_,
            } => Self::Case {
                operand: operand.as_ref().map(b),
                whens: whens.iter().map(|(w, t)| (m(w), m(t))).collect(),
                else_: else_.as_ref().map(b),
            },
            Self::Index(x, y) => Self::Index(b(x), b(y)),
            Self::Slice(x, y, z) => Self::Slice(b(x), y.as_ref().map(b), z.as_ref().map(b)),
            other => other.clone(),
        }
    }

    /// Replaces sub-expressions equal to one of `targets[i].0` with `targets[i].1`.
    #[allow(clippy::borrowed_box)]
    pub fn replace_subexprs(&self, targets: &[(Expr, Expr)]) -> Expr {
        if let Some((_, r)) = targets.iter().find(|(t, _)| t == self) {
            return r.clone();
        }
        let m = |e: &Expr| e.replace_subexprs(targets);
        let b = |e: &Box<Expr>| Box::new(e.replace_subexprs(targets));
        match self {
            Self::List(items) => Self::List(items.iter().map(m).collect()),
            Self::Map(items) => Self::Map(items.iter().map(|(k, e)| (k.clone(), m(e))).collect()),
            Self::Prop(e, k) => Self::Prop(b(e), k.clone()),
            Self::Unary(op, e) => Self::Unary(*op, b(e)),
            Self::Binary(op, x, y) => Self::Binary(*op, b(x), b(y)),
            Self::IsNull(e, n) => Self::IsNull(b(e), *n),
            Self::Func {
                name,
                distinct,
                args,
            } => Self::Func {
                name: name.clone(),
                distinct: *distinct,
                args: args.iter().map(m).collect(),
            },
            Self::Case {
                operand,
                whens,
                else_,
            } => Self::Case {
                operand: operand.as_ref().map(b),
                whens: whens.iter().map(|(w, t)| (m(w), m(t))).collect(),
                else_: else_.as_ref().map(b),
            },
            Self::Index(x, y) => Self::Index(b(x), b(y)),
            other => other.clone(),
        }
    }

    /// Whether the expression contains an aggregate call.
    pub fn has_aggregate(&self) -> bool {
        let mut found = false;
        self.walk(&mut |e| {
            if let Self::Func { name, .. } = e {
                if is_aggregate(name) {
                    found = true;
                }
            }
            if matches!(e, Self::CountStar) {
                found = true;
            }
        });
        found
    }
}

pub fn is_aggregate(name: &str) -> bool {
    matches!(
        name,
        "count"
            | "sum"
            | "avg"
            | "min"
            | "max"
            | "collect"
            | "stdev"
            | "stdevp"
            | "percentilecont"
            | "percentiledisc"
    )
}
