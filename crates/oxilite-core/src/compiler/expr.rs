//! SPARQL expressions → SQL.
//!
//! Every SPARQL value is represented by a [`V`]: a bundle of SQL fragments giving its kind,
//! lexical form, datatype, language, numeric value, timestamp and boolean value. For a stored
//! term these fragments are derived from its id (inline values decode arithmetically, hashed
//! ones through correlated `terms` lookups that SQLite evaluates as early as possible). SQL
//! `NULL` plays the role of the SPARQL error / unbound value; SQL three-valued logic then
//! matches SPARQL's error semantics for `&&`, `||` and `!`.
//!
// @lat: [[architecture#SPARQL to SQL compiler#Expressions]]

use super::{Binding, Block, Col, Compiler};
use crate::encoding::{
    encode_literal, numeric_rank, numeric_type, Tag, INT_OFFSET, PAYLOAD_BITS, PAYLOAD_MASK,
};
use crate::error::{Error, Result};
use crate::sql::{sql_f64, sql_str};
use oxrdf::vocab::{rdf, xsd};
use oxrdf::{Literal, NamedNode, Term, Variable};
use spargebra::algebra::{Expression, Function};
use std::collections::BTreeMap;

/// What is statically known about a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stat {
    Any,
    Numeric,
    String,
    LangString,
    Bool,
    DateTime,
    Iri,
}

/// A SPARQL value as SQL fragments.
#[derive(Debug, Clone)]
pub(crate) struct V {
    /// Term id, when it is defined whenever the value is.
    pub id: Option<String>,
    pub kind: String,
    pub lex: String,
    pub dt: String,
    pub lang: String,
    pub num: String,
    pub nt: String,
    pub ts: String,
    pub boolv: String,
    pub stat: Stat,
    /// `lex` is only an approximation of the canonical form (computed numbers).
    pub computed_num: bool,
}

pub(crate) const K_IRI: i64 = Tag::Iri as i64;
pub(crate) const K_BNODE: i64 = Tag::BlankNode as i64;
pub(crate) const K_STRING: i64 = Tag::String as i64;
pub(crate) const K_LANG: i64 = Tag::LangString as i64;
pub(crate) const K_TYPED: i64 = Tag::Typed as i64;
pub(crate) const K_INT: i64 = Tag::Integer as i64;
pub(crate) const K_BOOL: i64 = Tag::Boolean as i64;
pub(crate) const K_TRIPLE: i64 = Tag::Triple as i64;
pub(crate) const K_DIRLANG: i64 = Tag::DirLangString as i64;

/// `id` of the inline integer `x` is `INT_BASE + x`.
pub(crate) const INT_BASE: i64 = Tag::Integer.base() + INT_OFFSET;
pub(crate) const BOOL_BASE: i64 = Tag::Boolean.base();

fn xsd_str(n: oxrdf::NamedNodeRef<'_>) -> String {
    sql_str(n.as_str())
}

impl V {
    pub(crate) fn null() -> Self {
        Self {
            id: Some("NULL".into()),
            kind: "NULL".into(),
            lex: "NULL".into(),
            dt: "NULL".into(),
            lang: "NULL".into(),
            num: "NULL".into(),
            nt: "NULL".into(),
            ts: "NULL".into(),
            boolv: "NULL".into(),
            stat: Stat::Any,
            computed_num: false,
        }
    }

    /// Value of a stored term given the SQL expression of its id.
    pub(crate) fn from_id(x: &str) -> Self {
        let k = format!("(({x}) >> {PAYLOAD_BITS})");
        let payload = format!("(({x}) & {PAYLOAD_MASK})");
        Self {
            id: Some(x.into()),
            lex: format!(
                "CASE {k} WHEN {K_INT} THEN CAST({payload} - {INT_OFFSET} AS TEXT) WHEN {K_BOOL} THEN CASE {payload} WHEN 1 THEN 'true' ELSE 'false' END ELSE (SELECT lex FROM terms WHERE id = {x}) END"
            ),
            dt: format!(
                "CASE {k} WHEN {K_STRING} THEN {} WHEN {K_LANG} THEN {} WHEN {K_DIRLANG} THEN {} WHEN {K_INT} THEN {} WHEN {K_BOOL} THEN {} WHEN {K_TYPED} THEN (SELECT dt FROM terms WHERE id = {x}) END",
                xsd_str(xsd::STRING),
                xsd_str(rdf::LANG_STRING),
                sql_str("http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString"),
                xsd_str(xsd::INTEGER),
                xsd_str(xsd::BOOLEAN),
            ),
            lang: format!(
                "CASE WHEN {k} IN ({K_LANG}, {K_DIRLANG}) THEN (SELECT lang FROM terms WHERE id = {x}) END"
            ),
            num: format!(
                "CASE {k} WHEN {K_INT} THEN {payload} - {INT_OFFSET} WHEN {K_TYPED} THEN (SELECT num FROM terms WHERE id = {x} AND nt IS NOT NULL) END"
            ),
            nt: format!(
                "CASE {k} WHEN {K_INT} THEN 1 WHEN {K_TYPED} THEN (SELECT nt FROM terms WHERE id = {x}) END"
            ),
            ts: format!("CASE WHEN {k} = {K_TYPED} THEN (SELECT ts FROM terms WHERE id = {x}) END"),
            boolv: format!(
                "CASE {k} WHEN {K_BOOL} THEN {payload} WHEN {K_TYPED} THEN (SELECT num FROM terms WHERE id = {x} AND dt = {}) END",
                xsd_str(xsd::BOOLEAN)
            ),
            kind: k,
            stat: Stat::Any,
            computed_num: false,
        }
    }

    /// Value of a constant term.
    pub(crate) fn from_term(term: &Term, id: i64) -> Result<Self> {
        let mut v = Self::null();
        v.id = Some(id.to_string());
        match term {
            Term::NamedNode(n) => {
                v.kind = K_IRI.to_string();
                v.lex = sql_str(n.as_str());
                v.stat = Stat::Iri;
            }
            Term::BlankNode(b) => {
                v.kind = K_BNODE.to_string();
                v.lex = sql_str(b.as_str());
            }
            Term::Literal(l) => {
                let (lid, row) = encode_literal(l.as_ref());
                debug_assert_eq!(lid, id);
                let tag = crate::encoding::tag_of(id).unwrap_or(Tag::Typed);
                v.kind = (tag as i64).to_string();
                v.lex = sql_str(l.value());
                v.dt = sql_str(l.datatype().as_str());
                v.lang = l.language().map_or_else(|| "NULL".into(), sql_str);
                match tag {
                    Tag::Integer => {
                        v.num = l.value().to_string();
                        v.nt = "1".into();
                        v.stat = Stat::Numeric;
                    }
                    Tag::Boolean => {
                        v.boolv = if l.value() == "true" { "1" } else { "0" }.into();
                        v.stat = Stat::Bool;
                    }
                    Tag::String => v.stat = Stat::String,
                    Tag::LangString | Tag::DirLangString => v.stat = Stat::LangString,
                    _ => {
                        if let Some(row) = row {
                            if let (Some(num), Some(nt)) = (row.num, row.nt) {
                                v.num = sql_f64(num);
                                v.nt = nt.to_string();
                                v.stat = Stat::Numeric;
                            } else if l.datatype() == xsd::BOOLEAN {
                                v.boolv =
                                    row.num.map_or_else(|| "NULL".into(), |b| (b as i64).to_string());
                                v.stat = Stat::Bool;
                            }
                            if let Some(ts) = row.ts {
                                v.ts = sql_f64(ts);
                                v.stat = Stat::DateTime;
                            }
                        }
                    }
                }
            }
            Term::Triple(_) => {
                v.kind = K_TRIPLE.to_string();
            }
        }
        Ok(v)
    }

    /// A computed simple (or language-tagged, following `like`) string.
    fn string(lex: String, like: Option<&V>) -> Self {
        let mut v = Self::null();
        v.id = None;
        match like {
            Some(l) if l.stat != Stat::String => {
                v.kind = format!(
                    "CASE WHEN ({lex}) IS NOT NULL THEN CASE WHEN {} IN ({K_LANG}, {K_DIRLANG}) THEN {K_LANG} ELSE {K_STRING} END END",
                    l.kind
                );
                v.lang = format!("CASE WHEN {} IN ({K_LANG}, {K_DIRLANG}) THEN {} END", l.kind, l.lang);
                v.dt = format!(
                    "CASE WHEN {} IN ({K_LANG}, {K_DIRLANG}) THEN {} ELSE {} END",
                    l.kind,
                    xsd_str(rdf::LANG_STRING),
                    xsd_str(xsd::STRING)
                );
                v.stat = Stat::Any;
            }
            _ => {
                v.kind = format!("CASE WHEN ({lex}) IS NOT NULL THEN {K_STRING} END");
                v.dt = xsd_str(xsd::STRING);
                v.stat = Stat::String;
            }
        }
        v.lex = lex;
        v
    }

    /// A computed numeric value.
    pub(crate) fn numeric(num: String, nt: String) -> Self {
        let mut v = Self::null();
        v.id = None;
        v.kind = format!(
            "CASE WHEN ({num}) IS NULL THEN NULL WHEN ({nt}) = 1 THEN {K_INT} WHEN ({nt}) IS NOT NULL THEN {K_TYPED} END"
        );
        v.dt = format!(
            "CASE ({nt}) WHEN 1 THEN {} WHEN 2 THEN {} WHEN 3 THEN {} WHEN 4 THEN {} END",
            xsd_str(xsd::INTEGER),
            xsd_str(xsd::DECIMAL),
            xsd_str(xsd::FLOAT),
            xsd_str(xsd::DOUBLE)
        );
        v.lex = format!(
            "CASE WHEN ({nt}) = 1 THEN CAST(CAST({num} AS INTEGER) AS TEXT) ELSE CAST({num} AS TEXT) END"
        );
        v.num = num;
        v.nt = nt;
        v.stat = Stat::Numeric;
        v.computed_num = true;
        v
    }

    /// A computed integer (always `xsd:integer`).
    pub(crate) fn integer(num: String) -> Self {
        let mut v = Self::numeric(num.clone(), "1".into());
        v.id = Some(format!("({INT_BASE} + ({num}))"));
        v.kind = format!("CASE WHEN ({num}) IS NOT NULL THEN {K_INT} END");
        v.dt = xsd_str(xsd::INTEGER);
        v
    }

    /// A computed boolean.
    pub(crate) fn boolean(b: &str) -> Self {
        let mut v = Self::null();
        v.id = Some(format!("({BOOL_BASE} + ({b}))"));
        v.kind = format!("CASE WHEN ({b}) IS NOT NULL THEN {K_BOOL} END");
        v.lex = format!("CASE ({b}) WHEN 1 THEN 'true' WHEN 0 THEN 'false' END");
        v.dt = xsd_str(xsd::BOOLEAN);
        v.boolv = format!("({b})");
        v.stat = Stat::Bool;
        v
    }

    /// A computed IRI.
    fn iri(lex: String) -> Self {
        let mut v = Self::null();
        v.id = None;
        v.kind = format!("CASE WHEN ({lex}) IS NOT NULL THEN {K_IRI} END");
        v.lex = lex;
        v.stat = Stat::Iri;
        v
    }

    /// A computed typed literal with a statically known datatype.
    fn typed(lex: String, dt: &str) -> Self {
        let mut v = Self::null();
        v.id = None;
        v.kind = format!("CASE WHEN ({lex}) IS NOT NULL THEN {K_TYPED} END");
        v.dt = sql_str(dt);
        v.lex = lex;
        v
    }

    fn is_string_like(&self) -> String {
        match self.stat {
            Stat::String | Stat::LangString => "1".into(),
            _ => format!("{} IN ({K_STRING}, {K_LANG}, {K_DIRLANG})", self.kind),
        }
    }

    /// Effective boolean value.
    pub(crate) fn ebv(&self) -> String {
        match self.stat {
            Stat::Bool => format!("({})", self.boolv),
            Stat::Numeric => format!("(({}) <> 0)", self.num),
            Stat::String => format!("(({}) <> '')", self.lex),
            Stat::Iri | Stat::LangString | Stat::DateTime => "NULL".into(),
            Stat::Any => format!(
                "(CASE WHEN ({b}) IS NOT NULL THEN ({b}) WHEN ({n}) IS NOT NULL THEN (({n}) <> 0) WHEN {k} = {K_STRING} THEN (({l}) <> '') END)",
                b = self.boolv,
                n = self.num,
                k = self.kind,
                l = self.lex
            ),
        }
    }
}

/// A compiled expression: a SQL boolean (0/1/NULL) or a value.
pub(crate) enum E {
    B(String),
    T(V),
}

impl E {
    pub(crate) fn bool_sql(self) -> String {
        match self {
            Self::B(b) => b,
            Self::T(v) => v.ebv(),
        }
    }

    pub(crate) fn term(self) -> V {
        match self {
            Self::B(b) => V::boolean(&b),
            Self::T(v) => v,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    fn op(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

fn may(s: Stat, want: Stat) -> bool {
    s == Stat::Any || s == want
}

fn same_term(a: &V, b: &V) -> String {
    match (&a.id, &b.id) {
        (Some(x), Some(y)) => format!("(({x}) = ({y}))"),
        _ => format!(
            "(({ak}) = ({bk}) AND ({al}) = ({bl}) AND ({ad}) IS ({bd}) AND ({ag}) IS ({bg}))",
            ak = a.kind,
            bk = b.kind,
            al = a.lex,
            bl = b.lex,
            ad = a.dt,
            bd = b.dt,
            ag = a.lang,
            bg = b.lang
        ),
    }
}

fn compare(a: &V, b: &V, cmp: Cmp) -> String {
    let op = cmp.op();
    let mut branches = Vec::new();
    if cmp == Cmp::Eq {
        if let (Some(x), Some(y)) = (&a.id, &b.id) {
            branches.push(format!("WHEN ({x}) = ({y}) THEN 1"));
        }
    }
    if may(a.stat, Stat::Numeric) && may(b.stat, Stat::Numeric) {
        if a.stat == Stat::Numeric && b.stat == Stat::Numeric {
            return format!("(({}) {op} ({}))", a.num, b.num);
        }
        branches.push(format!(
            "WHEN ({an}) IS NOT NULL AND ({bn}) IS NOT NULL THEN ({an}) {op} ({bn})",
            an = a.num,
            bn = b.num
        ));
    }
    if may(a.stat, Stat::String) && may(b.stat, Stat::String) {
        if a.stat == Stat::String && b.stat == Stat::String {
            return format!("(({}) {op} ({}))", a.lex, b.lex);
        }
        branches.push(format!(
            "WHEN ({ak}) = {K_STRING} AND ({bk}) = {K_STRING} THEN ({al}) {op} ({bl})",
            ak = a.kind,
            bk = b.kind,
            al = a.lex,
            bl = b.lex
        ));
    }
    if may(a.stat, Stat::DateTime) && may(b.stat, Stat::DateTime) {
        branches.push(format!(
            "WHEN ({at}) IS NOT NULL AND ({bt}) IS NOT NULL THEN ({at}) {op} ({bt})",
            at = a.ts,
            bt = b.ts
        ));
    }
    if may(a.stat, Stat::Bool) && may(b.stat, Stat::Bool) {
        branches.push(format!(
            "WHEN ({ab}) IS NOT NULL AND ({bb}) IS NOT NULL THEN ({ab}) {op} ({bb})",
            ab = a.boolv,
            bb = b.boolv
        ));
    }
    if cmp == Cmp::Eq {
        if a.id.is_none() || b.id.is_none() {
            branches.push(format!("WHEN {} THEN 1", same_term(a, b)));
        }
        if may(a.stat, Stat::LangString) && may(b.stat, Stat::LangString) {
            branches.push(format!(
                "WHEN ({ak}) = {K_LANG} AND ({bk}) = {K_LANG} THEN (({al}) = ({bl}) AND ({ag}) = ({bg}))",
                ak = a.kind,
                bk = b.kind,
                al = a.lex,
                bl = b.lex,
                ag = a.lang,
                bg = b.lang
            ));
        }
        // IRIs, blank nodes and triple terms are only equal to themselves.
        branches.push(format!(
            "WHEN ({}) IN ({K_IRI}, {K_BNODE}, {K_TRIPLE}) OR ({}) IN ({K_IRI}, {K_BNODE}, {K_TRIPLE}) THEN 0",
            a.kind, b.kind
        ));
    }
    if branches.is_empty() {
        return "NULL".into();
    }
    format!("(CASE {} END)", branches.join(" "))
}

fn arith(a: &V, b: &V, op: char) -> V {
    let nt = if op == '/' {
        format!("max(2, max({}, {}))", a.nt, b.nt)
    } else {
        format!("max({}, {})", a.nt, b.nt)
    };
    let num = match op {
        '/' => format!(
            "(CASE WHEN ({bn}) = 0 AND max({ant}, {bnt}) < 3 THEN NULL ELSE CAST({an} AS REAL) / ({bn}) END)",
            an = a.num,
            bn = b.num,
            ant = a.nt,
            bnt = b.nt
        ),
        _ => format!("(({}) {op} ({}))", a.num, b.num),
    };
    V::numeric(num, nt)
}

/// Is a regex made only of literal characters (plus optional `^` / `$` anchors)?
fn simple_regex(pattern: &str) -> Option<(bool, String, bool)> {
    let mut chars = pattern.chars().peekable();
    let starts = chars.peek() == Some(&'^');
    if starts {
        chars.next();
    }
    let mut lit = String::new();
    let mut ends = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(e) if ".*+?()[]{}|\\^$/-".contains(e) => lit.push(e),
                _ => return None,
            },
            '$' if chars.peek().is_none() => ends = true,
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' => {
                return None
            }
            c => lit.push(c),
        }
    }
    Some((starts, lit, ends))
}

impl Compiler<'_> {
    /// Resolves a variable against a block's bindings and the outer (EXISTS) scopes.
    pub(crate) fn var_value(&mut self, v: &Variable, cols: &BTreeMap<usize, Binding>) -> V {
        let idx = self.var(v);
        if let Some(b) = cols.get(&idx) {
            return b.col.value();
        }
        for scope in self.outer.iter().rev() {
            if let Some(b) = scope.get(&idx) {
                return b.col.value();
            }
        }
        V::null()
    }

    fn constant(&mut self, t: Term) -> Result<V> {
        let id = self.constant_id(&t)?;
        V::from_term(&t, id)
    }

    pub(crate) fn expr_term(&mut self, e: &Expression, cols: &BTreeMap<usize, Binding>) -> Result<V> {
        Ok(self.expr(e, cols)?.term())
    }

    pub(crate) fn expr_bool(&mut self, e: &Expression, cols: &BTreeMap<usize, Binding>) -> Result<String> {
        Ok(self.expr(e, cols)?.bool_sql())
    }

    pub(crate) fn expr(&mut self, e: &Expression, cols: &BTreeMap<usize, Binding>) -> Result<E> {
        Ok(match e {
            Expression::NamedNode(n) => E::T(self.constant(n.clone().into())?),
            Expression::Literal(l) => E::T(self.constant(l.clone().into())?),
            Expression::Variable(v) => E::T(self.var_value(v, cols)),
            Expression::Or(a, b) => E::B(format!(
                "({} OR {})",
                self.expr_bool(a, cols)?,
                self.expr_bool(b, cols)?
            )),
            Expression::And(a, b) => E::B(format!(
                "({} AND {})",
                self.expr_bool(a, cols)?,
                self.expr_bool(b, cols)?
            )),
            Expression::Not(a) => E::B(format!("(NOT {})", self.expr_bool(a, cols)?)),
            Expression::Equal(a, b) => {
                let (a, b) = (self.expr_term(a, cols)?, self.expr_term(b, cols)?);
                E::B(compare(&a, &b, Cmp::Eq))
            }
            Expression::SameTerm(a, b) => {
                let (a, b) = (self.expr_term(a, cols)?, self.expr_term(b, cols)?);
                E::B(same_term(&a, &b))
            }
            Expression::Greater(a, b) => self.cmp(a, b, Cmp::Gt, cols)?,
            Expression::GreaterOrEqual(a, b) => self.cmp(a, b, Cmp::Ge, cols)?,
            Expression::Less(a, b) => self.cmp(a, b, Cmp::Lt, cols)?,
            Expression::LessOrEqual(a, b) => self.cmp(a, b, Cmp::Le, cols)?,
            Expression::In(a, list) => {
                if list.is_empty() {
                    return Ok(E::B("0".into()));
                }
                let a = self.expr_term(a, cols)?;
                let mut parts = Vec::new();
                for b in list {
                    let b = self.expr_term(b, cols)?;
                    parts.push(compare(&a, &b, Cmp::Eq));
                }
                E::B(format!("({})", parts.join(" OR ")))
            }
            Expression::Add(a, b) => self.arith(a, b, '+', cols)?,
            Expression::Subtract(a, b) => self.arith(a, b, '-', cols)?,
            Expression::Multiply(a, b) => self.arith(a, b, '*', cols)?,
            Expression::Divide(a, b) => self.arith(a, b, '/', cols)?,
            Expression::UnaryPlus(a) => {
                let a = self.expr_term(a, cols)?;
                E::T(V::numeric(a.num.clone(), a.nt.clone()))
            }
            Expression::UnaryMinus(a) => {
                let a = self.expr_term(a, cols)?;
                E::T(V::numeric(format!("(-({}))", a.num), a.nt.clone()))
            }
            Expression::Bound(v) => {
                let v = self.var_value(v, cols);
                E::B(format!("(({}) IS NOT NULL)", v.kind))
            }
            Expression::If(c, a, b) => {
                let c = self.expr_bool(c, cols)?;
                let a = self.expr_term(a, cols)?;
                let b = self.expr_term(b, cols)?;
                E::T(Self::choose(&c, &a, &b))
            }
            Expression::Coalesce(list) => {
                let mut vals = Vec::new();
                for e in list {
                    vals.push(self.expr_term(e, cols)?);
                }
                let Some(mut acc) = vals.pop() else {
                    return Ok(E::T(V::null()));
                };
                while let Some(v) = vals.pop() {
                    let c = format!("(({}) IS NOT NULL)", v.kind);
                    acc = Self::choose(&c, &v, &acc);
                }
                E::T(acc)
            }
            Expression::Exists(p) => E::B(self.exists(p, cols)?),
            Expression::FunctionCall(f, args) => self.function(f, args, cols)?,
        })
    }

    fn choose(c: &str, a: &V, b: &V) -> V {
        let pick = |x: &str, y: &str| format!("(CASE WHEN ({c}) IS NULL THEN NULL WHEN ({c}) THEN {x} ELSE {y} END)");
        V {
            id: match (&a.id, &b.id) {
                (Some(x), Some(y)) => Some(pick(x, y)),
                _ => None,
            },
            kind: pick(&a.kind, &b.kind),
            lex: pick(&a.lex, &b.lex),
            dt: pick(&a.dt, &b.dt),
            lang: pick(&a.lang, &b.lang),
            num: pick(&a.num, &b.num),
            nt: pick(&a.nt, &b.nt),
            ts: pick(&a.ts, &b.ts),
            boolv: pick(&a.boolv, &b.boolv),
            stat: if a.stat == b.stat { a.stat } else { Stat::Any },
            computed_num: a.computed_num || b.computed_num,
        }
    }

    fn cmp(&mut self, a: &Expression, b: &Expression, c: Cmp, cols: &BTreeMap<usize, Binding>) -> Result<E> {
        let (a, b) = (self.expr_term(a, cols)?, self.expr_term(b, cols)?);
        Ok(E::B(compare(&a, &b, c)))
    }

    fn arith(&mut self, a: &Expression, b: &Expression, op: char, cols: &BTreeMap<usize, Binding>) -> Result<E> {
        let (a, b) = (self.expr_term(a, cols)?, self.expr_term(b, cols)?);
        Ok(E::T(arith(&a, &b, op)))
    }

    fn exists(&mut self, p: &spargebra::algebra::GraphPattern, cols: &BTreeMap<usize, Binding>) -> Result<String> {
        self.outer.push(cols.clone());
        let inner = self.pattern(p);
        self.outer.pop();
        let mut inner = inner?;
        if !inner.is_plain() || inner.from.is_empty() {
            inner = self.seal(inner);
        }
        // Variables bound on both sides must agree (substitution semantics).
        let mut conds = inner.wheres.clone();
        for (idx, b) in &inner.cols {
            if let Some(outer) = cols.get(idx) {
                let (Col::Id(i), Col::Id(o)) = (&b.col, &outer.col) else {
                    return Err(Error::unsupported("EXISTS over computed values"));
                };
                if b.correlated {
                    continue;
                }
                conds.push(if outer.nullable || b.nullable {
                    format!("(({o}) IS NULL OR ({i}) IS NULL OR ({i}) = ({o}))")
                } else {
                    format!("({i}) = ({o})")
                });
            }
        }
        Ok(format!(
            "EXISTS (SELECT 1 FROM {}{})",
            Block::render_from(&inner.from),
            if conds.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", conds.join(" AND "))
            }
        ))
    }

    fn args(&mut self, args: &[Expression], cols: &BTreeMap<usize, Binding>) -> Result<Vec<V>> {
        args.iter().map(|a| self.expr_term(a, cols)).collect()
    }

    fn function(&mut self, f: &Function, args: &[Expression], cols: &BTreeMap<usize, Binding>) -> Result<E> {
        let udf = self.caps.udf;
        Ok(match f {
            Function::Str => {
                let a = self.expr_term(&args[0], cols)?;
                let lex = format!(
                    "CASE WHEN ({}) IN ({K_IRI}, {K_STRING}, {K_LANG}, {K_TYPED}, {K_INT}, {K_BOOL}, {K_DIRLANG}) THEN {} END",
                    a.kind, a.lex
                );
                if a.computed_num {
                    // String form of a computed number is only approximately canonical.
                    return Err(Error::unsupported("STR() of a computed number"));
                }
                E::T(V::string(lex, None))
            }
            Function::Lang => {
                let a = self.expr_term(&args[0], cols)?;
                E::T(V::string(
                    format!(
                        "CASE WHEN ({k}) IN ({K_LANG}, {K_DIRLANG}) THEN ({l}) WHEN ({k}) IN ({K_STRING}, {K_TYPED}, {K_INT}, {K_BOOL}) THEN '' END",
                        k = a.kind,
                        l = a.lang
                    ),
                    None,
                ))
            }
            Function::Datatype => {
                let a = self.expr_term(&args[0], cols)?;
                E::T(V::iri(a.dt.clone()))
            }
            Function::IsIri => {
                let a = self.expr_term(&args[0], cols)?;
                E::B(format!("(({}) = {K_IRI})", a.kind))
            }
            Function::IsBlank => {
                let a = self.expr_term(&args[0], cols)?;
                E::B(format!("(({}) = {K_BNODE})", a.kind))
            }
            Function::IsLiteral => {
                let a = self.expr_term(&args[0], cols)?;
                E::B(format!(
                    "(({}) IN ({K_STRING}, {K_LANG}, {K_TYPED}, {K_INT}, {K_BOOL}, {K_DIRLANG}))",
                    a.kind
                ))
            }
            Function::IsNumeric => {
                let a = self.expr_term(&args[0], cols)?;
                E::B(format!(
                    "(CASE WHEN ({}) IS NULL THEN NULL ELSE ({}) IS NOT NULL END)",
                    a.kind, a.nt
                ))
            }
            Function::IsTriple => {
                let a = self.expr_term(&args[0], cols)?;
                E::B(format!("(({}) = {K_TRIPLE})", a.kind))
            }
            Function::StrLen => {
                let a = self.expr_term(&args[0], cols)?;
                E::T(V::integer(format!(
                    "CASE WHEN {} THEN length({}) END",
                    a.is_string_like(),
                    a.lex
                )))
            }
            Function::UCase | Function::LCase => {
                let a = self.expr_term(&args[0], cols)?;
                let func = if matches!(f, Function::UCase) { "upper" } else { "lower" };
                E::T(V::string(
                    format!("CASE WHEN {} THEN {func}({}) END", a.is_string_like(), a.lex),
                    Some(&a),
                ))
            }
            Function::SubStr => {
                let a = self.args(args, cols)?;
                let start = format!("CAST(round({}) AS INTEGER)", a[1].num);
                let lex = if let Some(len) = a.get(2) {
                    format!(
                        "CASE WHEN {} THEN substr({}, {start}, CAST(round({}) AS INTEGER)) END",
                        a[0].is_string_like(),
                        a[0].lex,
                        len.num
                    )
                } else {
                    format!("CASE WHEN {} THEN substr({}, {start}) END", a[0].is_string_like(), a[0].lex)
                };
                E::T(V::string(lex, Some(&a[0])))
            }
            Function::Concat => {
                let a = self.args(args, cols)?;
                if a.is_empty() {
                    E::T(V::string("''".into(), None))
                } else {
                    let parts: Vec<String> = a
                        .iter()
                        .map(|v| format!("(CASE WHEN {} THEN {} END)", v.is_string_like(), v.lex))
                        .collect();
                    E::T(V::string(format!("({})", parts.join(" || ")), None))
                }
            }
            Function::Contains | Function::StrStarts | Function::StrEnds => {
                let a = self.args(args, cols)?;
                let (x, y) = (&a[0].lex, &a[1].lex);
                let test = match f {
                    Function::Contains => format!("instr({x}, {y}) > 0"),
                    Function::StrStarts => format!("substr({x}, 1, length({y})) = {y}"),
                    _ => format!("(length({y}) = 0 OR substr({x}, -length({y})) = {y})"),
                };
                E::B(format!(
                    "(CASE WHEN {} AND {} THEN {test} END)",
                    a[0].is_string_like(),
                    a[1].is_string_like()
                ))
            }
            Function::StrBefore | Function::StrAfter => {
                let a = self.args(args, cols)?;
                let (x, y) = (&a[0].lex, &a[1].lex);
                let lex = if matches!(f, Function::StrBefore) {
                    format!("CASE WHEN instr({x}, {y}) > 0 THEN substr({x}, 1, instr({x}, {y}) - 1) ELSE '' END")
                } else {
                    format!("CASE WHEN instr({x}, {y}) > 0 THEN substr({x}, instr({x}, {y}) + length({y})) ELSE '' END")
                };
                E::T(V::string(
                    format!(
                        "CASE WHEN {} AND {} THEN {lex} END",
                        a[0].is_string_like(),
                        a[1].is_string_like()
                    ),
                    Some(&a[0]),
                ))
            }
            Function::LangMatches => {
                let a = self.args(args, cols)?;
                let (tag, range) = (&a[0].lex, &a[1].lex);
                E::B(format!(
                    "(CASE WHEN ({range}) = '*' THEN ({tag}) <> '' ELSE lower({tag}) = lower({range}) OR lower({tag}) LIKE lower({range}) || '-%' END)"
                ))
            }
            Function::Regex => self.regex(args, cols)?,
            Function::Replace if udf => {
                let a = self.args(args, cols)?;
                let flags = a.get(3).map_or_else(|| "''".into(), |f| f.lex.clone());
                E::T(V::string(
                    format!(
                        "CASE WHEN {} THEN oxilite_replace({}, {}, {}, {flags}) END",
                        a[0].is_string_like(),
                        a[0].lex,
                        a[1].lex,
                        a[2].lex
                    ),
                    Some(&a[0]),
                ))
            }
            Function::EncodeForUri if udf => {
                let a = self.expr_term(&args[0], cols)?;
                E::T(V::string(format!("oxilite_encode_for_uri({})", a.lex), None))
            }
            Function::Md5 | Function::Sha1 | Function::Sha256 | Function::Sha384 | Function::Sha512
                if udf =>
            {
                let a = self.expr_term(&args[0], cols)?;
                let algo = match f {
                    Function::Md5 => "md5",
                    Function::Sha1 => "sha1",
                    Function::Sha256 => "sha256",
                    Function::Sha384 => "sha384",
                    _ => "sha512",
                };
                E::T(V::string(
                    format!(
                        "CASE WHEN ({}) = {K_STRING} THEN oxilite_hash('{algo}', {}) END",
                        a.kind, a.lex
                    ),
                    None,
                ))
            }
            Function::Abs | Function::Ceil | Function::Floor | Function::Round => {
                let a = self.expr_term(&args[0], cols)?;
                let n = &a.num;
                let floor = format!("(CAST({n} AS INTEGER) - (({n}) < CAST({n} AS INTEGER)))");
                let num = match f {
                    Function::Abs => format!("abs({n})"),
                    Function::Floor => floor,
                    Function::Ceil => format!("(-(CAST(-({n}) AS INTEGER) - ((-({n})) < CAST(-({n}) AS INTEGER))))"),
                    _ => {
                        let m = format!("(({n}) + 0.5)");
                        format!("(CAST({m} AS INTEGER) - (({m}) < CAST({m} AS INTEGER)))")
                    }
                };
                E::T(V::numeric(num, a.nt.clone()))
            }
            Function::Year | Function::Month | Function::Day | Function::Hours | Function::Minutes => {
                let a = self.expr_term(&args[0], cols)?;
                let (start, len) = match f {
                    Function::Year => (1, 4),
                    Function::Month => (6, 2),
                    Function::Day => (9, 2),
                    Function::Hours => (12, 2),
                    _ => (15, 2),
                };
                let neg = if matches!(f, Function::Year) {
                    // Negative years: skip the sign.
                    format!(
                        "CASE WHEN substr({l}, 1, 1) = '-' THEN -CAST(substr({l}, 2, 4) AS INTEGER) ELSE CAST(substr({l}, 1, 4) AS INTEGER) END",
                        l = a.lex
                    )
                } else {
                    format!("CAST(substr({}, {start}, {len}) AS INTEGER)", a.lex)
                };
                E::T(V::integer(format!(
                    "CASE WHEN ({}) IS NOT NULL THEN {neg} END",
                    a.ts
                )))
            }
            Function::Seconds => {
                let a = self.expr_term(&args[0], cols)?;
                // Lexical seconds are at offset 18, followed by an optional fraction and timezone.
                E::T(V::numeric(
                    format!(
                        "CASE WHEN ({}) IS NOT NULL AND length({l}) >= 19 THEN CAST(rtrim(substr({l}, 18), 'Z+-:0123456789') AS REAL) + CAST(substr({l}, 18, 2) AS REAL) * 0 END",
                        a.ts,
                        l = a.lex
                    ),
                    "2".into(),
                ))
            }
            Function::Tz => {
                let a = self.expr_term(&args[0], cols)?;
                let l = &a.lex;
                E::T(V::string(
                    format!(
                        "CASE WHEN ({}) IS NULL THEN NULL WHEN substr({l}, -1) = 'Z' THEN 'Z' WHEN substr({l}, -6, 1) IN ('+', '-') AND substr({l}, -3, 1) = ':' THEN substr({l}, -6) ELSE '' END",
                        a.ts
                    ),
                    None,
                ))
            }
            Function::Now => {
                let now = self.now.clone();
                E::T(self.constant(now.into())?)
            }
            Function::Rand => {
                let mut v = V::numeric(
                    "((random() / 18446744073709551616.0) + 0.5)".into(),
                    numeric_type::DOUBLE.to_string(),
                );
                v.computed_num = true;
                E::T(v)
            }
            Function::StrUuid | Function::Uuid => {
                let uuid = "lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' || substr(hex(randomblob(2)), 2) || '-' || substr('89ab', 1 + (abs(random()) % 4), 1) || substr(hex(randomblob(2)), 2) || '-' || hex(randomblob(6)))";
                if matches!(f, Function::Uuid) {
                    E::T(V::iri(format!("('urn:uuid:' || {uuid})")))
                } else {
                    E::T(V::string(uuid.into(), None))
                }
            }
            Function::Iri => {
                let a = self.expr_term(&args[0], cols)?;
                if let Some(base) = &self.base_iri {
                    // Only absolute IRIs are passed through; relative ones need resolution.
                    let _ = base;
                }
                E::T(V::iri(format!(
                    "CASE WHEN ({k}) = {K_IRI} OR ({k}) = {K_STRING} THEN {l} END",
                    k = a.kind,
                    l = a.lex
                )))
            }
            Function::StrDt => {
                let a = self.args(args, cols)?;
                let dt_expr = &args[1];
                let Expression::NamedNode(dt) = dt_expr else {
                    return Err(Error::unsupported("STRDT with a non-constant datatype"));
                };
                E::T(Self::cast_value(&a[0], dt)?)
            }
            Function::StrLang => {
                let a = self.args(args, cols)?;
                let mut v = V::string(
                    format!("CASE WHEN ({}) = {K_STRING} THEN {} END", a[0].kind, a[0].lex),
                    None,
                );
                v.kind = format!("CASE WHEN ({}) = {K_STRING} THEN {K_LANG} END", a[0].kind);
                v.lang = format!("lower({})", a[1].lex);
                v.dt = xsd_str(rdf::LANG_STRING);
                v.stat = Stat::LangString;
                E::T(v)
            }
            Function::Custom(name) => {
                let a = self.args(args, cols)?;
                if a.len() == 1 && name.as_str().starts_with("http://www.w3.org/2001/XMLSchema#") {
                    E::T(Self::cast_value(&a[0], name)?)
                } else {
                    return Err(Error::unsupported(format!("custom function {name}")));
                }
            }
            other => return Err(Error::unsupported(format!("SPARQL function {other}"))),
        })
    }

    /// `xsd:*` casts and `STRDT`.
    fn cast_value(a: &V, dt: &NamedNode) -> Result<V> {
        let lex = &a.lex;
        Ok(match numeric_rank(dt.as_str()) {
            Some(numeric_type::INTEGER) if dt.as_ref() == xsd::INTEGER => V::integer(format!(
                "CASE WHEN ({n}) IS NOT NULL THEN CAST({n} AS INTEGER) WHEN ({b}) IS NOT NULL THEN ({b}) WHEN ({k}) = {K_STRING} AND trim({lex}) GLOB '[-+0-9]*' AND CAST(trim({lex}) AS INTEGER) || '' = ltrim(trim({lex}), '+') THEN CAST(trim({lex}) AS INTEGER) END",
                n = a.num,
                b = a.boolv,
                k = a.kind
            )),
            Some(rank) if rank != numeric_type::INTEGER => V::numeric(
                format!(
                    "CASE WHEN ({n}) IS NOT NULL THEN CAST({n} AS REAL) WHEN ({b}) IS NOT NULL THEN ({b}) * 1.0 WHEN ({k}) = {K_STRING} AND trim({lex}) GLOB '*[0-9]*' THEN CAST(trim({lex}) AS REAL) END",
                    n = a.num,
                    b = a.boolv,
                    k = a.kind
                ),
                rank.to_string(),
            ),
            _ if dt.as_ref() == xsd::STRING => V::string(
                format!(
                    "CASE WHEN ({}) IN ({K_IRI}, {K_STRING}, {K_TYPED}, {K_INT}, {K_BOOL}) THEN {lex} END",
                    a.kind
                ),
                None,
            ),
            _ if dt.as_ref() == xsd::BOOLEAN => {
                let b = format!(
                    "CASE WHEN ({bv}) IS NOT NULL THEN ({bv}) WHEN ({n}) IS NOT NULL THEN ({n}) <> 0 WHEN {lex} IN ('true', '1') THEN 1 WHEN {lex} IN ('false', '0') THEN 0 END",
                    bv = a.boolv,
                    n = a.num
                );
                V::boolean(&b)
            }
            _ => {
                if a.computed_num {
                    return Err(Error::unsupported("cast of a computed number"));
                }
                let mut v = V::typed(format!("CASE WHEN ({}) IS NOT NULL THEN {lex} END", a.kind), dt.as_str());
                if let Some(_) = crate::encoding::timestamp("1970-01-01T00:00:00Z", dt.as_str()) {
                    v.ts = format!("CASE WHEN ({}) IS NOT NULL THEN ({}) END", a.kind, a.ts);
                    v.stat = Stat::DateTime;
                }
                v
            }
        })
    }

    fn regex(&mut self, args: &[Expression], cols: &BTreeMap<usize, Binding>) -> Result<E> {
        let text = self.expr_term(&args[0], cols)?;
        let flags = match args.get(2) {
            None => Some(String::new()),
            Some(Expression::Literal(l)) => Some(l.value().to_string()),
            Some(_) => None,
        };
        let guard = text.is_string_like();
        if let (Expression::Literal(p), Some(flags)) = (&args[1], &flags) {
            let ci = flags == "i";
            if flags.is_empty() || ci {
                if let Some((starts, lit, ends)) = simple_regex(p.value()) {
                    let (t, l) = if ci {
                        (format!("lower({})", text.lex), sql_str(&lit.to_lowercase()))
                    } else {
                        (text.lex.clone(), sql_str(&lit))
                    };
                    let test = match (starts, ends) {
                        (true, true) => format!("{t} = {l}"),
                        (true, false) => format!("substr({t}, 1, length({l})) = {l}"),
                        (false, true) => format!("(length({l}) = 0 OR substr({t}, -length({l})) = {l})"),
                        (false, false) => format!("instr({t}, {l}) > 0"),
                    };
                    return Ok(E::B(format!("(CASE WHEN {guard} THEN {test} END)")));
                }
            }
        }
        if self.caps.udf {
            let pattern = self.expr_term(&args[1], cols)?;
            let flags = match args.get(2) {
                Some(f) => self.expr_term(f, cols)?.lex,
                None => "''".into(),
            };
            return Ok(E::B(format!(
                "(CASE WHEN {guard} THEN oxilite_regex({}, {}, {flags}) END)",
                text.lex, pattern.lex
            )));
        }
        Err(Error::unsupported(
            "REGEX with a non-literal pattern (no regex UDF on this backend)",
        ))
    }
}

/// Canonical lexical form of a computed numeric result, used by the decoder.
pub(crate) fn format_number(num: f64, dt: &str) -> Literal {
    match numeric_rank(dt) {
        Some(numeric_type::INTEGER) => Literal::from(num as i64),
        Some(numeric_type::DECIMAL) => {
            let d = oxsdatatypes::Decimal::try_from(oxsdatatypes::Double::from(num))
                .unwrap_or_default();
            Literal::from(d)
        }
        Some(numeric_type::FLOAT) => Literal::from(oxsdatatypes::Float::from(num as f32)),
        _ => Literal::from(oxsdatatypes::Double::from(num)),
    }
}

#[cfg(test)]
mod tests {
    use super::simple_regex;

    #[test]
    fn simple_regexes() {
        assert_eq!(simple_regex("^abc$"), Some((true, "abc".into(), true)));
        assert_eq!(simple_regex("a\\.b"), Some((false, "a.b".into(), false)));
        assert_eq!(simple_regex("a.b"), None);
        assert_eq!(simple_regex("(x|y)"), None);
    }
}
