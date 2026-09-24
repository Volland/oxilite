//! "Why?": the justification of an entailed triple, as a proof tree. Asserted triples are
//! leaves (with their source line); an inference is explained by the rule that derives it and
//! the premises that rule matched, recursively. OWL 2 RL and RDFS conclusions are explained by
//! rule templates run as SPARQL with the conclusion bound; rule-file conclusions by re-running
//! the rule's body in the Datalog engine with its head bound.
//!
// @lat: [[architecture#Studio server#Justifications]]

use super::conn::Target;
use super::index::SourceIndex;
use oxilite::datalog::ast::{Arg, BinOp, BodyItem, Expr, HeadArg, Pred, Rule};
use oxilite::model::{GraphName, NamedNode, NamedOrBlankNode, Quad, Term, Triple};
use oxilite::sparql::SparqlParser;
use oxilite_core::QueryOutput;
use serde_json::{json, Value};
use std::collections::HashSet;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const MAX_DEPTH: usize = 8;

/// One OWL 2 RL / RDFS rule as a template: which conclusions it can explain, and a SPARQL
/// pattern over the conclusion's `?s ?p ?o` whose solutions give the premises.
struct Template {
    name: &'static str,
    /// The conclusion's predicate the rule applies to (`None`: any predicate).
    predicate: Option<&'static str>,
    pattern: &'static str,
    premises: &'static [(&'static str, &'static str, &'static str)],
}

const TEMPLATES: &[Template] = &[
    Template { name: "cax-sco (subclass)", predicate: Some(RDF_TYPE),
        pattern: "?s a ?c . ?c rdfs:subClassOf ?o . FILTER(?c != ?o)",
        premises: &[("?s", "rdf:type", "?c"), ("?c", "rdfs:subClassOf", "?o")] },
    Template { name: "cax-eqc (equivalent class)", predicate: Some(RDF_TYPE),
        pattern: "?s a ?c . { ?c owl:equivalentClass ?o } UNION { ?o owl:equivalentClass ?c } FILTER(?c != ?o)",
        premises: &[("?s", "rdf:type", "?c"), ("?c", "owl:equivalentClass", "?o")] },
    Template { name: "prp-dom (domain)", predicate: Some(RDF_TYPE),
        pattern: "?s ?q ?y . ?q rdfs:domain ?o",
        premises: &[("?s", "?q", "?y"), ("?q", "rdfs:domain", "?o")] },
    Template { name: "prp-rng (range)", predicate: Some(RDF_TYPE),
        pattern: "?y ?q ?s . ?q rdfs:range ?o",
        premises: &[("?y", "?q", "?s"), ("?q", "rdfs:range", "?o")] },
    Template { name: "scm-sco (subclass transitivity)", predicate: Some("http://www.w3.org/2000/01/rdf-schema#subClassOf"),
        pattern: "?s rdfs:subClassOf ?c . ?c rdfs:subClassOf ?o . FILTER(?c != ?s && ?c != ?o)",
        premises: &[("?s", "rdfs:subClassOf", "?c"), ("?c", "rdfs:subClassOf", "?o")] },
    Template { name: "prp-spo1 (subproperty)", predicate: None,
        pattern: "?s ?q ?o . ?q rdfs:subPropertyOf ?p . FILTER(?q != ?p)",
        premises: &[("?s", "?q", "?o"), ("?q", "rdfs:subPropertyOf", "?p")] },
    Template { name: "prp-eqp (equivalent property)", predicate: None,
        pattern: "?s ?q ?o . { ?q owl:equivalentProperty ?p } UNION { ?p owl:equivalentProperty ?q } FILTER(?q != ?p)",
        premises: &[("?s", "?q", "?o"), ("?q", "owl:equivalentProperty", "?p")] },
    Template { name: "prp-inv (inverse)", predicate: None,
        pattern: "?o ?q ?s . { ?q owl:inverseOf ?p } UNION { ?p owl:inverseOf ?q }",
        premises: &[("?o", "?q", "?s"), ("?q", "owl:inverseOf", "?p")] },
    Template { name: "prp-symp (symmetric)", predicate: None,
        pattern: "?o ?p ?s . ?p a owl:SymmetricProperty . FILTER(?s != ?o)",
        premises: &[("?o", "?p", "?s"), ("?p", "rdf:type", "owl:SymmetricProperty")] },
    Template { name: "prp-trp (transitive)", predicate: None,
        pattern: "?s ?p ?y . ?y ?p ?o . ?p a owl:TransitiveProperty . FILTER(?y != ?s && ?y != ?o)",
        premises: &[("?s", "?p", "?y"), ("?y", "?p", "?o"), ("?p", "rdf:type", "owl:TransitiveProperty")] },
    Template { name: "eq-rep-s (sameAs subject)", predicate: None,
        pattern: "{ ?x owl:sameAs ?s } UNION { ?s owl:sameAs ?x } ?x ?p ?o . FILTER(?x != ?s)",
        premises: &[("?x", "owl:sameAs", "?s"), ("?x", "?p", "?o")] },
    Template { name: "eq-rep-o (sameAs object)", predicate: None,
        pattern: "{ ?x owl:sameAs ?o } UNION { ?o owl:sameAs ?x } ?s ?p ?x . FILTER(?x != ?o)",
        premises: &[("?x", "owl:sameAs", "?o"), ("?s", "?p", "?x")] },
    Template { name: "eq-sym (sameAs symmetry)", predicate: Some("http://www.w3.org/2002/07/owl#sameAs"),
        pattern: "?o owl:sameAs ?s",
        premises: &[("?o", "owl:sameAs", "?s")] },
];

const PREFIXES: &str = "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> PREFIX owl: <http://www.w3.org/2002/07/owl#> ";

pub struct Explainer<'a> {
    pub target: &'a Target,
    pub index: &'a SourceIndex,
    /// Rule files by producer name (relative path) with their text.
    pub rules: Vec<(String, String, String)>,
}

impl Explainer<'_> {
    /// The proof tree of a triple.
    pub fn why(&self, t: &Triple) -> Result<Value> {
        let mut path = HashSet::new();
        self.node(t, 0, &mut path)
    }

    fn asserted(&self, t: &Triple) -> Result<bool> {
        self.target
            .store
            .contains_any(t.subject.as_ref(), t.predicate.as_ref(), t.object.as_ref())
    }

    fn holds(&self, t: &Triple) -> Result<bool> {
        let q = format!("ASK {{ {} {} {} }}", t.subject, t.predicate, t.object);
        Ok(matches!(
            self.target
                .store
                .query_output(SparqlParser::new().parse_query(&q)?, &self.target.options)?,
            QueryOutput::Boolean(true)
        ))
    }

    fn node(&self, t: &Triple, depth: usize, path: &mut HashSet<Triple>) -> Result<Value> {
        let triple = json!({
            "s": oxilite_core::json::term_to_json(&t.subject.clone().into()),
            "p": oxilite_core::json::term_to_json(&t.predicate.clone().into()),
            "o": oxilite_core::json::term_to_json(&t.object),
        });
        if self.asserted(t)? {
            let at = match &t.subject {
                NamedOrBlankNode::NamedNode(s) => self
                    .index
                    .triple_location(s.as_str(), Some(t.predicate.as_str())),
                NamedOrBlankNode::BlankNode(_) => None,
            };
            return Ok(json!({
                "triple": triple,
                "status": "asserted",
                "location": at.map(|l| json!({"uri": l.uri, "range": {"start": {"line": l.start.line, "character": l.start.col}, "end": {"line": l.end.line, "character": l.end.col}}})),
            }));
        }
        if !self.holds(t)? {
            return Ok(json!({"triple": triple, "status": "absent"}));
        }
        if depth >= MAX_DEPTH || !path.insert(t.clone()) {
            return Ok(json!({"triple": triple, "status": "inferred", "note": "explained above"}));
        }
        let producers = self.target.store.inference_producers(&Quad::new(
            t.subject.clone(),
            t.predicate.clone(),
            t.object.clone(),
            GraphName::DefaultGraph,
        ))?;
        let mut result = None;
        // A rule file first: its conclusions are what the user wrote rules for.
        for (producer, _uri, text) in &self.rules {
            if !producers.is_empty() && !producers.contains(producer) {
                continue;
            }
            if let Some((rule, premises)) = self.by_rule(text, t)? {
                let children = premises
                    .iter()
                    .map(|p| self.node(p, depth + 1, path))
                    .collect::<Result<Vec<_>>>()?;
                result = Some(
                    json!({"triple": triple, "status": "inferred", "producer": producer, "rule": rule, "premises": children}),
                );
                break;
            }
        }
        if result.is_none() {
            for template in TEMPLATES {
                if let Some(premises) = self.by_template(template, t)? {
                    let children = premises
                        .iter()
                        .map(|p| self.node(p, depth + 1, path))
                        .collect::<Result<Vec<_>>>()?;
                    let producer = if producers.iter().any(|p| p == "owl2rl") {
                        "owl2rl"
                    } else {
                        "query-time reasoning"
                    };
                    result = Some(
                        json!({"triple": triple, "status": "inferred", "producer": producer, "rule": template.name, "premises": children}),
                    );
                    break;
                }
            }
        }
        path.remove(t);
        Ok(result.unwrap_or_else(|| {
            json!({"triple": triple, "status": "inferred", "producers": producers, "note": "no rule template matched this conclusion"})
        }))
    }

    /// Premises of an OWL/RDFS template for `t`, if it applies.
    fn by_template(&self, template: &Template, t: &Triple) -> Result<Option<Vec<Triple>>> {
        if template
            .predicate
            .is_some_and(|p| p != t.predicate.as_str())
        {
            return Ok(None);
        }
        let values = format!(
            "VALUES (?s ?p ?o) {{ ({} {} {}) }}",
            t.subject, t.predicate, t.object
        );
        let vars = ["?c", "?q", "?x", "?y"];
        let q = format!(
            "{PREFIXES}SELECT * WHERE {{ {values} {} }} LIMIT 20",
            template.pattern
        );
        let QueryOutput::Solutions { variables, rows } = self
            .target
            .store
            .query_output(SparqlParser::new().parse_query(&q)?, &self.target.options)?
        else {
            return Ok(None);
        };
        let _ = vars;
        'rows: for row in rows {
            let get = |name: &str| -> Option<Term> {
                match name {
                    "rdf:type" => Some(NamedNode::new_unchecked(RDF_TYPE).into()),
                    "owl:SymmetricProperty" => Some(
                        NamedNode::new_unchecked("http://www.w3.org/2002/07/owl#SymmetricProperty")
                            .into(),
                    ),
                    "owl:TransitiveProperty" => Some(
                        NamedNode::new_unchecked(
                            "http://www.w3.org/2002/07/owl#TransitiveProperty",
                        )
                        .into(),
                    ),
                    n if n.starts_with('?') => {
                        let i = variables.iter().position(|v| v.as_str() == &n[1..])?;
                        row[i].clone()
                    }
                    n => {
                        let (prefix, local) = n.split_once(':')?;
                        let ns = match prefix {
                            "rdfs" => "http://www.w3.org/2000/01/rdf-schema#",
                            "owl" => "http://www.w3.org/2002/07/owl#",
                            _ => return None,
                        };
                        Some(NamedNode::new_unchecked(format!("{ns}{local}")).into())
                    }
                }
            };
            let mut premises = Vec::new();
            for (s, p, o) in template.premises {
                let (Some(s), Some(Term::NamedNode(p)), Some(o)) = (get(s), get(p), get(o)) else {
                    continue 'rows;
                };
                let Ok(s) = to_subject(s) else { continue 'rows };
                let premise = Triple::new(s, p, o);
                if premise == *t {
                    continue 'rows;
                }
                premises.push(premise);
            }
            return Ok(Some(premises));
        }
        Ok(None)
    }

    /// The first rule of a program that derives `t`, with the premises its body matched.
    fn by_rule(&self, text: &str, t: &Triple) -> Result<Option<(String, Vec<Triple>)>> {
        let program = oxilite::datalog::parse(text)?;
        for rule in &program.rules {
            let Some(bound) = head_binding(rule, t) else {
                continue;
            };
            let free: Vec<String> = body_vars(rule)
                .into_iter()
                .filter(|v| !bound.iter().any(|(b, _)| b == v))
                .collect();
            let subst = |v: &str| bound.iter().find(|(b, _)| b == v).map(|(_, t)| t.clone());
            let mut solutions = vec![Vec::<(String, Term)>::new()];
            if !free.is_empty() {
                // Re-run the body with the head bound: the goal's answers are the premises' bindings.
                let body: Vec<String> = rule.body.iter().map(|b| body_text(b, &subst)).collect();
                let args: Vec<String> = free.iter().map(|v| format!("?{v}")).collect();
                let query = format!(
                    "{text}\nwhy_premises({args}) :- {body}.\n?- why_premises({args}).\n",
                    args = args.join(", "),
                    body = body.join(", ")
                );
                let options = oxilite::datalog::Options {
                    union_default_graph: self.target.options.union_default_graph,
                    include_inferred: true,
                    ..Default::default()
                };
                let r = self.target.store.datalog_with(&query, &options)?;
                solutions = r
                    .rows
                    .into_iter()
                    .take(1)
                    .map(|row| {
                        r.variables
                            .iter()
                            .cloned()
                            .zip(row)
                            .filter_map(|(v, t)| Some((v, t?)))
                            .collect()
                    })
                    .collect();
            }
            let Some(solution) = solutions.into_iter().next() else {
                continue;
            };
            let value = |v: &str| {
                subst(v).or_else(|| {
                    solution
                        .iter()
                        .find(|(n, _)| n == v)
                        .map(|(_, t)| t.clone())
                })
            };
            let premises: Vec<Triple> = rule
                .body
                .iter()
                .filter_map(|b| match b {
                    BodyItem::Atom(a) => atom_triple(&a.pred, &a.args, &value),
                    _ => None,
                })
                .collect();
            // A rule explains the triple only if its premises hold: with every body variable
            // bound by the head nothing was checked yet, so check now and try the next rule.
            let mut hold = true;
            for p in &premises {
                if !self.holds(p)? {
                    hold = false;
                    break;
                }
            }
            if !hold {
                continue;
            }
            return Ok(Some((rule_text(rule, &|_| None), premises)));
        }
        Ok(None)
    }
}

fn to_subject(t: Term) -> Result<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Ok(n.into()),
        Term::BlankNode(b) => Ok(b.into()),
        _ => Err("a literal cannot be a subject".into()),
    }
}

/// The variables of a rule head bound to `t`'s terms, if the head can derive `t`.
fn head_binding(rule: &Rule, t: &Triple) -> Option<Vec<(String, Term)>> {
    let args: Vec<&Arg> = rule
        .head
        .args
        .iter()
        .map(|a| match a {
            HeadArg::Plain(a) => Some(a),
            HeadArg::Agg { .. } => None,
        })
        .collect::<Option<_>>()?;
    let values: Vec<Term> = match (&rule.head.pred, args.len()) {
        (Pred::Edb(c), 1)
            if t.predicate.as_str() == RDF_TYPE && Term::NamedNode(c.clone()) == t.object =>
        {
            vec![t.subject.clone().into()]
        }
        (Pred::Edb(p), 2) if p == &t.predicate => vec![t.subject.clone().into(), t.object.clone()],
        (Pred::Triple { .. }, 3 | 4) => vec![
            t.subject.clone().into(),
            t.predicate.clone().into(),
            t.object.clone(),
        ],
        _ => return None,
    };
    let mut bound: Vec<(String, Term)> = Vec::new();
    for (a, v) in args.iter().zip(values) {
        match a {
            Arg::Var(name) => match bound.iter().find(|(n, _)| n == name) {
                Some((_, prev)) if *prev != v => return None,
                Some(_) => {}
                None => bound.push((name.clone(), v)),
            },
            Arg::Const(c) if *c != v => return None,
            _ => {}
        }
    }
    Some(bound)
}

fn body_vars(rule: &Rule) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in &rule.body {
        if let BodyItem::Atom(a) = b {
            for arg in &a.args {
                if let Arg::Var(v) = arg {
                    if !out.contains(v) {
                        out.push(v.clone());
                    }
                }
            }
        }
    }
    out
}

fn atom_triple(pred: &Pred, args: &[Arg], value: &impl Fn(&str) -> Option<Term>) -> Option<Triple> {
    let term = |a: &Arg| match a {
        Arg::Var(v) => value(v),
        Arg::Const(c) => Some(c.clone()),
        Arg::Wildcard => None,
    };
    match (pred, args.len()) {
        (Pred::Edb(c), 1) => Some(Triple::new(
            to_subject(term(&args[0])?).ok()?,
            NamedNode::new_unchecked(RDF_TYPE),
            Term::NamedNode(c.clone()),
        )),
        (Pred::Edb(p), 2) => Some(Triple::new(
            to_subject(term(&args[0])?).ok()?,
            p.clone(),
            term(&args[1])?,
        )),
        (Pred::Triple { .. }, 3 | 4) => {
            let Term::NamedNode(p) = term(&args[1])? else {
                return None;
            };
            Some(Triple::new(
                to_subject(term(&args[0])?).ok()?,
                p,
                term(&args[2])?,
            ))
        }
        _ => None,
    }
}

// Printing rules back as Datalog text, with some variables replaced by constants.

fn pred_text(p: &Pred) -> String {
    match p {
        Pred::Edb(iri) => format!("<{}>", iri.as_str()),
        Pred::Idb(name) => name.clone(),
        Pred::Triple { graph: false } => "triple".into(),
        Pred::Triple { graph: true } => "quad".into(),
    }
}

fn arg_text(a: &Arg, subst: &impl Fn(&str) -> Option<Term>) -> String {
    match a {
        Arg::Var(v) => subst(v).map_or_else(|| format!("?{v}"), |t| t.to_string()),
        Arg::Wildcard => "_".into(),
        Arg::Const(t) => t.to_string(),
    }
}

fn expr_text(e: &Expr, subst: &impl Fn(&str) -> Option<Term>) -> String {
    match e {
        Expr::Var(v) => subst(v).map_or_else(|| format!("?{v}"), |t| t.to_string()),
        Expr::Const(t) => t.to_string(),
        Expr::Binary { op, left, right } => {
            let op = match op {
                BinOp::Eq => "=",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::And => "&&",
                BinOp::Or => "||",
            };
            format!(
                "({} {op} {})",
                expr_text(left, subst),
                expr_text(right, subst)
            )
        }
        Expr::Not(x) => format!("!({})", expr_text(x, subst)),
        Expr::Neg(x) => format!("-({})", expr_text(x, subst)),
        Expr::Call { name, args } => format!(
            "{name}({})",
            args.iter()
                .map(|a| expr_text(a, subst))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub(super) fn body_text(b: &BodyItem, subst: &impl Fn(&str) -> Option<Term>) -> String {
    let atom = |pred: &Pred, args: &[Arg]| {
        format!(
            "{}({})",
            pred_text(pred),
            args.iter()
                .map(|a| arg_text(a, subst))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    match b {
        BodyItem::Atom(a) => atom(&a.pred, &a.args),
        BodyItem::Negated(a) => format!("not {}", atom(&a.pred, &a.args)),
        BodyItem::Constraint(e) => expr_text(e, subst),
    }
}

pub(super) fn rule_text(rule: &Rule, subst: &impl Fn(&str) -> Option<Term>) -> String {
    let head_args: Vec<String> = rule
        .head
        .args
        .iter()
        .map(|a| match a {
            HeadArg::Plain(a) => arg_text(a, subst),
            HeadArg::Agg { func, var } => format!("{func:?}(?{var})").to_lowercase(),
        })
        .collect();
    format!(
        "{}({}) :- {}.",
        pred_text(&rule.head.pred),
        head_args.join(", "),
        rule.body
            .iter()
            .map(|b| body_text(b, subst))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
