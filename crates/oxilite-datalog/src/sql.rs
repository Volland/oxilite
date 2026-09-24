//! Compiles a checked program to SQL over the term-id encoding.
//!
//! Every relation — stored or derived — is a set of rows of tagged 64-bit term ids, so a
//! derived predicate composes with a triple pattern without any conversion. Non-recursive
//! predicates become plain common table expressions, a linear recursive component becomes one
//! `WITH RECURSIVE` member, and a mutually recursive component becomes one member carrying a
//! discriminant column.
//!
// @lat: [[architecture#Datalog frontend#SQL generation]]

use crate::ast::*;
use crate::error::{DatalogError, Result};
use crate::program::{Analysis, Shape, Stratum};
use oxilite_core::encoding::{term_id, Tag, INT_OFFSET, PAYLOAD_MASK};
use oxilite_core::sql::Capabilities;
use oxrdf::{Term, TermRef};
use std::collections::HashMap;
use std::fmt::Write as _;

/// The id every inline integer is offset from: `id = INT_ZERO + value`.
fn int_zero() -> i64 {
    Tag::Integer.base() + INT_OFFSET
}

fn int_lo() -> i64 {
    Tag::Integer.base()
}

fn int_hi() -> i64 {
    Tag::Integer.base() | PAYLOAD_MASK
}

/// The widest relation the work table can hold, and so the widest a component evaluated by
/// iteration may be.
pub const MAX_FIXPOINT_ARITY: usize = 6;

/// The default producer name of materialized rule conclusions.
pub const DATALOG_PRODUCER: &str = "datalog";

/// Options for compiling a program. The defaults match SPARQL's: the default graph only, and
/// asserted triples only.
#[derive(Debug, Clone)]
pub struct Options {
    /// Match every graph rather than only the default graph.
    pub union_default_graph: bool,
    /// Also match materialized inferences (`quads_inf`).
    pub include_inferred: bool,
    /// How many rounds a component evaluated by iteration may take before it is called
    /// divergent. Each round is one request, so this also bounds the round trips.
    pub max_iterations: usize,
    /// The name materialized conclusions are attributed to: materializing replaces only this
    /// producer's earlier conclusions (default `datalog`).
    pub producer: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            union_default_graph: false,
            include_inferred: false,
            max_iterations: 100,
            producer: DATALOG_PRODUCER.to_owned(),
        }
    }
}

/// One component evaluated by iterating, because SQLite cannot express it in a statement.
///
/// The seed statements apply the component's non-recursive rules once; the step statements
/// apply its recursive rules and are repeated until the work table stops growing.
#[derive(Debug, Clone)]
pub struct FixpointPhase {
    /// The predicates of the component, for `explain()`.
    pub preds: Vec<String>,
    pub seed: Vec<String>,
    pub step: Vec<String>,
}

/// Everything a program needs to run before its main statement.
#[derive(Debug, Clone, Default)]
pub struct Fixpoint {
    /// Scopes this evaluation's rows in the shared work table.
    pub run: i64,
    pub phases: Vec<FixpointPhase>,
    /// Rounds a phase may take before it is called divergent.
    pub max_iterations: usize,
}

impl Fixpoint {
    /// Is there anything to iterate?
    pub fn is_empty(&self) -> bool {
        self.phases.is_empty()
    }

    /// Removes this evaluation's rows.
    pub fn cleanup(&self) -> String {
        format!("DELETE FROM datalog_work WHERE run = {}", self.run)
    }

    /// Counts them, which is how growth is detected.
    pub fn count(&self) -> String {
        format!("SELECT COUNT(*) FROM datalog_work WHERE run = {}", self.run)
    }
}

/// A program compiled to one SQL statement.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub sql: String,
    /// Output variables, in column order.
    pub variables: Vec<String>,
    /// Constants the program mentions, so the resolver need not look them up.
    pub constants: HashMap<i64, Term>,
    /// Strategy and planning notes for `explain()`.
    pub notes: Vec<String>,
    /// Components that must be iterated into the work table before `sql` runs.
    pub fixpoint: Fixpoint,
}

/// Compiles a checked program.
pub fn compile(
    program: &Program,
    analysis: &Analysis,
    caps: &Capabilities,
    options: &Options,
) -> Result<Compiled> {
    let goal = program.goal.as_ref().ok_or(DatalogError::NoGoal)?;
    compile_for(program, analysis, caps, options, goal, new_run())
}

/// Compiles a program against a goal of the caller's choosing, which is how materialization
/// asks for one rule head's relation.
///
/// `run` scopes the work table. Several goals over one program share a run so that a component
/// needing iteration is evaluated once and read by all of them.
pub fn compile_for(
    program: &Program,
    analysis: &Analysis,
    caps: &Capabilities,
    options: &Options,
    goal: &Goal,
    run: i64,
) -> Result<Compiled> {
    let mut c = Compiler {
        program,
        analysis,
        caps,
        options,
        names: HashMap::new(),
        fixpoint_rel: HashMap::new(),
        constants: HashMap::new(),
        notes: Vec::new(),
        counter: 0,
        run,
    };
    c.assign_names();

    let mut members: Vec<String> = Vec::new();
    let mut phases: Vec<FixpointPhase> = Vec::new();
    for (i, stratum) in analysis.strata.iter().enumerate() {
        c.stratum(i, stratum, &mut members, &mut phases)?;
    }

    let prelude = prelude_of(&members);
    let (select, variables) = c.goal_select(goal)?;
    let sql = format!("{prelude}{select}");

    Ok(Compiled {
        sql,
        variables,
        constants: c.constants,
        notes: c.notes,
        fixpoint: Fixpoint {
            run,
            phases,
            max_iterations: options.max_iterations,
        },
    })
}

/// A fresh id scoping one evaluation's rows in the shared work table.
///
/// Process id and a counter keep concurrent evaluations — in one process or several — from
/// seeing each other's rows, and make cleanup exact.
pub fn new_run() -> i64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let pid = u64::from(std::process::id());
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    ((pid.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ clock.rotate_left(17) ^ n.wrapping_mul(0x51)) >> 1)
        as i64
}

/// `WITH RECURSIVE …` for the members a statement needs, or nothing when it needs none.
///
/// `WITH RECURSIVE` only enables recursion for the list; plain members stay plain, so one
/// prefix serves both kinds.
fn prelude_of(members: &[String]) -> String {
    if members.is_empty() {
        String::new()
    } else {
        format!("WITH RECURSIVE {}\n", members.join(",\n"))
    }
}

struct Compiler<'a> {
    program: &'a Program,
    analysis: &'a Analysis,
    caps: &'a Capabilities,
    options: &'a Options,
    /// SQL name of each derived relation.
    names: HashMap<Pred, String>,
    /// Relations that live in the work table instead of a common table expression.
    fixpoint_rel: HashMap<Pred, usize>,
    constants: HashMap<i64, Term>,
    notes: Vec<String>,
    counter: usize,
    run: i64,
}

impl<'a> Compiler<'a> {
    fn assign_names(&mut self) {
        let mut n = 0;
        for stratum in &self.analysis.strata {
            for pred in &stratum.preds {
                self.names.insert(pred.clone(), format!("dl{n}"));
                n += 1;
            }
        }
    }

    fn alias(&mut self) -> String {
        self.counter += 1;
        format!("r{}", self.counter)
    }

    fn arity_of(&self, pred: &Pred) -> usize {
        self.analysis
            .arity
            .get(pred)
            .copied()
            .or_else(|| pred.arity())
            .unwrap_or(0)
    }

    /// Is this relation defined by rules, rather than read from the store?
    fn is_derived(&self, pred: &Pred) -> bool {
        self.analysis.arity.contains_key(pred)
    }

    /// The table every triple pattern reads.
    fn quad_source(&self) -> String {
        if self.options.include_inferred {
            "(SELECT s, p, o, g FROM quads UNION ALL SELECT s, p, o, g FROM quads_inf)".to_owned()
        } else {
            "quads".to_owned()
        }
    }

    fn register_const(&mut self, term: &Term) -> i64 {
        let id = term_id(TermRef::from(term));
        self.constants.insert(id, term.clone());
        id
    }

    // ---------------------------------------------------------------- strata

    fn stratum(
        &mut self,
        index: usize,
        stratum: &Stratum,
        members: &mut Vec<String>,
        phases: &mut Vec<FixpointPhase>,
    ) -> Result<()> {
        match stratum.shape {
            Shape::NonRecursive => {
                for pred in &stratum.preds {
                    let body = self.union_of_rules(pred, None)?;
                    members.push(self.member(pred, &body));
                }
                Ok(())
            }
            Shape::Linear => {
                let pred = &stratum.preds[0];
                self.notes.push(format!(
                    "{pred}: linear recursion, one WITH RECURSIVE member"
                ));
                let body = self.union_of_rules(pred, None)?;
                members.push(self.member(pred, &body));
                Ok(())
            }
            Shape::LinearMutual => self.mutual(index, stratum, members),
            // Two recursive body atoms have no single-statement form, so the component is
            // iterated into the work table instead, one request per round.
            Shape::NonLinear => self.fixpoint(stratum, members, phases),
        }
    }

    /// Builds the seed and step statements for a component that must be iterated.
    ///
    /// The statements run in their own requests, so each one carries the common table
    /// expressions of the earlier strata it reads.
    fn fixpoint(
        &mut self,
        stratum: &Stratum,
        members: &[String],
        phases: &mut Vec<FixpointPhase>,
    ) -> Result<()> {
        let names: Vec<String> = stratum.preds.iter().map(Pred::to_string).collect();
        self.notes.push(format!(
            "{names:?}: non-linear recursion, evaluated by iteration — one request per round"
        ));
        // Register the relations first: the component's own rules refer to them.
        let base = self.fixpoint_rel.len();
        for (i, pred) in stratum.preds.iter().enumerate() {
            let arity = self.arity_of(pred);
            if arity > MAX_FIXPOINT_ARITY {
                return Err(DatalogError::unsupported(format!(
                    "`{pred}` has arity {arity}; a component evaluated by iteration may be at \
                     most {MAX_FIXPOINT_ARITY} wide"
                )));
            }
            self.fixpoint_rel.insert(pred.clone(), base + i);
        }

        let prelude = prelude_of(members);
        let mut seed = Vec::new();
        let mut step = Vec::new();
        for pred in &stratum.preds {
            let rel = self.fixpoint_rel[pred];
            let arity = self.arity_of(pred);
            let rules: Vec<Rule> = self.program.rules_for(pred).cloned().collect();
            for rule in &rules {
                let recursive = rule.positive().any(|a| stratum.preds.contains(&a.pred));
                let select = self.rule_select(rule, None)?;
                let sql = self.insert_into_work(&prelude, rel, arity, &select);
                if recursive {
                    step.push(sql);
                } else {
                    seed.push(sql);
                }
            }
        }
        phases.push(FixpointPhase {
            preds: names,
            seed,
            step,
        });
        Ok(())
    }

    /// `INSERT OR IGNORE INTO datalog_work … SELECT run, rel, * FROM (<the rule>)`.
    ///
    /// The primary key covers every column, so `OR IGNORE` is the deduplication that makes the
    /// row count monotone and the fixpoint detectable.
    fn insert_into_work(&self, prelude: &str, rel: usize, arity: usize, select: &str) -> String {
        let cols: Vec<String> = (0..arity).map(|i| format!("c{i}")).collect();
        format!(
            "{prelude}INSERT OR IGNORE INTO datalog_work (run, rel, {}) \
             SELECT {}, {rel}, * FROM ({select})",
            cols.join(", "),
            self.run
        )
    }

    /// `name(c0, c1, …) AS (<body>)`
    fn member(&self, pred: &Pred, body: &str) -> String {
        let name = &self.names[pred];
        let cols: Vec<String> = (0..self.arity_of(pred)).map(|i| format!("c{i}")).collect();
        format!("{name}({}) AS (\n{body}\n)", cols.join(", "))
    }

    /// A mutually recursive component: one member with a `tag` column, then one plain member
    /// per predicate reading its own rows back out.
    fn mutual(&mut self, index: usize, stratum: &Stratum, out: &mut Vec<String>) -> Result<()> {
        if !self.caps.compound_recursive_cte {
            return Err(DatalogError::unsupported(format!(
                "mutual recursion between {:?} needs a recursive CTE with several recursive \
                 terms, which this backend's SQLite does not support (3.34.0 or later is \
                 required)",
                stratum
                    .preds
                    .iter()
                    .map(Pred::to_string)
                    .collect::<Vec<_>>()
            )));
        }
        let scc = format!("scc{index}");
        let width = stratum
            .preds
            .iter()
            .map(|p| self.arity_of(p))
            .max()
            .unwrap_or(0);
        self.notes.push(format!(
            "{:?}: mutual recursion, one tagged WITH RECURSIVE member",
            stratum
                .preds
                .iter()
                .map(Pred::to_string)
                .collect::<Vec<_>>()
        ));

        let mut arms: Vec<String> = Vec::new();
        for (tag, pred) in stratum.preds.iter().enumerate() {
            let arity = self.arity_of(pred);
            let selects = self.rule_selects(pred, Some((&scc, stratum, tag, width)))?;
            for s in selects {
                // Pad every arm to the component's width so the compound terms line up.
                let padded = pad_select(&s, tag, arity, width);
                arms.push(padded);
            }
        }
        let cols: Vec<String> = std::iter::once("tag".to_owned())
            .chain((0..width).map(|i| format!("c{i}")))
            .collect();
        out.push(format!(
            "{scc}({}) AS (\n{}\n)",
            cols.join(", "),
            arms.join("\nUNION\n")
        ));

        for (tag, pred) in stratum.preds.iter().enumerate() {
            let name = &self.names[pred];
            let arity = self.arity_of(pred);
            let proj: Vec<String> = (0..arity).map(|i| format!("c{i}")).collect();
            let cols: Vec<String> = (0..arity).map(|i| format!("c{i}")).collect();
            out.push(format!(
                "{name}({}) AS (SELECT {} FROM {scc} WHERE tag = {tag})",
                cols.join(", "),
                proj.join(", ")
            ));
        }
        Ok(())
    }

    /// The `UNION` of every rule defining a predicate.
    fn union_of_rules(&mut self, pred: &Pred, scc: Option<SccCtx<'_>>) -> Result<String> {
        let selects = self.rule_selects(pred, scc)?;
        if selects.is_empty() {
            // A predicate with no rules is the empty relation.
            let arity = self.arity_of(pred);
            let nulls: Vec<&str> = (0..arity).map(|_| "NULL").collect();
            return Ok(format!(
                "SELECT {} WHERE 0",
                if nulls.is_empty() {
                    "1".to_owned()
                } else {
                    nulls.join(", ")
                }
            ));
        }
        // `UNION` rather than `UNION ALL`: Datalog is set semantics, and deduplication is what
        // makes a recursive member terminate on cyclic data.
        Ok(selects.join("\nUNION\n"))
    }

    fn rule_selects(&mut self, pred: &Pred, scc: Option<SccCtx<'_>>) -> Result<Vec<String>> {
        let rules: Vec<Rule> = self.program.rules_for(pred).cloned().collect();
        let mut out = Vec::with_capacity(rules.len());
        // Non-recursive rules must come first: SQLite needs the anchor before the recursive
        // terms of a compound recursive member.
        let inside: Vec<Pred> = scc
            .map(|(_, s, _, _)| s.preds.clone())
            .unwrap_or_else(|| vec![pred.clone()]);
        let recursive = |r: &Rule| r.positive().any(|a| inside.contains(&a.pred));
        for rule in rules.iter().filter(|r| !recursive(r)) {
            out.push(self.rule_select(rule, scc)?);
        }
        for rule in rules.iter().filter(|r| recursive(r)) {
            out.push(self.rule_select(rule, scc)?);
        }
        Ok(out)
    }

    // ------------------------------------------------------------- one rule

    fn rule_select(&mut self, rule: &Rule, scc: Option<SccCtx<'_>>) -> Result<String> {
        let mut frame = Frame::default();
        for item in &rule.body {
            if let BodyItem::Atom(atom) = item {
                self.bind_atom(atom, &mut frame, scc)?;
            }
        }
        for item in &rule.body {
            match item {
                BodyItem::Negated(atom) => {
                    let sql = self.negated(atom, &frame, scc)?;
                    frame.wheres.push(sql);
                }
                BodyItem::Constraint(expr) => {
                    let sql = self.expr(expr, &frame)?.as_bool();
                    frame.wheres.push(format!("({sql})"));
                }
                BodyItem::Atom(_) => {}
            }
        }

        let mut projection: Vec<String> = Vec::new();
        let mut group: Vec<String> = Vec::new();
        let aggregating = rule.head.aggregates();
        for arg in &rule.head.args {
            match arg {
                HeadArg::Plain(a) => {
                    let sql = self.head_term(a, &frame, &rule.head)?;
                    if aggregating {
                        group.push(sql.clone());
                    }
                    projection.push(sql);
                }
                HeadArg::Agg { func, var } => {
                    projection.push(self.aggregate(*func, var, &frame, &rule.head)?);
                }
            }
        }

        let mut sql = format!("SELECT {}", projection.join(", "));
        if frame.froms.is_empty() {
            // A fact: a row of constants, with no table to read.
            if !frame.wheres.is_empty() {
                let _ = write!(sql, " WHERE {}", frame.wheres.join(" AND "));
            }
            return Ok(sql);
        }
        let _ = write!(sql, " FROM {}", frame.froms.join(", "));
        if !frame.wheres.is_empty() {
            let _ = write!(sql, " WHERE {}", frame.wheres.join(" AND "));
        }
        if aggregating && !group.is_empty() {
            let _ = write!(sql, " GROUP BY {}", group.join(", "));
        }
        Ok(sql)
    }

    /// Adds a positive atom to the frame: one table reference, plus the conditions its
    /// constants and repeated variables impose.
    fn bind_atom(&mut self, atom: &Atom, frame: &mut Frame, scc: Option<SccCtx<'_>>) -> Result<()> {
        let alias = self.alias();
        let (source, columns, extra) = self.source_of(atom, &alias, scc)?;
        frame.froms.push(format!("{source} {alias}"));
        frame.wheres.extend(extra);
        for (i, arg) in atom.args.iter().enumerate() {
            let col = format!("{alias}.{}", columns[i]);
            match arg {
                Arg::Wildcard => {}
                Arg::Var(v) => match frame.binding.get(v) {
                    // A variable seen before is an equality between the two occurrences.
                    Some(prev) => frame.wheres.push(format!("{col} = {prev}")),
                    None => {
                        frame.binding.insert(v.clone(), col);
                    }
                },
                Arg::Const(t) => {
                    let id = self.register_const(t);
                    frame.wheres.push(format!("{col} = {id}"));
                }
            }
        }
        Ok(())
    }

    /// The table (or CTE) an atom reads, the columns its arguments map onto, and any
    /// conditions the source itself imposes.
    fn source_of(
        &mut self,
        atom: &Atom,
        alias: &str,
        scc: Option<SccCtx<'_>>,
    ) -> Result<(String, Vec<String>, Vec<String>)> {
        // A predicate that rules define is read from its relation, never from `quads` — the
        // two would otherwise mean different things in the head and in the body.
        if self.is_derived(&atom.pred) {
            let arity = self.arity_of(&atom.pred);
            let cols: Vec<String> = (0..arity).map(|i| format!("c{i}")).collect();
            // A component that is iterated lives in the work table, in this run's rows.
            if let Some(&rel) = self.fixpoint_rel.get(&atom.pred) {
                return Ok((
                    "datalog_work".to_owned(),
                    cols,
                    vec![
                        format!("{alias}.run = {}", self.run),
                        format!("{alias}.rel = {rel}"),
                    ],
                ));
            }
            return Ok(match scc {
                // Inside a mutually recursive member, a predicate of the same component is
                // read from the tagged relation, so the member keeps exactly one
                // self-reference per recursive term.
                Some((scc_name, stratum, _, _)) if stratum.preds.contains(&atom.pred) => {
                    let tag = stratum
                        .preds
                        .iter()
                        .position(|p| p == &atom.pred)
                        .expect("checked by contains");
                    (
                        scc_name.to_owned(),
                        cols,
                        vec![format!("{alias}.tag = {tag}")],
                    )
                }
                _ => {
                    let name = self
                        .names
                        .get(&atom.pred)
                        .cloned()
                        .ok_or_else(|| DatalogError::UnknownGoal(atom.pred.to_string()))?;
                    (name, cols, Vec::new())
                }
            });
        }
        Ok(match &atom.pred {
            Pred::Edb(iri) => {
                let id = self.register_const(&Term::from(iri.clone()));
                let unary = atom.args.len() == 1;
                let mut extra = if unary {
                    // `ex:Person(?x)` is `?x rdf:type ex:Person`.
                    vec![
                        format!("{alias}.p = {}", oxilite_core::encoding::rdf_type_id()),
                        format!("{alias}.o = {id}"),
                    ]
                } else {
                    vec![format!("{alias}.p = {id}")]
                };
                if !self.options.union_default_graph {
                    extra.push(format!("{alias}.g = 0"));
                }
                let cols = if unary {
                    vec!["s".to_owned()]
                } else {
                    vec!["s".to_owned(), "o".to_owned()]
                };
                (self.quad_source(), cols, extra)
            }
            Pred::Triple { graph } => {
                let mut extra = Vec::new();
                let mut cols = vec!["s".to_owned(), "p".to_owned(), "o".to_owned()];
                if *graph {
                    cols.push("g".to_owned());
                } else if !self.options.union_default_graph {
                    extra.push(format!("{alias}.g = 0"));
                }
                (self.quad_source(), cols, extra)
            }
            Pred::Idb(name) => {
                // A derived relation with no rules is empty, and the goal naming one is a
                // mistake worth reporting by name.
                return Err(DatalogError::UnknownGoal(name.clone()));
            }
        })
    }

    /// `NOT EXISTS (…)` for a negated atom. Safety guarantees every variable is already bound,
    /// so the subquery only ever correlates outward.
    fn negated(&mut self, atom: &Atom, frame: &Frame, scc: Option<SccCtx<'_>>) -> Result<String> {
        let alias = self.alias();
        let (source, columns, extra) = self.source_of(atom, &alias, scc)?;
        let mut conds = extra;
        let mut local: HashMap<String, String> = HashMap::new();
        for (i, arg) in atom.args.iter().enumerate() {
            let col = format!("{alias}.{}", columns[i]);
            match arg {
                Arg::Wildcard => {}
                Arg::Var(v) => {
                    if let Some(outer) = frame.binding.get(v) {
                        conds.push(format!("{col} = {outer}"));
                    } else if let Some(prev) = local.get(v) {
                        conds.push(format!("{col} = {prev}"));
                    } else {
                        local.insert(v.clone(), col);
                    }
                }
                Arg::Const(t) => {
                    let id = self.register_const(t);
                    conds.push(format!("{col} = {id}"));
                }
            }
        }
        let where_clause = if conds.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conds.join(" AND "))
        };
        Ok(format!(
            "NOT EXISTS (SELECT 1 FROM {source} {alias}{where_clause})"
        ))
    }

    fn head_term(&mut self, arg: &Arg, frame: &Frame, head: &Head) -> Result<String> {
        Ok(match arg {
            Arg::Var(v) => frame
                .binding
                .get(v)
                .cloned()
                .ok_or_else(|| DatalogError::Unsafe {
                    predicate: head.pred.to_string(),
                    variable: format!("?{v}"),
                })?,
            Arg::Const(t) => self.register_const(t).to_string(),
            Arg::Wildcard => {
                return Err(DatalogError::Unsafe {
                    predicate: head.pred.to_string(),
                    variable: "_".to_owned(),
                })
            }
        })
    }

    /// Aggregates produce term ids, so they are restricted to what the encoding can represent
    /// without a `terms` row: counts and sums become inline integers, and `MIN`/`MAX`/`SAMPLE`
    /// pick an existing id under the store's total order.
    fn aggregate(&mut self, func: AggFn, var: &str, frame: &Frame, head: &Head) -> Result<String> {
        let zero = int_zero();
        if var == "*" {
            return Ok(match func {
                AggFn::Count | AggFn::CountDistinct => format!("(COUNT(*) + {zero})"),
                _ => {
                    return Err(DatalogError::unsupported(format!(
                        "{}(*) is only defined for COUNT",
                        func.name()
                    )))
                }
            });
        }
        let col = frame
            .binding
            .get(var)
            .cloned()
            .ok_or_else(|| DatalogError::Unsafe {
                predicate: head.pred.to_string(),
                variable: format!("?{var}"),
            })?;
        Ok(match func {
            AggFn::Count => format!("(COUNT({col}) + {zero})"),
            AggFn::CountDistinct => format!("(COUNT(DISTINCT {col}) + {zero})"),
            AggFn::Sum => format!("(CAST(SUM({}) AS INTEGER) + {zero})", numeric(&col)),
            // Ids of inline integers sort by value, and the encoding gives every other term a
            // total order, so MIN and MAX are exact for numbers and deterministic elsewhere.
            AggFn::Min => format!("MIN({col})"),
            AggFn::Max => format!("MAX({col})"),
            AggFn::Sample => format!("MIN({col})"),
            AggFn::Avg | AggFn::GroupConcat => {
                return Err(DatalogError::unsupported(format!(
                    "{} produces a value that has no inline term id; compute it in the goal \
                     projection instead",
                    func.name()
                )))
            }
        })
    }

    // --------------------------------------------------------- constraints

    fn expr(&mut self, e: &Expr, frame: &Frame) -> Result<Val> {
        Ok(match e {
            Expr::Var(v) => {
                Val::Id(
                    frame
                        .binding
                        .get(v)
                        .cloned()
                        .ok_or_else(|| DatalogError::Unsafe {
                            predicate: "constraint".to_owned(),
                            variable: format!("?{v}"),
                        })?,
                )
            }
            Expr::Const(t) => match numeric_literal(t) {
                // A number in the program text is a value, so it compares numerically with
                // whatever the store holds, whatever datatype that value was written with.
                Some(n) => Val::Num(n),
                None => Val::Const {
                    id: self.register_const(t).to_string(),
                    lex: lexical_form(t),
                },
            },
            Expr::Neg(inner) => {
                let a = self.expr(inner, frame)?;
                Val::Num(format!("(-{})", a.as_num()))
            }
            Expr::Not(inner) => {
                let a = self.expr(inner, frame)?;
                Val::Bool(format!("(NOT ({}))", a.as_bool()))
            }
            Expr::Binary { op, left, right } => {
                let l = self.expr(left, frame)?;
                let r = self.expr(right, frame)?;
                match op {
                    BinOp::And => Val::Bool(format!("(({}) AND ({}))", l.as_bool(), r.as_bool())),
                    BinOp::Or => Val::Bool(format!("(({}) OR ({}))", l.as_bool(), r.as_bool())),
                    // Division is real division, as in SPARQL, not SQLite's integer division.
                    BinOp::Div => Val::Num(format!(
                        "({} * 1.0 / NULLIF({}, 0))",
                        l.as_num(),
                        r.as_num()
                    )),
                    BinOp::Add | BinOp::Sub | BinOp::Mul => {
                        let o = match op {
                            BinOp::Add => "+",
                            BinOp::Sub => "-",
                            _ => "*",
                        };
                        Val::Num(format!("({} {o} {})", l.as_num(), r.as_num()))
                    }
                    BinOp::Eq | BinOp::Ne => {
                        let o = if *op == BinOp::Eq { "=" } else { "<>" };
                        // Ids are canonical, so term identity is id equality — the cheapest
                        // comparison there is. Values are compared only when one side was
                        // computed and therefore has no id.
                        if l.is_term() && r.is_term() {
                            Val::Bool(format!("({} {o} {})", l.raw(), r.raw()))
                        } else {
                            Val::Bool(format!("({} {o} {})", l.as_num(), r.as_num()))
                        }
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        let o = match op {
                            BinOp::Lt => "<",
                            BinOp::Le => "<=",
                            BinOp::Gt => ">",
                            _ => ">=",
                        };
                        Val::Bool(format!("({} {o} {})", l.as_num(), r.as_num()))
                    }
                }
            }
            Expr::Call { name, args } => self.call(name, args, frame)?,
        })
    }

    fn call(&mut self, name: &str, args: &[Expr], frame: &Frame) -> Result<Val> {
        let mut a = Vec::with_capacity(args.len());
        for arg in args {
            a.push(self.expr(arg, frame)?);
        }
        let need = |i: usize| -> Result<&Val> {
            a.get(i)
                .ok_or_else(|| DatalogError::unsupported(format!("{name} needs more arguments")))
        };
        Ok(match name {
            "ABS" => Val::Num(format!("ABS({})", need(0)?.as_num())),
            "CEIL" => Val::Num(format!("CAST(CEIL({}) AS INTEGER)", need(0)?.as_num())),
            "FLOOR" => Val::Num(format!("CAST(FLOOR({}) AS INTEGER)", need(0)?.as_num())),
            "ROUND" => Val::Num(format!("CAST(ROUND({}) AS INTEGER)", need(0)?.as_num())),
            "STRLEN" => Val::Num(format!("LENGTH({})", need(0)?.as_text())),
            "UCASE" => Val::Text(format!("UPPER({})", need(0)?.as_text())),
            "LCASE" => Val::Text(format!("LOWER({})", need(0)?.as_text())),
            "STR" => Val::Text(need(0)?.as_text()),
            "CONCAT" => Val::Text(format!(
                "({})",
                a.iter().map(Val::as_text).collect::<Vec<_>>().join(" || ")
            )),
            "SUBSTR" => {
                let len = match a.get(2) {
                    Some(v) => format!(", {}", v.as_num()),
                    None => String::new(),
                };
                Val::Text(format!(
                    "SUBSTR({}, {}{len})",
                    need(0)?.as_text(),
                    need(1)?.as_num()
                ))
            }
            "CONTAINS" => Val::Bool(format!(
                "(INSTR({}, {}) > 0)",
                need(0)?.as_text(),
                need(1)?.as_text()
            )),
            "STRSTARTS" => Val::Bool(format!(
                "(SUBSTR({a}, 1, LENGTH({b})) = {b})",
                a = need(0)?.as_text(),
                b = need(1)?.as_text()
            )),
            "STRENDS" => Val::Bool(format!(
                "(SUBSTR({a}, -LENGTH({b})) = {b})",
                a = need(0)?.as_text(),
                b = need(1)?.as_text()
            )),
            "STRBEFORE" => Val::Text(format!(
                "SUBSTR({a}, 1, INSTR({a}, {b}) - 1)",
                a = need(0)?.as_text(),
                b = need(1)?.as_text()
            )),
            "STRAFTER" => Val::Text(format!(
                "SUBSTR({a}, INSTR({a}, {b}) + LENGTH({b}))",
                a = need(0)?.as_text(),
                b = need(1)?.as_text()
            )),
            "ISNUMERIC" => Val::Bool(format!("({} IS NOT NULL)", need(0)?.as_num())),
            "BOUND" => Val::Bool(format!("({} IS NOT NULL)", need(0)?.raw())),
            "SAMETERM" => Val::Bool(format!("({} = {})", need(0)?.raw(), need(1)?.raw())),
            "ISIRI" | "ISURI" => Val::Bool(tag_test(&need(0)?.raw(), Tag::Iri)),
            "ISBLANK" => Val::Bool(tag_test(&need(0)?.raw(), Tag::BlankNode)),
            "ISLITERAL" => Val::Bool(format!(
                "(NOT ({}) AND NOT ({}))",
                tag_test(&need(0)?.raw(), Tag::Iri),
                tag_test(&need(0)?.raw(), Tag::BlankNode)
            )),
            "REGEX" => {
                if !self.caps.udf {
                    return Err(DatalogError::unsupported(
                        "REGEX needs the oxilite user-defined functions, which this backend does \
                         not provide",
                    ));
                }
                let flags = match a.get(2) {
                    Some(f) => f.as_text(),
                    None => "''".to_owned(),
                };
                Val::Bool(format!(
                    "oxilite_regex({}, {}, {flags})",
                    need(0)?.as_text(),
                    need(1)?.as_text()
                ))
            }
            "IF" => Val::Num(format!(
                "(CASE WHEN {} THEN {} ELSE {} END)",
                need(0)?.as_bool(),
                need(1)?.as_num(),
                need(2)?.as_num()
            )),
            "COALESCE" => Val::Num(format!(
                "COALESCE({})",
                a.iter().map(Val::as_num).collect::<Vec<_>>().join(", ")
            )),
            other => {
                return Err(DatalogError::unsupported(format!(
                    "the function {other} is not available in a Datalog constraint"
                )))
            }
        })
    }

    // ---------------------------------------------------------------- goal

    fn goal_select(&mut self, goal: &Goal) -> Result<(String, Vec<String>)> {
        let mut frame = Frame::default();
        self.bind_atom(&goal.atom, &mut frame, None)?;
        for c in &goal.constraints {
            let sql = self.expr(c, &frame)?.as_bool();
            frame.wheres.push(format!("({sql})"));
        }
        let mut variables = Vec::new();
        atom_vars(&goal.atom, &mut variables);
        // Columns are aliased `v0…` so that a caller — materialization, for one — can wrap
        // the goal in another SELECT and address its columns by name.
        let projection: Vec<String> = variables
            .iter()
            .enumerate()
            .map(|(i, v)| format!("{} AS v{i}", frame.binding[v]))
            .collect();
        let projection = if projection.is_empty() {
            // A ground goal is a yes/no question; one constant column answers it.
            vec!["1 AS v0".to_owned()]
        } else {
            projection
        };
        let mut sql = format!(
            "SELECT DISTINCT {} FROM {}",
            projection.join(", "),
            frame.froms.join(", ")
        );
        if !frame.wheres.is_empty() {
            let _ = write!(sql, " WHERE {}", frame.wheres.join(" AND "));
        }
        Ok((sql, variables))
    }
}

/// What a rule body has bound so far.
#[derive(Debug, Default)]
struct Frame {
    froms: Vec<String>,
    wheres: Vec<String>,
    binding: HashMap<String, String>,
}

/// The tagged relation of a mutually recursive component: its name, the component, the tag
/// being compiled, and the component's column width.
type SccCtx<'a> = (&'a str, &'a Stratum, usize, usize);

/// A compiled expression, tagged with what its SQL actually produces.
///
/// Relations hold term ids, but arithmetic and string functions produce plain SQL values, so
/// the two must not be confused: decoding an already-decoded value would compare a number
/// against the inline-integer id range.
#[derive(Debug, Clone)]
enum Val {
    /// A tagged 64-bit term id.
    Id(String),
    /// A term written in the program text: its id, and the lexical form it was written with.
    /// A constant need not exist in the store, so its text can never be looked up there.
    Const { id: String, lex: String },
    /// A number, or a timestamp, already decoded.
    Num(String),
    /// A SQL boolean.
    Bool(String),
    /// A lexical form.
    Text(String),
}

impl Val {
    /// The SQL as written, whatever it produces.
    fn raw(&self) -> String {
        match self {
            Self::Id(s) | Self::Num(s) | Self::Bool(s) | Self::Text(s) => s.clone(),
            Self::Const { id, .. } => id.clone(),
        }
    }

    /// Is this a term, rather than a computed value? Two terms compare by id.
    fn is_term(&self) -> bool {
        matches!(self, Self::Id(_) | Self::Const { .. })
    }

    /// As a number: a term id is decoded, anything else already is one.
    fn as_num(&self) -> String {
        match self {
            Self::Id(s) | Self::Const { id: s, .. } => numeric(s),
            Self::Text(s) => format!("CAST({s} AS REAL)"),
            Self::Num(s) | Self::Bool(s) => s.clone(),
        }
    }

    /// As a lexical form.
    fn as_text(&self) -> String {
        match self {
            Self::Id(s) => lexical(s),
            Self::Const { lex, .. } => sql_string(lex),
            Self::Text(s) => s.clone(),
            Self::Num(s) | Self::Bool(s) => format!("CAST({s} AS TEXT)"),
        }
    }

    /// As a truth value. A bare term id is true when it is the boolean `true`.
    fn as_bool(&self) -> String {
        match self {
            Self::Bool(s) => s.clone(),
            Self::Id(s) | Self::Const { id: s, .. } => {
                format!("(({s}) = {})", oxilite_core::encoding::boolean_id(true))
            }
            Self::Num(s) => format!("(({s}) <> 0)"),
            Self::Text(s) => format!("(({s}) <> '')"),
        }
    }
}

/// Tests the tag bits of a term id.
fn tag_test(id: &str, tag: Tag) -> String {
    format!(
        "(({id}) BETWEEN {lo} AND {hi})",
        lo = tag.base(),
        hi = tag.base() | PAYLOAD_MASK
    )
}

/// The lexical form of a term, which is what SPARQL's `STR` returns.
fn lexical_form(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_owned(),
        Term::BlankNode(b) => b.as_str().to_owned(),
        Term::Literal(l) => l.value().to_owned(),
        other => other.to_string(),
    }
}

/// Quotes a string as a SQL literal.
fn sql_string(s: &str) -> String {
    let mut out = String::new();
    oxilite_core::sql::quote_str(&mut out, s);
    out
}

/// The numeric value of a literal written in the program text, if it has one.
fn numeric_literal(t: &Term) -> Option<String> {
    let Term::Literal(l) = t else { return None };
    oxilite_core::encoding::numeric_rank(l.datatype().as_str())?;
    let v: f64 = l.value().parse().ok()?;
    Some(if v == v.trunc() && v.abs() < 9e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    })
}

/// The numeric (or temporal) value of a term id.
///
/// Inline integers carry their value in the id, so they need no lookup; anything else reads
/// the typed side columns of `terms`, which are indexed. A term has either `num` or `ts`, so
/// the `COALESCE` picks whichever exists.
fn numeric(id: &str) -> String {
    format!(
        "(CASE WHEN ({id}) BETWEEN {lo} AND {hi} THEN ({id}) - {zero} \
         ELSE (SELECT COALESCE(num, ts) FROM terms WHERE id = ({id})) END)",
        lo = int_lo(),
        hi = int_hi(),
        zero = int_zero()
    )
}

/// The lexical form of a term id.
fn lexical(id: &str) -> String {
    format!(
        "(CASE WHEN ({id}) BETWEEN {lo} AND {hi} THEN CAST(({id}) - {zero} AS TEXT) \
         ELSE (SELECT lex FROM terms WHERE id = ({id})) END)",
        lo = int_lo(),
        hi = int_hi(),
        zero = int_zero()
    )
}

/// Widens one arm of a tagged recursive member to the component's column count.
fn pad_select(select: &str, tag: usize, arity: usize, width: usize) -> String {
    let rest = select
        .strip_prefix("SELECT ")
        .expect("every arm is built as a SELECT");
    if arity == width {
        return format!("SELECT {tag}, {rest}");
    }
    // Split the projection from the rest of the statement at the top-level ` FROM `.
    let (proj, tail) = split_projection(rest);
    let nulls: String = (arity..width).map(|_| ", NULL").collect();
    format!("SELECT {tag}, {proj}{nulls}{tail}")
}

/// Splits `a, b FROM t WHERE …` into its projection and its tail, ignoring `FROM` inside
/// parentheses or string literals.
fn split_projection(s: &str) -> (&str, &str) {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => in_str = !in_str,
            b'(' if !in_str => depth += 1,
            b')' if !in_str => depth -= 1,
            b' ' if !in_str
                && depth == 0
                && bytes.len() >= i + 6
                && bytes[i..i + 6].eq_ignore_ascii_case(b" FROM ") =>
            {
                return (&s[..i], &s[i..]);
            }
            _ => {}
        }
        i += 1;
    }
    (s, "")
}
