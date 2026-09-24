//! `oxilite check`: the studio's checks without the editor. Loads a project like the Project
//! store, then reports load and rule errors, SHACL results and knowledge-graph test outcomes;
//! the exit status fails on any error, violation or failing test.
//!
// @lat: [[architecture#Studio server#Check command]]

use super::kgtest::Runner;
use super::project::Project;
use super::validate;
use rudof_rdf::rdf_core::RDFFormat;
use serde_json::{json, Value};
use std::path::PathBuf;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Everything `oxilite check` found, and whether it passes.
pub fn check(root: PathBuf) -> Result<(bool, Value)> {
    let root = root.canonicalize()?;
    let project = Project::open(Some(root.clone()), ":memory:")?;
    let rel = |uri: &str| -> String {
        url::Url::parse(uri)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .and_then(|p| p.strip_prefix(&root).ok().map(|p| p.display().to_string()))
            .unwrap_or_else(|| uri.to_string())
    };
    let mut errors = Vec::new();
    if let Some(e) = project.layout_error() {
        errors.push(json!({"file": "oxilite.toml", "line": 1, "message": e}));
    }
    for f in project.files() {
        if let Some(e) = &f.error {
            errors.push(json!({"file": rel(&f.uri), "line": e.start.map_or(1, |s| s.0 + 1), "message": e.message}));
        }
    }
    for (uri, e) in project.reasoning_errors() {
        errors.push(json!({"file": rel(uri), "line": 1, "message": e}));
    }
    let mut violations = Vec::new();
    let mut conforms = Value::Null;
    if project.has_shacl() && project.validation_enabled() {
        if let Some(report) = validate::report(
            project.store(),
            &project.options(),
            &project.shapes_ntriples(),
            &RDFFormat::NTriples,
            project.validate_inferred(),
            &|| false,
        )? {
            conforms = json!(report.conforms());
            for r in report.results() {
                let focus = r.focus_node().to_string();
                let path = r.path().map(|p| p.to_string());
                let severity = r.severity().to_string();
                let at = match r.focus_node() {
                    rudof_rdf::rdf_core::term::Object::Iri(i) => project.index().triple_location(
                        i.as_str(),
                        path.as_deref()
                            .map(|p| p.trim_matches(|c| c == '<' || c == '>')),
                    ),
                    _ => None,
                };
                violations.push(json!({
                    "severity": severity,
                    "focus": focus,
                    "path": path,
                    "message": r.message().iter().next().map(|(_, m)| m.clone()).unwrap_or_else(|| r.constraint_component().to_string()),
                    "file": at.as_ref().map(|l| rel(&l.uri)),
                    "line": at.as_ref().map(|l| l.start.line + 1),
                }));
            }
        }
    }
    let tests: Vec<Value> = Runner { project: &project }
        .run_all()
        .iter()
        .map(|o| o.to_json())
        .collect();
    let failing_violations = violations
        .iter()
        .filter(|v| {
            v["severity"]
                .as_str()
                .is_some_and(|s| s.to_ascii_lowercase().contains("violation"))
        })
        .count();
    let passed =
        errors.is_empty() && failing_violations == 0 && tests.iter().all(|t| t["passed"] == true);
    Ok((
        passed,
        json!({
            "root": root.display().to_string(),
            "profile": project.profile().name(),
            "files": project.files().len(),
            "triples": project.store().len()?,
            "errors": errors,
            "conforms": conforms,
            "violations": violations,
            "tests": tests,
            "passed": passed,
        }),
    ))
}

/// The report as text for a terminal.
pub fn render(v: &Value) -> String {
    let mut out = format!(
        "oxilite check: {} files, {} triples, reasoning {}\n",
        v["files"],
        v["triples"],
        v["profile"].as_str().unwrap_or("none")
    );
    for e in v["errors"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "error   {}:{}: {}\n",
            e["file"].as_str().unwrap_or(""),
            e["line"],
            e["message"].as_str().unwrap_or("")
        ));
    }
    for r in v["violations"].as_array().into_iter().flatten() {
        let at = match (r["file"].as_str(), r["line"].as_u64()) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            _ => r["focus"].as_str().unwrap_or("").to_string(),
        };
        out.push_str(&format!(
            "shacl   {at}: {} ({})\n",
            r["message"].as_str().unwrap_or(""),
            r["severity"].as_str().unwrap_or("")
        ));
    }
    for t in v["tests"].as_array().into_iter().flatten() {
        let mark = if t["passed"] == true { "pass" } else { "FAIL" };
        out.push_str(&format!(
            "{mark}    {}: {}\n",
            t["name"].as_str().unwrap_or(""),
            t["message"]
                .as_str()
                .unwrap_or("")
                .replace('\n', "\n        ")
        ));
    }
    out.push_str(if v["passed"] == true {
        "ok\n"
    } else {
        "FAILED\n"
    });
    out
}
