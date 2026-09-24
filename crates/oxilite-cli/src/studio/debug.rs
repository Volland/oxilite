//! The Datalog debugger: for each rule of a program, how many ways its body matches and how many
//! facts its head predicate holds, so an empty or exploding rule stands out, with the strata
//! and strategies the engine chose.
//!
// @lat: [[architecture#Studio server#Datalog debugger]]

use super::conn::Target;
use super::why::{body_text, rule_text};
use oxilite::datalog::ast::{Arg, BodyItem, HeadArg};
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Body matches per rule are counted up to this many.
const CAP: usize = 100_000;

pub fn debug(target: &Target, text: &str) -> Result<Value> {
    let program = oxilite::datalog::parse(text)?;
    let options = oxilite::datalog::Options {
        union_default_graph: target.options.union_default_graph,
        include_inferred: target.options.include_inferred,
        max_iterations: 100,
        ..Default::default()
    };
    let count = |query: String| -> std::result::Result<usize, String> {
        target
            .store
            .datalog_with(&query, &options)
            .map(|r| r.rows.len().min(CAP))
            .map_err(|e| e.to_string())
    };
    // The program without its goal: the debugger adds its own.
    let rules_only: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("?-"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut rules = Vec::new();
    for (i, rule) in program.rules.iter().enumerate() {
        let mut vars: Vec<String> = Vec::new();
        for b in &rule.body {
            if let BodyItem::Atom(a) = b {
                for arg in &a.args {
                    if let Arg::Var(v) = arg {
                        if !vars.contains(v) {
                            vars.push(v.clone());
                        }
                    }
                }
            }
        }
        let head_arity = rule.head.args.len();
        let aggregate = rule
            .head
            .args
            .iter()
            .any(|a| matches!(a, HeadArg::Agg { .. }));
        let body = rule
            .body
            .iter()
            .map(|b| body_text(b, &|_| None))
            .collect::<Vec<_>>()
            .join(", ");
        let matches = if vars.is_empty() {
            Ok(1)
        } else {
            let args = vars
                .iter()
                .map(|v| format!("?{v}"))
                .collect::<Vec<_>>()
                .join(", ");
            count(format!(
                "{rules_only}\ndebug_body_{i}({args}) :- {body}.\n?- debug_body_{i}({args}).\n"
            ))
        };
        let head_pred = rule.head.pred.to_string();
        let facts = if aggregate {
            Err("aggregate head".to_string())
        } else {
            let args = (0..head_arity)
                .map(|k| format!("?h{k}"))
                .collect::<Vec<_>>()
                .join(", ");
            let pred = match &rule.head.pred {
                oxilite::datalog::ast::Pred::Edb(iri) => format!("<{}>", iri.as_str()),
                other => other.to_string(),
            };
            count(format!("{rules_only}\n?- {pred}({args}).\n"))
        };
        rules.push(json!({
            "index": i,
            "line": rule.head.span.line.saturating_sub(1),
            "head": head_pred,
            "rule": rule_text(rule, &|_| None),
            "bodyMatches": matches.as_ref().ok(),
            "facts": facts.as_ref().ok(),
            "error": matches.err().or(facts.err()),
        }));
    }
    let plan = target
        .store
        .explain_datalog(&program_with_goal(text, &program))
        .unwrap_or_else(|e| e.to_string());
    Ok(json!({"rules": rules, "plan": plan, "cap": CAP}))
}

/// Explain needs a goal: the program's own, else one over its first rule's head.
fn program_with_goal(text: &str, program: &oxilite::datalog::Program) -> String {
    if text.lines().any(|l| l.trim_start().starts_with("?-")) {
        return text.to_string();
    }
    let Some(rule) = program.rules.first() else {
        return text.to_string();
    };
    let args = (0..rule.head.args.len())
        .map(|k| format!("?h{k}"))
        .collect::<Vec<_>>()
        .join(", ");
    let pred = match &rule.head.pred {
        oxilite::datalog::ast::Pred::Edb(iri) => format!("<{}>", iri.as_str()),
        other => other.to_string(),
    };
    format!("{text}\n?- {pred}({args}).\n")
}
