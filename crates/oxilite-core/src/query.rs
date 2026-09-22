//! Query compilation entry point, execution job and result decoding.
//!
// @lat: [[architecture#SPARQL to SQL compiler]]

use crate::compiler::expr::format_number;
use crate::compiler::{Block, Col, Compiler, QueryOptions, VAL_FIELDS};
use crate::encoding::{blank_node_id, tag_of, Tag, DEFAULT_GRAPH_ID};
use crate::error::{Error, Result};
use crate::job::{Job, Step};
use crate::resolve::{join, TermResolver};
use crate::sql::{Capabilities, Request, Response, SqlValue, Statement};
use crate::stats::Stats;
use oxrdf::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple, Variable};
use spargebra::algebra::GraphPattern;
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};
use spargebra::Query;
use std::collections::{HashMap, HashSet};

/// Decoded query results.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryOutput {
    Solutions {
        variables: Vec<Variable>,
        rows: Vec<Vec<Option<Term>>>,
    },
    Boolean(bool),
    Graph(Vec<Triple>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Out {
    Id,
    Val,
}

#[derive(Debug, Clone)]
enum Form {
    Select,
    Ask,
    Construct(Vec<TriplePattern>),
    Describe,
}

/// A query compiled to SQL.
#[derive(Debug, Clone)]
pub struct CompiledQuery {
    /// The main SQL statement.
    pub sql: String,
    form: Form,
    variables: Vec<Variable>,
    layout: Vec<Out>,
    constants: HashMap<i64, Term>,
    union_default_graph: bool,
}

impl CompiledQuery {
    /// Human-readable description for `explain()`.
    pub fn explain(&self) -> String {
        format!(
            "-- oxilite: fully compiled to SQL ({} output variable(s))\n{}",
            self.variables.len(),
            self.sql
        )
    }
}

fn projected(p: &GraphPattern) -> Option<&[Variable]> {
    match p {
        GraphPattern::Project { variables, .. } => Some(variables),
        GraphPattern::Slice { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::OrderBy { inner, .. } => projected(inner),
        _ => None,
    }
}

fn template_vars(template: &[TriplePattern], out: &mut Vec<Variable>) {
    fn tp(t: &TermPattern, out: &mut Vec<Variable>) {
        match t {
            TermPattern::Variable(v) if !out.contains(v) => out.push(v.clone()),
            TermPattern::Triple(t) => {
                tp(&t.subject, out);
                if let NamedNodePattern::Variable(v) = &t.predicate {
                    if !out.contains(v) {
                        out.push(v.clone());
                    }
                }
                tp(&t.object, out);
            }
            _ => {}
        }
    }
    for t in template {
        tp(&t.subject, out);
        if let NamedNodePattern::Variable(v) = &t.predicate {
            if !out.contains(v) {
                out.push(v.clone());
            }
        }
        tp(&t.object, out);
    }
}

/// Compiles a SPARQL query to SQL. Returns [`Error::Unsupported`] for anything that cannot
/// be expressed in SQL on this backend.
pub fn compile_query(
    query: &Query,
    stats: &Stats,
    caps: &Capabilities,
    options: &QueryOptions,
) -> Result<CompiledQuery> {
    let (dataset, pattern, base, form) = match query {
        Query::Select {
            dataset,
            pattern,
            base_iri,
        } => (dataset, pattern, base_iri, Form::Select),
        Query::Ask {
            dataset,
            pattern,
            base_iri,
        } => (dataset, pattern, base_iri, Form::Ask),
        Query::Construct {
            template,
            dataset,
            pattern,
            base_iri,
        } => (
            dataset,
            pattern,
            base_iri,
            Form::Construct(template.clone()),
        ),
        Query::Describe {
            dataset,
            pattern,
            base_iri,
        } => (dataset, pattern, base_iri, Form::Describe),
    };
    let mut c = Compiler::new(
        stats,
        caps,
        options,
        dataset.as_ref(),
        base.as_ref().map(|b| b.as_str().to_string()),
    );
    let block = c.pattern(pattern)?;
    let variables: Vec<Variable> = match &form {
        Form::Construct(t) => {
            let mut v = Vec::new();
            template_vars(t, &mut v);
            v
        }
        _ => match projected(pattern) {
            Some(v) => v.to_vec(),
            None => block
                .cols
                .keys()
                .map(|i| c.var_names[*i].clone())
                .filter(|v| !v.as_str().starts_with('\u{1}'))
                .collect(),
        },
    };
    let idxs: Vec<usize> = variables.iter().map(|v| c.var(v)).collect();
    let layout = idxs
        .iter()
        .map(|i| match block.cols.get(i).map(|b| &b.col) {
            Some(Col::Val(_)) => Out::Val,
            _ => Out::Id,
        })
        .collect();
    let sql = match form {
        Form::Ask => {
            let inner = block.to_select(Some(&[]), false);
            format!("SELECT EXISTS ({inner})")
        }
        _ => final_select(&block, &idxs, caps.int64_as_text),
    };
    Ok(CompiledQuery {
        sql,
        form,
        variables,
        layout,
        constants: c.constants,
        union_default_graph: options.union_default_graph,
    })
}

fn final_select(block: &Block, idxs: &[usize], text_ids: bool) -> String {
    block.to_select(Some(idxs), text_ids)
}

/// Decodes a computed value from its 9 columns.
fn decode_val(cells: &[SqlValue]) -> Option<Term> {
    let get = |name: &str| {
        let i = VAL_FIELDS.iter().position(|f| *f == name).expect("field");
        &cells[i]
    };
    let kind = get("k").as_i64()?;
    let lex = get("l").clone().into_string();
    let dt = get("d").clone().into_string();
    let lang = get("g").clone().into_string();
    let num = get("n").as_f64();
    Some(match Tag::from_u8(kind as u8)? {
        Tag::Iri => NamedNode::new(lex?).ok()?.into(),
        Tag::BlankNode => BlankNode::new_unchecked(lex?).into(),
        Tag::String => Literal::new_simple_literal(lex?).into(),
        Tag::LangString => Literal::new_language_tagged_literal(lex?, lang?)
            .ok()?
            .into(),
        Tag::DirLangString => {
            let lang = lang?;
            let (tag, dir) = lang.split_once("--")?;
            let dir = if dir == "rtl" {
                oxrdf::BaseDirection::Rtl
            } else {
                oxrdf::BaseDirection::Ltr
            };
            Literal::new_directional_language_tagged_literal(lex?, tag, dir)
                .ok()?
                .into()
        }
        Tag::Integer => match lex {
            Some(l) => Literal::new_typed_literal(l, oxrdf::vocab::xsd::INTEGER).into(),
            None => Literal::from(num? as i64).into(),
        },
        Tag::Boolean => Literal::from(get("b").as_i64()? != 0).into(),
        Tag::Typed => {
            let dt = dt?;
            match lex {
                Some(l) => Literal::new_typed_literal(l, NamedNode::new_unchecked(dt)).into(),
                None => format_number(num?, &dt).into(),
            }
        }
        Tag::Triple | Tag::Default => return None,
    })
}

enum Cell {
    Id(i64),
    Term(Term),
    Unbound,
}

enum State {
    Start,
    Main,
    Resolving,
    Describe,
    DescribeResolving,
}

/// Runs a compiled query.
pub struct QueryJob {
    compiled: CompiledQuery,
    caps: Capabilities,
    state: State,
    resolver: TermResolver,
    cells: Vec<Vec<Cell>>,
    describe_done: HashSet<i64>,
    describe_quads: Vec<[i64; 3]>,
}

impl QueryJob {
    pub fn new(compiled: CompiledQuery, caps: Capabilities) -> Self {
        let resolver = TermResolver::with_constants(compiled.constants.clone());
        Self {
            compiled,
            caps,
            state: State::Start,
            resolver,
            cells: Vec::new(),
            describe_done: HashSet::new(),
            describe_quads: Vec::new(),
        }
    }

    pub fn compiled(&self) -> &CompiledQuery {
        &self.compiled
    }

    fn absorb_main(&mut self, response: Response) -> Result<()> {
        let rs = response
            .into_iter()
            .next()
            .ok_or_else(|| Error::backend("empty response"))?;
        for row in rs.rows {
            let mut out = Vec::with_capacity(self.compiled.layout.len());
            let mut i = 0;
            for l in &self.compiled.layout {
                match l {
                    Out::Id => {
                        let cell = match row.get(i).and_then(SqlValue::as_i64) {
                            Some(id) => {
                                self.resolver.want(id);
                                Cell::Id(id)
                            }
                            None => Cell::Unbound,
                        };
                        out.push(cell);
                        i += 1;
                    }
                    Out::Val => {
                        let cells = row
                            .get(i..i + VAL_FIELDS.len())
                            .ok_or_else(|| Error::corrupted("short row"))?;
                        out.push(decode_val(cells).map_or(Cell::Unbound, Cell::Term));
                        i += VAL_FIELDS.len();
                    }
                }
            }
            self.cells.push(out);
        }
        Ok(())
    }

    fn solutions(&self) -> Result<Vec<Vec<Option<Term>>>> {
        self.cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| match c {
                        Cell::Id(id) => self.resolver.get(*id).map(Some),
                        Cell::Term(t) => Ok(Some(t.clone())),
                        Cell::Unbound => Ok(None),
                    })
                    .collect()
            })
            .collect()
    }

    fn describe_request(&mut self, ids: Vec<i64>) -> Option<Request> {
        let ids: Vec<i64> = ids
            .into_iter()
            .filter(|id| {
                matches!(tag_of(*id), Some(Tag::Iri | Tag::BlankNode))
                    && self.describe_done.insert(*id)
            })
            .collect();
        if ids.is_empty() {
            return None;
        }
        let c = |x: &str| {
            if self.caps.int64_as_text {
                format!("CAST({x} AS TEXT)")
            } else {
                x.into()
            }
        };
        let graph = if self.compiled.union_default_graph {
            String::new()
        } else {
            format!(" AND g = {DEFAULT_GRAPH_ID}")
        };
        let stmts = ids
            .chunks(400)
            .map(|chunk| {
                Statement::new(format!(
                    "SELECT DISTINCT {}, {}, {} FROM quads WHERE s IN ({}){graph}",
                    c("s"),
                    c("p"),
                    c("o"),
                    join(chunk)
                ))
            })
            .collect();
        Some(Request::read(stmts))
    }

    fn finish(&mut self) -> Result<QueryOutput> {
        let rows = self.solutions()?;
        Ok(match &self.compiled.form {
            Form::Select => QueryOutput::Solutions {
                variables: self.compiled.variables.clone(),
                rows,
            },
            Form::Ask => unreachable!("handled on the main response"),
            Form::Construct(template) => {
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                for row in rows {
                    let mut bnodes = HashMap::new();
                    for t in template {
                        if let Some(triple) =
                            instantiate(t, &self.compiled.variables, &row, &mut bnodes)
                        {
                            if seen.insert(triple.clone()) {
                                out.push(triple);
                            }
                        }
                    }
                }
                QueryOutput::Graph(out)
            }
            Form::Describe => {
                let mut out = Vec::new();
                let mut seen = HashSet::new();
                for [s, p, o] in &self.describe_quads {
                    let (Ok(s), Ok(p), Ok(o)) = (
                        self.resolver.get(*s),
                        self.resolver.get(*p),
                        self.resolver.get(*o),
                    ) else {
                        continue;
                    };
                    let (Ok(s), Term::NamedNode(p)) = (crate::encoding::to_subject(s), p) else {
                        continue;
                    };
                    let t = Triple::new(s, p, o);
                    if seen.insert(t.clone()) {
                        out.push(t);
                    }
                }
                QueryOutput::Graph(out)
            }
        })
    }

    fn after_resolution(&mut self) -> Result<Step<QueryOutput>> {
        if let Some(r) = self.resolver.request(&self.caps) {
            return Ok(Step::Execute(r));
        }
        if let (Form::Describe, State::Resolving) = (&self.compiled.form, &self.state) {
            let ids: Vec<i64> = self
                .cells
                .iter()
                .flatten()
                .filter_map(|c| match c {
                    Cell::Id(id) => Some(*id),
                    Cell::Term(t) => match t {
                        Term::NamedNode(n) => Some(crate::encoding::named_node_id(n.as_str())),
                        Term::BlankNode(b) => Some(blank_node_id(b.as_str())),
                        _ => None,
                    },
                    Cell::Unbound => None,
                })
                .collect();
            self.state = State::Describe;
            if let Some(r) = self.describe_request(ids) {
                return Ok(Step::Execute(r));
            }
        }
        Ok(Step::Done(self.finish()?))
    }
}

fn instantiate(
    t: &TriplePattern,
    vars: &[Variable],
    row: &[Option<Term>],
    bnodes: &mut HashMap<String, BlankNode>,
) -> Option<Triple> {
    fn term(
        t: &TermPattern,
        vars: &[Variable],
        row: &[Option<Term>],
        bnodes: &mut HashMap<String, BlankNode>,
    ) -> Option<Term> {
        Some(match t {
            TermPattern::NamedNode(n) => n.clone().into(),
            TermPattern::Literal(l) => l.clone().into(),
            TermPattern::BlankNode(b) => bnodes
                .entry(b.as_str().to_string())
                .or_default()
                .clone()
                .into(),
            TermPattern::Variable(v) => row[vars.iter().position(|x| x == v)?].clone()?,
            TermPattern::Triple(tp) => instantiate(tp, vars, row, bnodes)?.into(),
        })
    }
    let s = match term(&t.subject, vars, row, bnodes)? {
        Term::NamedNode(n) => NamedOrBlankNode::from(n),
        Term::BlankNode(b) => NamedOrBlankNode::from(b),
        _ => return None,
    };
    let p = match &t.predicate {
        NamedNodePattern::NamedNode(n) => n.clone(),
        NamedNodePattern::Variable(v) => match row[vars.iter().position(|x| x == v)?].clone()? {
            Term::NamedNode(n) => n,
            _ => return None,
        },
    };
    let o = term(&t.object, vars, row, bnodes)?;
    Some(Triple::new(s, p, o))
}

impl Job for QueryJob {
    type Output = QueryOutput;

    fn step(&mut self, response: Option<Response>) -> Result<Step<QueryOutput>> {
        match (&self.state, response) {
            (State::Start, _) => {
                self.state = State::Main;
                Ok(Step::Execute(Request::read(vec![Statement::new(
                    self.compiled.sql.clone(),
                )])))
            }
            (State::Main, Some(r)) => {
                if let Form::Ask = self.compiled.form {
                    let b = r
                        .first()
                        .and_then(|rs| rs.rows.first())
                        .and_then(|row| row.first())
                        .and_then(SqlValue::as_i64)
                        .unwrap_or(0);
                    return Ok(Step::Done(QueryOutput::Boolean(b != 0)));
                }
                self.absorb_main(r)?;
                self.state = State::Resolving;
                self.after_resolution()
            }
            (State::Resolving, Some(r)) => {
                self.resolver.absorb(r)?;
                self.after_resolution()
            }
            (State::Describe, Some(r)) => {
                let mut next = Vec::new();
                for rs in r {
                    for row in rs.rows {
                        let ids: Vec<i64> = row.iter().filter_map(SqlValue::as_i64).collect();
                        if let [s, p, o] = ids[..] {
                            self.resolver.want(s);
                            self.resolver.want(p);
                            self.resolver.want(o);
                            self.describe_quads.push([s, p, o]);
                            if tag_of(o) == Some(Tag::BlankNode) {
                                next.push(o);
                            }
                        }
                    }
                }
                // Concise bounded description: follow blank-node objects.
                if let Some(r) = self.describe_request(next) {
                    return Ok(Step::Execute(r));
                }
                self.state = State::DescribeResolving;
                match self.resolver.request(&self.caps) {
                    Some(r) => Ok(Step::Execute(r)),
                    None => Ok(Step::Done(self.finish()?)),
                }
            }
            (State::DescribeResolving, Some(r)) => {
                self.resolver.absorb(r)?;
                match self.resolver.request(&self.caps) {
                    Some(r) => Ok(Step::Execute(r)),
                    None => Ok(Step::Done(self.finish()?)),
                }
            }
            (_, None) => Err(Error::Other("query job resumed without a response".into())),
        }
    }
}
