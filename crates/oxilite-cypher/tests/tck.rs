//! The openCypher TCK (`testsuite/openCypher/tck`) on oxilite.
//!
//! A small Gherkin runner: scenarios and scenario outlines, `having executed`, parameters,
//! expected result tables written as Cypher literals, expected errors, and side effects
//! computed by diffing the graph before and after the query (as the TCK defines them).
//! Scenarios failing today are listed in `tck-allowlist.txt` with a reason; the test fails
//! when a listed scenario starts passing (so the list shrinks) or an unlisted one fails.
//!
//! `OXILITE_TCK_REPORT=1` prints every failure; `OXILITE_TCK_FILTER=<substring>` runs a subset.

use oxilite::cypher::{CypherError, CypherResult, Params, Value};
use oxilite::store::Store;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ----- Gherkin -----

#[derive(Debug, Clone)]
enum StepArg {
    None,
    Doc(String),
    Table(Vec<Vec<String>>),
}

#[derive(Debug, Clone)]
struct GStep {
    text: String,
    arg: StepArg,
}

#[derive(Debug, Clone)]
struct Scenario {
    feature: String,
    name: String,
    steps: Vec<GStep>,
}

fn parse_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    // Cells are separated by unescaped pipes.
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '|' => {
                cells.push(cur.trim().to_string());
                cur.clear();
            }
            c => cur.push(c),
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

fn parse_feature(path: &Path) -> Vec<Scenario> {
    let text = std::fs::read_to_string(path).unwrap();
    let feature = path
        .strip_prefix(tck_dir().join("features"))
        .unwrap_or(path)
        .display()
        .to_string();
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut background: Vec<GStep> = Vec::new();
    while i < lines.len() {
        let l = lines[i].trim();
        let (is_outline, name) = if let Some(n) = l.strip_prefix("Scenario Outline:") {
            (true, n.trim().to_string())
        } else if let Some(n) = l.strip_prefix("Scenario:") {
            (false, n.trim().to_string())
        } else if l.starts_with("Background:") {
            i += 1;
            background = parse_steps(&lines, &mut i);
            continue;
        } else {
            i += 1;
            continue;
        };
        i += 1;
        let mut steps = background.clone();
        steps.extend(parse_steps(&lines, &mut i));
        if !is_outline {
            out.push(Scenario {
                feature: feature.clone(),
                name,
                steps,
            });
            continue;
        }
        // Examples tables.
        while i < lines.len() {
            let l = lines[i].trim();
            if l.starts_with("Examples:") {
                i += 1;
                let mut rows = Vec::new();
                while i < lines.len() && lines[i].trim().starts_with('|') {
                    rows.push(parse_table_row(lines[i]));
                    i += 1;
                }
                if rows.is_empty() {
                    continue;
                }
                let header = rows.remove(0);
                for (k, row) in rows.iter().enumerate() {
                    let subst = |s: &str| {
                        let mut s = s.to_string();
                        for (h, v) in header.iter().zip(row) {
                            s = s.replace(&format!("<{h}>"), v);
                        }
                        s
                    };
                    let steps = steps
                        .iter()
                        .map(|st| GStep {
                            text: subst(&st.text),
                            arg: match &st.arg {
                                StepArg::None => StepArg::None,
                                StepArg::Doc(d) => StepArg::Doc(subst(d)),
                                StepArg::Table(t) => StepArg::Table(
                                    t.iter()
                                        .map(|r| r.iter().map(|c| subst(c)).collect())
                                        .collect(),
                                ),
                            },
                        })
                        .collect();
                    out.push(Scenario {
                        feature: feature.clone(),
                        name: format!("{name} #{}", k + 1),
                        steps,
                    });
                }
            } else if l.starts_with("Scenario") {
                break;
            } else {
                i += 1;
            }
        }
    }
    out
}

fn parse_steps(lines: &[&str], i: &mut usize) -> Vec<GStep> {
    let mut steps = Vec::new();
    while *i < lines.len() {
        let l = lines[*i].trim();
        let kw = ["Given ", "When ", "Then ", "And ", "But "]
            .iter()
            .find(|k| l.starts_with(**k));
        let Some(kw) = kw else {
            if l.starts_with("Scenario") || l.starts_with("Examples:") || l.starts_with('@') {
                break;
            }
            *i += 1;
            continue;
        };
        let text = l[kw.len()..].trim_end_matches(':').trim().to_string();
        *i += 1;
        let mut arg = StepArg::None;
        if *i < lines.len() && lines[*i].trim().starts_with("\"\"\"") {
            let indent = lines[*i].len() - lines[*i].trim_start().len();
            *i += 1;
            let mut doc = Vec::new();
            while *i < lines.len() && !lines[*i].trim().starts_with("\"\"\"") {
                let line = lines[*i];
                doc.push(if line.len() >= indent {
                    &line[indent..]
                } else {
                    line.trim_start()
                });
                *i += 1;
            }
            *i += 1;
            arg = StepArg::Doc(doc.join("\n"));
        } else if *i < lines.len() && lines[*i].trim().starts_with('|') {
            let mut rows = Vec::new();
            while *i < lines.len() && lines[*i].trim().starts_with('|') {
                rows.push(parse_table_row(lines[*i]));
                *i += 1;
            }
            arg = StepArg::Table(rows);
        }
        steps.push(GStep { text, arg });
    }
    steps
}

// ----- expected values (Cypher literal syntax of the TCK) -----

#[derive(Debug, Clone, PartialEq)]
enum TV {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<TV>),
    Map(BTreeMap<String, TV>),
    Node(BTreeSet<String>, BTreeMap<String, TV>),
    Rel(String, BTreeMap<String, TV>),
    /// Nodes and relationships with their direction (`true` = forward).
    Path(Vec<TV>, Vec<(TV, bool)>),
}

struct VP<'a> {
    s: &'a [u8],
    i: usize,
}

impl VP<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> u8 {
        self.ws();
        self.s.get(self.i).copied().unwrap_or(0)
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == c {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn name(&mut self) -> String {
        self.ws();
        if self.s.get(self.i) == Some(&b'`') {
            self.i += 1;
            let st = self.i;
            while self.i < self.s.len() && self.s[self.i] != b'`' {
                self.i += 1;
            }
            let n = String::from_utf8_lossy(&self.s[st..self.i]).to_string();
            self.i += 1;
            return n;
        }
        let st = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[st..self.i]).to_string()
    }

    fn map(&mut self) -> Option<BTreeMap<String, TV>> {
        let mut m = BTreeMap::new();
        if !self.eat(b'{') {
            return Some(m);
        }
        if self.eat(b'}') {
            return Some(m);
        }
        loop {
            let k = self.name();
            if !self.eat(b':') {
                return None;
            }
            let v = self.value()?;
            m.insert(k, v);
            if self.eat(b',') {
                continue;
            }
            if self.eat(b'}') {
                return Some(m);
            }
            return None;
        }
    }

    fn node(&mut self) -> Option<TV> {
        if !self.eat(b'(') {
            return None;
        }
        let mut labels = BTreeSet::new();
        while self.eat(b':') {
            labels.insert(self.name());
        }
        let props = if self.peek() == b'{' {
            self.map()?
        } else {
            BTreeMap::new()
        };
        if !self.eat(b')') {
            return None;
        }
        Some(TV::Node(labels, props))
    }

    fn rel(&mut self) -> Option<TV> {
        if !self.eat(b'[') {
            return None;
        }
        if !self.eat(b':') {
            return None;
        }
        let t = self.name();
        let props = if self.peek() == b'{' {
            self.map()?
        } else {
            BTreeMap::new()
        };
        if !self.eat(b']') {
            return None;
        }
        Some(TV::Rel(t, props))
    }

    fn value(&mut self) -> Option<TV> {
        match self.peek() {
            b'(' => self.node(),
            b'<' => {
                self.i += 1;
                let mut nodes = vec![self.node()?];
                let mut rels = Vec::new();
                loop {
                    match self.peek() {
                        b'>' => {
                            self.i += 1;
                            return Some(TV::Path(nodes, rels));
                        }
                        b'<' => {
                            self.i += 1;
                            self.eat(b'-');
                            let r = self.rel()?;
                            self.eat(b'-');
                            rels.push((r, false));
                            nodes.push(self.node()?);
                        }
                        b'-' => {
                            self.i += 1;
                            let r = self.rel()?;
                            self.eat(b'-');
                            self.eat(b'>');
                            rels.push((r, true));
                            nodes.push(self.node()?);
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                // A relationship `[:T]` or a list.
                let save = self.i;
                self.i += 1;
                if self.peek() == b':' {
                    self.i = save;
                    return self.rel();
                }
                let mut items = Vec::new();
                if self.eat(b']') {
                    return Some(TV::List(items));
                }
                loop {
                    items.push(self.value()?);
                    if self.eat(b',') {
                        continue;
                    }
                    if self.eat(b']') {
                        return Some(TV::List(items));
                    }
                    return None;
                }
            }
            b'{' => self.map().map(TV::Map),
            b'\'' => {
                self.i += 1;
                let mut out = Vec::new();
                while self.i < self.s.len() && self.s[self.i] != b'\'' {
                    if self.s[self.i] == b'\\' && self.i + 1 < self.s.len() {
                        self.i += 1;
                        match self.s[self.i] {
                            b'n' => out.push(b'\n'),
                            b't' => out.push(b'\t'),
                            c => out.push(c),
                        }
                    } else {
                        out.push(self.s[self.i]);
                    }
                    self.i += 1;
                }
                self.i += 1;
                Some(TV::Str(String::from_utf8_lossy(&out).to_string()))
            }
            _ => {
                let st = self.i;
                while self.i < self.s.len() && !b",]}) \t".contains(&self.s[self.i]) {
                    self.i += 1;
                }
                let tok = std::str::from_utf8(&self.s[st..self.i]).ok()?.to_string();
                match tok.as_str() {
                    "null" => Some(TV::Null),
                    "true" => Some(TV::Bool(true)),
                    "false" => Some(TV::Bool(false)),
                    "NaN" => Some(TV::Float(f64::NAN)),
                    "Infinity" | "+Infinity" => Some(TV::Float(f64::INFINITY)),
                    "-Infinity" => Some(TV::Float(f64::NEG_INFINITY)),
                    t => {
                        if let Ok(i) = t.parse::<i64>() {
                            Some(TV::Int(i))
                        } else if let Some(h) = t.strip_prefix("0x") {
                            i64::from_str_radix(h, 16).ok().map(TV::Int)
                        } else {
                            t.parse::<f64>().ok().map(TV::Float)
                        }
                    }
                }
            }
        }
    }
}

fn parse_tv(s: &str) -> Option<TV> {
    let mut p = VP {
        s: s.as_bytes(),
        i: 0,
    };
    let v = p.value()?;
    p.ws();
    (p.i == s.len()).then_some(v)
}

fn to_tv(v: &Value) -> TV {
    let props =
        |p: &BTreeMap<String, Value>| p.iter().map(|(k, v)| (k.clone(), to_tv(v))).collect();
    match v {
        Value::Null => TV::Null,
        Value::Bool(b) => TV::Bool(*b),
        Value::Int(i) => TV::Int(*i),
        Value::Float(f) => TV::Float(*f),
        Value::String(s) | Value::Temporal(_, s) => TV::Str(s.clone()),
        Value::List(l) => TV::List(l.iter().map(to_tv).collect()),
        Value::Map(m) => TV::Map(props(m)),
        Value::Node(n) => TV::Node(n.labels.iter().cloned().collect(), props(&n.properties)),
        Value::Relationship(r) => TV::Rel(r.rel_type.clone(), props(&r.properties)),
        Value::Path(p) => {
            let nodes = p
                .nodes
                .iter()
                .map(|n| to_tv(&Value::Node(n.clone())))
                .collect();
            let mut rels = Vec::new();
            for (i, r) in p.relationships.iter().enumerate() {
                let forward = p.nodes.get(i).is_some_and(|n| n.id == r.start);
                rels.push((to_tv(&Value::Relationship(r.clone())), forward));
            }
            TV::Path(nodes, rels)
        }
    }
}

fn tv_eq(a: &TV, b: &TV, unordered_lists: bool) -> bool {
    match (a, b) {
        (TV::Float(x), TV::Float(y)) => {
            (x.is_nan() && y.is_nan()) || x == y || ((x - y).abs() <= 1e-9 * x.abs().max(y.abs()))
        }
        (TV::List(x), TV::List(y)) => {
            if x.len() != y.len() {
                return false;
            }
            if unordered_lists {
                multiset_eq(x, y, true)
            } else {
                x.iter().zip(y).all(|(p, q)| tv_eq(p, q, false))
            }
        }
        (TV::Map(x), TV::Map(y))
        | (TV::Node(_, x), TV::Node(_, y))
        | (TV::Rel(_, x), TV::Rel(_, y)) => {
            let labels_ok = match (a, b) {
                (TV::Node(l1, _), TV::Node(l2, _)) => l1 == l2,
                (TV::Rel(t1, _), TV::Rel(t2, _)) => t1 == t2,
                _ => true,
            };
            labels_ok
                && x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| tv_eq(v, w, unordered_lists)))
        }
        (TV::Path(n1, r1), TV::Path(n2, r2)) => {
            n1.len() == n2.len()
                && r1.len() == r2.len()
                && n1.iter().zip(n2).all(|(p, q)| tv_eq(p, q, false))
                && r1
                    .iter()
                    .zip(r2)
                    .all(|((p, d1), (q, d2))| d1 == d2 && tv_eq(p, q, false))
        }
        _ => a == b,
    }
}

fn multiset_eq(x: &[TV], y: &[TV], unordered_lists: bool) -> bool {
    let mut used = vec![false; y.len()];
    'outer: for a in x {
        for (j, b) in y.iter().enumerate() {
            if !used[j] && tv_eq(a, b, unordered_lists) {
                used[j] = true;
                continue 'outer;
            }
        }
        return false;
    }
    true
}

fn rows_eq(actual: &[Vec<TV>], expected: &[Vec<TV>], ordered: bool, unordered_lists: bool) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    let row_eq = |a: &Vec<TV>, b: &Vec<TV>| {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| tv_eq(x, y, unordered_lists))
    };
    if ordered {
        return actual.iter().zip(expected).all(|(a, b)| row_eq(a, b));
    }
    let mut used = vec![false; expected.len()];
    'outer: for a in actual {
        for (j, b) in expected.iter().enumerate() {
            if !used[j] && row_eq(a, b) {
                used[j] = true;
                continue 'outer;
            }
        }
        return false;
    }
    true
}

// ----- graph snapshots for side effects -----

#[derive(Default, Debug)]
struct Snapshot {
    nodes: BTreeSet<String>,
    rels: BTreeSet<String>,
    labels: BTreeSet<String>,
    props: Vec<String>,
}

fn snapshot(store: &Store) -> Snapshot {
    let mut s = Snapshot::default();
    let q = |c: &str| store.cypher(c).map(|r| r.rows).unwrap_or_default();
    for r in q("MATCH (n) RETURN elementId(n)") {
        s.nodes.insert(format!("{:?}", r[0]));
    }
    for r in q("MATCH ()-[r]->() RETURN elementId(r)") {
        s.rels.insert(format!("{:?}", r[0]));
    }
    if let Ok(oxilite::sparql::QueryResults::Solutions(sol)) =
        store.query("SELECT DISTINCT ?l WHERE { ?s a ?l FILTER(?l != <http://www.w3.org/2000/01/rdf-schema#Resource>) }")
    {
        for x in sol.flatten() {
            s.labels.insert(format!("{:?}", x.get("l")));
        }
    }
    if let Ok(oxilite::sparql::QueryResults::Solutions(sol)) =
        store.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o FILTER(isLITERAL(?o)) }")
    {
        for x in sol.flatten() {
            s.props.push(format!(
                "{:?} {:?} {:?}",
                x.get("s"),
                x.get("p"),
                x.get("o")
            ));
        }
    }
    s.props.sort();
    s
}

fn diff_count<T: Ord + Clone>(a: &[T], b: &[T]) -> (i64, i64) {
    let mut plus = 0;
    let mut minus = 0;
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x == y => {
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x < y => {
                minus += 1;
                i += 1;
            }
            (Some(_), Some(_)) => {
                plus += 1;
                j += 1;
            }
            (Some(_), None) => {
                minus += 1;
                i += 1;
            }
            (None, Some(_)) => {
                plus += 1;
                j += 1;
            }
            (None, None) => break,
        }
    }
    (plus, minus)
}

fn side_effects(before: &Snapshot, after: &Snapshot) -> BTreeMap<String, i64> {
    let set = |a: &BTreeSet<String>| a.iter().cloned().collect::<Vec<_>>();
    let mut out = BTreeMap::new();
    for (name, (p, m)) in [
        ("nodes", diff_count(&set(&before.nodes), &set(&after.nodes))),
        (
            "relationships",
            diff_count(&set(&before.rels), &set(&after.rels)),
        ),
        (
            "labels",
            diff_count(&set(&before.labels), &set(&after.labels)),
        ),
        ("properties", diff_count(&before.props, &after.props)),
    ] {
        if p > 0 {
            out.insert(format!("+{name}"), p);
        }
        if m > 0 {
            out.insert(format!("-{name}"), m);
        }
    }
    out
}

// ----- running -----

fn tck_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testsuite/openCypher/tck")
}

fn cypher_params(table: &[Vec<String>]) -> Result<Params, String> {
    let mut p = Params::new();
    for row in table {
        let [k, v] = row.as_slice() else {
            return Err("bad parameter row".into());
        };
        let tv = parse_tv(v).ok_or_else(|| format!("cannot parse parameter {v}"))?;
        p.insert(k.clone(), from_tv(&tv));
    }
    Ok(p)
}

fn from_tv(v: &TV) -> Value {
    match v {
        TV::Null => Value::Null,
        TV::Bool(b) => Value::Bool(*b),
        TV::Int(i) => Value::Int(*i),
        TV::Float(f) => Value::Float(*f),
        TV::Str(s) => Value::String(s.clone()),
        TV::List(l) => Value::List(l.iter().map(from_tv).collect()),
        TV::Map(m) => Value::Map(m.iter().map(|(k, v)| (k.clone(), from_tv(v))).collect()),
        _ => Value::Null,
    }
}

enum Outcome {
    Pass,
    Fail(String),
}

fn run_scenario(sc: &Scenario) -> Outcome {
    let mut store = Store::new().unwrap();
    let mut params = Params::new();
    let mut result: Option<Result<CypherResult, CypherError>> = None;
    let mut before = Snapshot::default();
    let mut after = Snapshot::default();
    for st in &sc.steps {
        let t = st.text.as_str();
        let doc = match &st.arg {
            StepArg::Doc(d) => d.clone(),
            _ => String::new(),
        };
        let table = match &st.arg {
            StepArg::Table(t) => t.clone(),
            _ => Vec::new(),
        };
        if t == "an empty graph" || t == "any graph" {
            store = Store::new().unwrap();
        } else if let Some(g) = t
            .strip_prefix("the ")
            .and_then(|x| x.strip_suffix(" graph"))
        {
            let path = tck_dir().join("graphs").join(g).join(format!("{g}.cypher"));
            let Ok(src) = std::fs::read_to_string(&path) else {
                return Outcome::Fail(format!("missing graph {g}"));
            };
            if let Err(e) = store.cypher(&src) {
                return Outcome::Fail(format!("graph {g}: {e}"));
            }
        } else if t == "having executed" {
            if let Err(e) = store.cypher(&doc) {
                return Outcome::Fail(format!("setup failed: {e}\n{doc}"));
            }
        } else if t == "parameters are" {
            match cypher_params(&table) {
                Ok(p) => params = p,
                Err(e) => return Outcome::Fail(e),
            }
        } else if t.starts_with("there exists a procedure") {
            return Outcome::Fail("user-defined procedures are not supported".into());
        } else if t == "executing query" || t == "executing control query" {
            before = snapshot(&store);
            result = Some(store.cypher_with(&doc, &params, &Default::default()));
            after = snapshot(&store);
        } else if t == "the result should be empty" {
            match &result {
                Some(Ok(r)) if r.rows.is_empty() => {}
                Some(Ok(r)) => return Outcome::Fail(format!("expected no rows, got {:?}", r.rows)),
                Some(Err(e)) => return Outcome::Fail(format!("error: {e}")),
                None => return Outcome::Fail("no query".into()),
            }
        } else if let Some(mode) = t.strip_prefix("the result should be") {
            let r = match &result {
                Some(Ok(r)) => r,
                Some(Err(e)) => return Outcome::Fail(format!("error: {e}")),
                None => return Outcome::Fail("no query".into()),
            };
            let ordered = mode.contains("in order") && !mode.contains("any order");
            let unordered_lists = mode.contains("ignoring element order for lists");
            let mut rows = table.clone();
            if rows.is_empty() {
                return Outcome::Fail("empty expectation table".into());
            }
            let header = rows.remove(0);
            if header != r.columns {
                return Outcome::Fail(format!("columns {:?}, expected {header:?}", r.columns));
            }
            let mut expected = Vec::new();
            for row in &rows {
                let mut e = Vec::new();
                for c in row {
                    match parse_tv(c) {
                        Some(v) => e.push(v),
                        None => return Outcome::Fail(format!("cannot parse expected value {c}")),
                    }
                }
                expected.push(e);
            }
            let actual: Vec<Vec<TV>> = r
                .rows
                .iter()
                .map(|row| row.iter().map(to_tv).collect())
                .collect();
            if !rows_eq(&actual, &expected, ordered, unordered_lists) {
                return Outcome::Fail(format!(
                    "rows differ\n  actual:   {}\n  expected: {}",
                    r.rows
                        .iter()
                        .map(|row| format!(
                            "[{}]",
                            row.iter()
                                .map(|v| v.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .collect::<Vec<_>>()
                        .join(" "),
                    rows.iter()
                        .map(|row| format!("[{}]", row.join(", ")))
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
        } else if t.starts_with("a ") && t.contains("should be raised") {
            match &result {
                Some(Err(_)) => {}
                Some(Ok(r)) => return Outcome::Fail(format!("expected {t}, got {:?}", r.rows)),
                None => return Outcome::Fail("no query".into()),
            }
        } else if t == "no side effects" {
            if matches!(result, Some(Err(_))) {
                continue;
            }
            let fx = side_effects(&before, &after);
            if !fx.is_empty() {
                return Outcome::Fail(format!("unexpected side effects {fx:?}"));
            }
        } else if t == "the side effects should be" {
            let fx = side_effects(&before, &after);
            let mut expected = BTreeMap::new();
            for row in &table {
                if let [k, v] = row.as_slice() {
                    expected.insert(k.clone(), v.parse::<i64>().unwrap_or(0));
                }
            }
            expected.retain(|_, v| *v != 0);
            if fx != expected {
                return Outcome::Fail(format!("side effects {fx:?}, expected {expected:?}"));
            }
        } else {
            return Outcome::Fail(format!("unknown step: {t}"));
        }
    }
    Outcome::Pass
}

fn is_read_only(sc: &Scenario) -> bool {
    sc.steps.iter().all(|st| {
        if st.text != "executing query" {
            return true;
        }
        let StepArg::Doc(d) = &st.arg else {
            return true;
        };
        let u = d.to_uppercase();
        !["CREATE", "MERGE", "SET ", "DELETE", "REMOVE"]
            .iter()
            .any(|k| u.contains(k))
    })
}

fn load_allowlist() -> BTreeMap<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tck-allowlist.txt");
    let mut out = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for l in text.lines() {
            if l.starts_with('#') || l.trim().is_empty() {
                continue;
            }
            let (id, reason) = l.split_once(" :: ").unwrap_or((l, ""));
            out.insert(id.trim().to_string(), reason.trim().to_string());
        }
    }
    out
}

// @lat: [[tests#Cypher#openCypher TCK]]
/// Runs on its own thread: path-heavy scenarios (Match6/7/9, Pattern2) nest the compiled
/// algebra about a hundred joins deep, and unoptimized `Compiler::pattern` frames are large
/// enough to overflow the 2 MiB default test-thread stack.
#[test]
fn opencypher_tck() {
    let run = std::thread::Builder::new()
        .name("opencypher_tck".into())
        .stack_size(16 << 20)
        .spawn(run_tck)
        .unwrap();
    if let Err(e) = run.join() {
        std::panic::resume_unwind(e);
    }
}

fn run_tck() {
    let dir = tck_dir().join("features");
    if !dir.exists() {
        eprintln!(
            "openCypher TCK not checked out (git submodule update --init testsuite/openCypher)"
        );
        return;
    }
    let mut files = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "feature") {
                files.push(p);
            }
        }
    }
    files.sort();
    let filter = std::env::var("OXILITE_TCK_FILTER").ok();
    let report = std::env::var("OXILITE_TCK_REPORT").is_ok();
    let allow = load_allowlist();
    let (mut total, mut passed, mut ro_total, mut ro_passed) = (0, 0, 0, 0);
    let mut by_dir: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut unexpected_fail = Vec::new();
    let mut unexpected_pass = Vec::new();
    let mut failures = Vec::new();
    for f in &files {
        for sc in parse_feature(f) {
            let id = format!("{} :: {}", sc.feature, sc.name);
            if filter.as_ref().is_some_and(|x| !id.contains(x.as_str())) {
                continue;
            }
            let ro = is_read_only(&sc);
            let outcome = std::panic::catch_unwind(|| run_scenario(&sc))
                .unwrap_or_else(|_| Outcome::Fail("panic".into()));
            total += 1;
            let dir_key = sc
                .feature
                .rsplit_once('/')
                .map_or(sc.feature.clone(), |(d, _)| d.to_string());
            let e = by_dir.entry(dir_key).or_default();
            e.0 += 1;
            if ro {
                ro_total += 1;
            }
            // `[n]`, plus `#k` for the k-th example of a scenario outline.
            let num = sc.name.split(' ').next().unwrap_or("");
            let example = sc
                .name
                .rsplit_once(" #")
                .map(|(_, k)| format!("#{k}"))
                .unwrap_or_default();
            let key = format!("{} {num}{example}", sc.feature);
            match outcome {
                Outcome::Pass => {
                    passed += 1;
                    e.1 += 1;
                    if ro {
                        ro_passed += 1;
                    }
                    if allow.contains_key(&key) {
                        unexpected_pass.push(key);
                    }
                }
                Outcome::Fail(why) => {
                    let why1 = why.lines().next().unwrap_or("").to_string();
                    failures.push(format!("{key} :: {why1}"));
                    if report {
                        let q = sc
                            .steps
                            .iter()
                            .filter(|st| st.text.starts_with("executing"))
                            .filter_map(|st| match &st.arg {
                                StepArg::Doc(d) => Some(d.replace('\n', " ")),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(" ; ");
                        println!("FAIL {id}\n  query: {q}\n  {why}\n");
                    }
                    if !allow.contains_key(&key) {
                        unexpected_fail.push(format!("{key} :: {why1}"));
                    }
                }
            }
        }
    }
    println!(
        "openCypher TCK: {passed}/{total} scenarios pass ({:.1}%)",
        100.0 * passed as f64 / total.max(1) as f64
    );
    println!(
        "  read-only scenarios: {ro_passed}/{ro_total} ({:.1}%)",
        100.0 * ro_passed as f64 / ro_total.max(1) as f64
    );
    for (d, (t, p)) in &by_dir {
        println!("  {d:<40} {p:>4}/{t:<4}");
    }
    if let Ok(path) = std::env::var("OXILITE_TCK_WRITE_ALLOWLIST") {
        std::fs::write(path, failures.join("\n") + "\n").unwrap();
    }
    if filter.is_none() {
        assert!(
            unexpected_fail.is_empty(),
            "{} scenario(s) fail and are not allow-listed:\n{}",
            unexpected_fail.len(),
            unexpected_fail.join("\n")
        );
        assert!(
            unexpected_pass.is_empty(),
            "{} allow-listed scenario(s) now pass, remove them from tck-allowlist.txt:\n{}",
            unexpected_pass.len(),
            unexpected_pass.join("\n")
        );
    }
}
