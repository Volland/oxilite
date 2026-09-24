//! The server driven over an in-memory LSP connection against temporary workspaces.
use super::*;
use lsp_server::RequestId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;

struct Client {
    connection: Connection,
    server: Option<JoinHandle<()>>,
    next_id: i32,
    dir: PathBuf,
    /// Notifications received while waiting for responses.
    inbox: Vec<Notification>,
}

fn workspace(files: &[(&str, &str)]) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "oxilite-studio-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        write(&dir, name, content);
    }
    dir
}

fn write(dir: &std::path::Path, name: &str, content: &str) {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

impl Client {
    fn start(dir: PathBuf) -> Self {
        let (server_side, connection) = Connection::memory();
        let server = std::thread::spawn(move || {
            serve(&server_side, Some(":memory:".into())).unwrap();
        });
        let mut client = Self {
            connection,
            server: Some(server),
            next_id: 0,
            dir,
            inbox: Vec::new(),
        };
        let root = url::Url::from_file_path(&client.dir).unwrap().to_string();
        client.call(
            "initialize",
            json!({"capabilities": {}, "workspaceFolders": [{"uri": root, "name": "w"}]}),
        );
        client.send(Notification::new("initialized".into(), json!({})).into());
        client
    }

    fn send(&self, m: Message) {
        self.connection.sender.send(m).unwrap();
    }

    /// Sends a request and returns its response, keeping notifications in the inbox.
    fn call(&mut self, method: &str, params: Value) -> Response {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.send(Request::new(id.clone(), method.into(), params).into());
        loop {
            match self.connection.receiver.recv().unwrap() {
                Message::Response(r) => {
                    assert_eq!(r.id, id);
                    return r;
                }
                Message::Notification(n) => self.inbox.push(n),
                Message::Request(_) => {}
            }
        }
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params);
        assert!(r.error.is_none(), "{:?}", r.error);
        r.result.unwrap()
    }

    /// Takes the notifications received so far with this method.
    fn notifications(&mut self, method: &str) -> Vec<Value> {
        let (taken, kept) = std::mem::take(&mut self.inbox)
            .into_iter()
            .partition(|n| n.method == method);
        self.inbox = kept;
        taken.into_iter().map(|n: Notification| n.params).collect()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.call("shutdown", Value::Null);
        self.send(Notification::new("exit".into(), Value::Null).into());
        if let Some(s) = self.server.take() {
            let _ = s.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const PEOPLE: &str =
    "@prefix ex: <http://ex.org/> .\nex:alice ex:name \"Alice\" .\nex:bob ex:name \"Bob\" .\n";
const KNOWS: &str = "@prefix ex: <http://ex.org/> .\nex:alice ex:knows ex:bob .\n";

// @lat: [[tests#Studio server#Files load on initialize]]
#[test]
fn files_load_on_initialize() {
    let dir = workspace(&[
        ("people.ttl", PEOPLE),
        ("sub/knows.ttl", KNOWS),
        ("node_modules/skip.ttl", KNOWS),
        (".hidden/skip.ttl", KNOWS),
        ("notes.txt", "not rdf"),
    ]);
    let mut c = Client::start(dir);
    let status = c.ok("oxilite/status", Value::Null);
    assert_eq!(status["files"].as_array().unwrap().len(), 2);
    assert_eq!(status["triples"], 3);
}

// @lat: [[tests#Studio server#Query joins across files]]
#[test]
fn query_joins_across_files() {
    let dir = workspace(&[("people.ttl", PEOPLE), ("knows.ttl", KNOWS)]);
    let mut c = Client::start(dir);
    let r = c.ok(
        "oxilite/query",
        json!({"query": "PREFIX ex: <http://ex.org/> SELECT ?n WHERE { ?a ex:knows ?b . ?b ex:name ?n }"}),
    );
    assert_eq!(r["kind"], "solutions");
    assert_eq!(r["variables"], json!(["n"]));
    assert_eq!(r["rows"][0][0]["value"], "Bob");
    assert_eq!(r["truncated"], false);
}

// @lat: [[tests#Studio server#Row limit truncates]]
#[test]
fn row_limit_truncates() {
    let mut c = Client::start(workspace(&[("people.ttl", PEOPLE)]));
    let r = c.ok(
        "oxilite/query",
        json!({"query": "SELECT * WHERE { ?s ?p ?o }", "limit": 1}),
    );
    assert_eq!(r["rows"].as_array().unwrap().len(), 1);
    assert_eq!(r["truncated"], true);
}

// @lat: [[tests#Studio server#Syntax error is a diagnostic]]
#[test]
fn syntax_error_is_a_diagnostic() {
    let broken = "@prefix ex: <http://ex.org/> .\nex:a ex:p ex:b .\nex:c ex:p \"unterminated .\n";
    let mut c = Client::start(workspace(&[("people.ttl", PEOPLE), ("broken.ttl", broken)]));
    let status = c.ok("oxilite/status", Value::Null);
    assert_eq!(status["triples"], 2, "the broken file loads nothing");
    let diagnostics = c.notifications(PublishDiagnostics::METHOD);
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0]["uri"]
        .as_str()
        .unwrap()
        .ends_with("broken.ttl"));
    assert_eq!(
        diagnostics[0]["diagnostics"][0]["range"]["start"]["line"],
        2
    );
}

// @lat: [[tests#Studio server#Reload picks up changes]]
#[test]
fn reload_picks_up_changes() {
    let broken = "@prefix ex: <http://ex.org/> .\nex:a ex:p .\n";
    let mut c = Client::start(workspace(&[("broken.ttl", broken)]));
    assert_eq!(c.ok("oxilite/status", Value::Null)["triples"], 0);
    c.notifications(PublishDiagnostics::METHOD);
    write(&c.dir.clone(), "broken.ttl", KNOWS);
    write(&c.dir.clone(), "people.ttl", PEOPLE);
    let status = c.ok("oxilite/reload", Value::Null);
    assert_eq!(status["triples"], 3);
    let cleared = c.notifications(PublishDiagnostics::METHOD);
    assert_eq!(cleared.len(), 1);
    assert_eq!(cleared[0]["diagnostics"], json!([]));
}

// @lat: [[tests#Studio server#Query errors are request failures]]
#[test]
fn query_errors_are_request_failures() {
    let mut c = Client::start(workspace(&[]));
    let r = c.call("oxilite/query", json!({"query": "SELEC nothing"}));
    assert_eq!(r.error.unwrap().code, ErrorCode::RequestFailed as i32);
}

impl Client {
    fn open(&mut self, name: &str, language: &str, text: &str) -> String {
        let uri = url::Url::from_file_path(self.dir.join(name))
            .unwrap()
            .to_string();
        self.send(
            Notification::new(
                "textDocument/didOpen".into(),
                json!({"textDocument": {"uri": uri, "languageId": language, "version": 1, "text": text}}),
            )
            .into(),
        );
        uri
    }
}

fn at(uri: &str, line: u32, character: u32) -> Value {
    json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}})
}

// @lat: [[tests#Studio server#Hover and definition across files]]
#[test]
fn hover_and_definition_across_files() {
    let people = "@prefix ex: <http://ex.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\nex:bob a ex:Person ; rdfs:label \"Bob\" .\n";
    let mut c = Client::start(workspace(&[("people.ttl", people), ("knows.ttl", KNOWS)]));
    let q = c.open(
        "q.rq",
        "sparql",
        "PREFIX ex: <http://ex.org/>\nSELECT * { ?x ex:knows ex:bob }",
    );
    let hover = c.ok("textDocument/hover", at(&q, 1, 27));
    let md = hover["contents"]["value"].as_str().unwrap();
    assert!(md.contains("**Bob**"), "{md}");
    assert!(md.contains("people.ttl:3"), "{md}");
    let defs = c.ok("textDocument/definition", at(&q, 1, 27));
    assert!(defs[0]["uri"].as_str().unwrap().ends_with("people.ttl"));
    assert_eq!(defs[0]["range"]["start"]["line"], 2);
    let mut params = at(&q, 1, 27);
    params["context"] = json!({"includeDeclaration": true});
    let refs = c.ok("textDocument/references", params);
    assert_eq!(refs.as_array().unwrap().len(), 2);
}

// @lat: [[tests#Studio server#Completion over LSP uses the store]]
#[test]
fn completion_over_lsp_uses_the_store() {
    let mut c = Client::start(workspace(&[("people.ttl", PEOPLE), ("knows.ttl", KNOWS)]));
    let q = c.open(
        "q.rq",
        "sparql",
        "PREFIX ex: <http://ex.org/>\nSELECT * { ?x ex:",
    );
    let items = c.ok("textDocument/completion", at(&q, 1, 17));
    let labels: Vec<&str> = items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["ex:name", "ex:knows"]);
    let diagnostics = c.notifications(PublishDiagnostics::METHOD);
    assert!(diagnostics
        .iter()
        .any(|d| d["uri"] == q.as_str() && !d["diagnostics"].as_array().unwrap().is_empty()));
}

// @lat: [[tests#Studio server#Attached stores and confirmed updates]]
#[test]
fn attached_store_needs_confirmation() {
    let mut c = Client::start(workspace(&[("people.ttl", PEOPLE)]));
    let db = c.dir.join("real.sqlite").display().to_string();
    let list = c.ok("oxilite/attach", json!({"path": db}));
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[1]["active"], true);
    let insert = json!({"query": "INSERT DATA { <http://ex.org/a> <http://ex.org/p> 1 }"});
    let r = c.call("oxilite/query", insert.clone());
    assert_eq!(r.error.unwrap().code, NEEDS_CONFIRMATION);
    let mut confirmed = insert;
    confirmed["confirmed"] = json!(true);
    let r = c.ok("oxilite/query", confirmed);
    assert_eq!(r["kind"], "update");
    assert_eq!(r["delta"], 1);
    let rows = c.ok("oxilite/query", json!({"query": "SELECT * { ?s ?p ?o }"}));
    assert_eq!(rows["rows"].as_array().unwrap().len(), 1);
    // The project store is separate and updates to it are marked as ephemeral.
    let r = c.ok("oxilite/query", json!({"query": "INSERT DATA { <http://ex.org/x> <http://ex.org/p> 2 }", "connection": "project"}));
    assert_eq!(r["ephemeral"], true);
    c.ok("oxilite/detach", json!({"id": format!("attached:{db}")}));
    let list = c.ok("oxilite/connections", Value::Null);
    assert_eq!(list[0]["active"], true);
}

// @lat: [[tests#Studio server#Explain returns the SQL]]
#[test]
fn explain_returns_the_sql() {
    let mut c = Client::start(workspace(&[("people.ttl", PEOPLE)]));
    let plan = c.ok(
        "oxilite/explain",
        json!({"query": "PREFIX ex: <http://ex.org/> SELECT ?n WHERE { ?x ex:name ?n }"}),
    );
    assert!(plan["text"].as_str().unwrap().contains("SELECT"), "{plan}");
}

impl Client {
    /// Waits for the next notification with this method (skipping others), up to ten seconds.
    fn wait_for(&mut self, method: &str) -> Value {
        if let Some(i) = self.inbox.iter().position(|n| n.method == method) {
            return self.inbox.remove(i).params;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self
                .connection
                .receiver
                .recv_timeout(left)
                .expect("notification in time")
            {
                Message::Notification(n) if n.method == method => return n.params,
                Message::Notification(n) => self.inbox.push(n),
                _ => {}
            }
        }
    }

    /// Waits for diagnostics published for `uri`.
    fn diagnostics_for(&mut self, uri: &str) -> Value {
        loop {
            let d = self.wait_for(PublishDiagnostics::METHOD);
            if d["uri"] == uri {
                return d;
            }
        }
    }

    fn changed(&mut self, names: &[&str]) {
        let changes: Vec<Value> = names
            .iter()
            .map(|n| json!({"uri": url::Url::from_file_path(self.dir.join(n)).unwrap().to_string(), "type": 2}))
            .collect();
        self.send(
            Notification::new(
                "workspace/didChangeWatchedFiles".into(),
                json!({"changes": changes}),
            )
            .into(),
        );
    }
}

const ONTOLOGY: &str = "@prefix ex: <http://ex.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://ex.org/onto> a owl:Ontology .\nex:Employee rdfs:subClassOf ex:Person .\n";
const STAFF: &str = "@prefix ex: <http://ex.org/> .\nex:carol a ex:Employee ; ex:name \"Carol\" .\nex:dan a ex:Employee .\n";
const SHAPES: &str = "@prefix ex: <http://ex.org/> .\n@prefix sh: <http://www.w3.org/ns/shacl#> .\nex:PersonShape a sh:NodeShape ;\n  sh:targetClass ex:Person ;\n  sh:property [ sh:path ex:name ; sh:minCount 1 ] .\n";

// @lat: [[tests#Studio server#SHACL results land on the data line]]
#[test]
fn shacl_results_land_on_the_data_line() {
    let mut c = Client::start(workspace(&[
        ("onto.ttl", ONTOLOGY),
        ("staff.ttl", STAFF),
        ("shapes.ttl", SHAPES),
    ]));
    // Without reasoning, nobody is a Person: the data conforms.
    let report = c.wait_for("oxilite/validationChanged");
    assert_eq!(report["conforms"], true, "{report}");
    // With RDFS, employees are persons, and dan has no name.
    c.inbox.clear();
    c.ok("oxilite/setReasoning", json!({"profile": "rdfs"}));
    let report = c.wait_for("oxilite/validationChanged");
    assert_eq!(report["conforms"], false, "{report}");
    let results = report["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["focus"].as_str().unwrap(), "http://ex.org/dan");
    let diagnostics: Vec<Value> = c
        .notifications(PublishDiagnostics::METHOD)
        .into_iter()
        .filter(|d| {
            d["uri"].as_str().unwrap().ends_with("staff.ttl")
                && !d["diagnostics"].as_array().unwrap().is_empty()
        })
        .collect();
    let d = &diagnostics.last().unwrap()["diagnostics"][0];
    assert_eq!(d["source"], "shacl");
    assert_eq!(d["range"]["start"]["line"], 2, "dan's line");
    assert!(d["relatedInformation"][0]["location"]["uri"]
        .as_str()
        .unwrap()
        .ends_with("shapes.ttl"));
    // Shapes are not data.
    let rows = c.ok(
        "oxilite/query",
        json!({"query": "ASK { ?s a <http://www.w3.org/ns/shacl#NodeShape> }"}),
    );
    assert_eq!(rows["value"], false);
}

// @lat: [[tests#Studio server#Manifest graphs and per-graph reload]]
#[test]
fn manifest_graphs_and_per_graph_reload() {
    let manifest = "reasoning = \"owl2rl\"\n[[graph]]\niri = \"https://ex.org/g/staff\"\nfiles = [\"data/*.ttl\"]\n[[graph]]\niri = \"https://ex.org/g/onto\"\nfiles = [\"onto.ttl\"]\nrole = \"ontology\"\n[rules]\nfiles = [\"rules/*.dl\"]\n";
    let rules = "@prefix ex: <http://ex.org/> .\nex:colleague(?x, ?y) :- ex:Employee(?x), ex:Employee(?y), ?x != ?y.\n";
    let mut c = Client::start(workspace(&[
        ("oxilite.toml", manifest),
        ("onto.ttl", ONTOLOGY),
        ("data/staff.ttl", STAFF),
        (
            "data/more.ttl",
            "@prefix ex: <http://ex.org/> .\nex:erin a ex:Employee .\n",
        ),
        ("rules/colleagues.dl", rules),
        ("ignored.ttl", KNOWS),
    ]));
    let status = c.ok("oxilite/status", Value::Null);
    assert_eq!(status["profile"], "owl2rl");
    assert_eq!(status["files"].as_array().unwrap().len(), 4, "{status}");
    let count = |c: &mut Client, q: &str| {
        c.ok("oxilite/query", json!({"query": q}))["rows"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(
        count(
            &mut c,
            "SELECT ?x WHERE { GRAPH <https://ex.org/g/staff> { ?x a ?t } }"
        ),
        3
    );
    // OWL 2 RL makes employees persons; the rule makes them colleagues.
    assert_eq!(
        count(&mut c, "SELECT ?x WHERE { ?x a <http://ex.org/Person> }"),
        3
    );
    assert_eq!(
        count(&mut c, "SELECT * WHERE { ?x <http://ex.org/colleague> ?y }"),
        6
    );
    // Changing one file of the staff graph reloads that graph only, and reasoning follows.
    write(
        &c.dir.clone(),
        "data/more.ttl",
        "@prefix ex: <http://ex.org/> .\n",
    );
    c.changed(&["data/more.ttl"]);
    c.wait_for("oxilite/storeChanged");
    assert_eq!(
        count(&mut c, "SELECT ?x WHERE { ?x a <http://ex.org/Person> }"),
        2
    );
    assert_eq!(
        count(&mut c, "SELECT * WHERE { ?x <http://ex.org/colleague> ?y }"),
        2
    );
    // The resource view knows what is inferred, and by which producer.
    let d = c.ok("oxilite/describe", json!({"iri": "http://ex.org/carol"}));
    let colleague = d["outgoing"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["p"]["value"] == "http://ex.org/colleague")
        .unwrap();
    assert_eq!(colleague["inferred"], true);
    assert_eq!(colleague["producers"], json!(["rules/colleagues.dl"]));
    let person = d["outgoing"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["o"]["value"] == "http://ex.org/Person")
        .unwrap();
    assert_eq!(person["producers"], json!(["owl2rl"]));
    let name = d["outgoing"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["p"]["value"] == "http://ex.org/name")
        .unwrap();
    assert_eq!(name["inferred"], false);
}

// @lat: [[tests#Studio server#Explorer shows the class hierarchy]]
#[test]
fn explorer_shows_the_class_hierarchy() {
    let mut c = Client::start(workspace(&[("onto.ttl", ONTOLOGY), ("staff.ttl", STAFF)]));
    c.ok("oxilite/setReasoning", json!({"profile": "rdfs"}));
    let root = c.ok("oxilite/explorer", json!({"node": "root"}));
    let ids: Vec<&str> = root
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["graphs", "classes", "properties", "files", "prefixes"]
    );
    let classes = c.ok("oxilite/explorer", json!({"node": "classes"}));
    let person = classes
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["iri"] == "http://ex.org/Person")
        .unwrap();
    assert_eq!(person["collapsible"], true);
    assert_eq!(person["description"], "0 (+2 inferred)");
    let subs = c.ok(
        "oxilite/explorer",
        json!({"node": "class:http://ex.org/Person"}),
    );
    assert_eq!(subs[0]["iri"], "http://ex.org/Employee");
    assert_eq!(subs[0]["description"], "2");
    let files = c.ok("oxilite/explorer", json!({"node": "files"}));
    assert!(files[0]["description"]
        .as_str()
        .unwrap()
        .starts_with("ontology"));
}

// @lat: [[tests#Studio server#Datalog runs and reports errors]]
#[test]
fn datalog_runs_and_reports_errors() {
    let family =
        "@prefix ex: <http://ex.org/> .\nex:ada ex:parent ex:bob .\nex:bob ex:parent ex:cy .\n";
    let mut c = Client::start(workspace(&[("family.ttl", family)]));
    let program = "@prefix ex: <http://ex.org/> .\nex:ancestor(?x, ?y) :- ex:parent(?x, ?y).\nex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z).\n?- ex:ancestor(ex:ada, ?who).\n";
    let r = c.ok("oxilite/datalog", json!({"query": program}));
    assert_eq!(r["kind"], "solutions");
    assert_eq!(r["rows"].as_array().unwrap().len(), 2);
    let plan = c.ok(
        "oxilite/explain",
        json!({"query": program, "language": "datalog"}),
    );
    assert_eq!(plan["kind"], "datalog");
    // An unsafe rule is reported on its head.
    let bad = "@prefix ex: <http://ex.org/> .\n\nex:orphan(?x, ?y) :- ex:parent(?x, ?z).\n";
    let uri = c.open("bad.dl", "datalog", bad);
    let d = c.diagnostics_for(&uri);
    assert!(
        d["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("unsafe"),
        "{d}"
    );
    assert_eq!(d["diagnostics"][0]["range"]["start"]["line"], 2);
    // Completion offers the store's predicates in rule bodies.
    let uri = c.open(
        "new.dl",
        "datalog",
        "@prefix ex: <http://ex.org/> .\nex:x(?a, ?b) :- ex:",
    );
    let items = c.ok("textDocument/completion", at(&uri, 1, 19));
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["label"] == "ex:parent"),
        "{items}"
    );
}

// @lat: [[tests#Studio server#Cypher runs over the data's own names]]
#[test]
fn cypher_runs_over_the_datas_own_names() {
    let people = "@prefix ex: <http://ex.org/> .\nex:alice a ex:Person ; ex:name \"Alice\" ; ex:knows ex:bob .\nex:bob a ex:Person ; ex:name \"Bob\" .\n";
    let mut c = Client::start(workspace(&[("people.ttl", people)]));
    // The first request is Cypher: the vocabulary (and its base namespace) is computed for it.
    let r = c.ok(
        "oxilite/cypher",
        json!({"query": "MATCH (a:Person)-[:knows]->(b:Person) RETURN a.name AS a, b.name AS b"}),
    );
    assert_eq!(r["kind"], "cypher");
    assert_eq!(r["rows"], json!([["Alice", "Bob"]]));
    let paths = c.ok(
        "oxilite/cypher",
        json!({"query": "MATCH p = (:Person)-[:knows]->(:Person) RETURN p"}),
    );
    assert_eq!(paths["rows"][0][0]["type"], "path");
    let uri = c.open("q.cypher", "cypher", "MATCH (p:Pe");
    let items = c.ok("textDocument/completion", at(&uri, 0, 11));
    assert_eq!(items[0]["label"], "Person");
    let uri = c.open("r.cypher", "cypher", "MATCH (a)-[:kn");
    let items = c.ok("textDocument/completion", at(&uri, 0, 14));
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["label"] == "knows"),
        "{items}"
    );
    let uri = c.open("bad.cypher", "cypher", "MATCH (n RETURN n");
    let d = c.diagnostics_for(&uri);
    assert!(!d["diagnostics"].as_array().unwrap().is_empty());
}

// @lat: [[tests#Studio server#Import and export round-trip]]
#[test]
fn import_and_export_round_trip() {
    let mut c = Client::start(workspace(&[]));
    let dir = c.dir.clone();
    write(
        &dir,
        "extra/people.nt",
        "<http://ex.org/a> <http://ex.org/p> \"x\" .\n",
    );
    let db = dir.join("real.sqlite").display().to_string();
    c.ok("oxilite/attach", json!({"path": db}));
    let path = dir.join("extra/people.nt").display().to_string();
    let r = c.call("oxilite/import", json!({"path": path}));
    assert_eq!(r.error.unwrap().code, NEEDS_CONFIRMATION);
    let r = c.ok(
        "oxilite/import",
        json!({"path": path, "graph": "http://ex.org/g", "confirmed": true}),
    );
    assert_eq!(r["added"], 1);
    let out = dir.join("out.nq").display().to_string();
    c.ok("oxilite/export", json!({"path": out}));
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("<http://ex.org/g>"), "{text}");
    let r = c.call(
        "oxilite/export",
        json!({"path": dir.join("out.ttl").display().to_string()}),
    );
    assert!(r.error.is_some(), "a dataset does not fit in Turtle");
}

fn iri(v: &str) -> Value {
    json!({"termType": "NamedNode", "value": v})
}

// @lat: [[tests#Studio server#Why explains inferences down to asserted lines]]
#[test]
fn why_explains_inferences_down_to_asserted_lines() {
    let manifest = "reasoning = \"owl2rl\"\n[[graph]]\niri = \"https://ex.org/g/all\"\nfiles = [\"*.ttl\"]\n[rules]\nfiles = [\"rules/*.dl\"]\n";
    let onto = "@prefix ex: <http://ex.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\nex:Manager rdfs:subClassOf ex:Employee .\nex:Employee rdfs:subClassOf ex:Person .\n";
    let data = "@prefix ex: <http://ex.org/> .\nex:ann a ex:Manager .\nex:ann ex:manages ex:bo .\n";
    let rules =
        "@prefix ex: <http://ex.org/> .\nex:boss(?y, ?x) :- ex:manages(?x, ?y), ex:Person(?x).\n";
    let mut c = Client::start(workspace(&[
        ("oxilite.toml", manifest),
        ("onto.ttl", onto),
        ("data.ttl", data),
        ("rules/boss.dl", rules),
    ]));
    // ann is a Person through two subclass steps.
    let tree = c.ok("oxilite/why", json!({"s": iri("http://ex.org/ann"), "p": iri(scanner::RDF_TYPE), "o": iri("http://ex.org/Person")}));
    assert_eq!(tree["status"], "inferred", "{tree}");
    assert_eq!(tree["producer"], "owl2rl");
    assert!(
        tree["rule"].as_str().unwrap().starts_with("cax-sco"),
        "{tree}"
    );
    let leaves: Vec<&Value> = tree["premises"].as_array().unwrap().iter().collect();
    assert!(
        leaves.iter().any(|p| p["status"] == "inferred"),
        "one premise is itself inferred: {tree}"
    );
    // The rule conclusion names its rule and bottoms out in asserted lines.
    let tree = c.ok("oxilite/why", json!({"s": iri("http://ex.org/bo"), "p": iri("http://ex.org/boss"), "o": iri("http://ex.org/ann")}));
    assert_eq!(tree["producer"], "rules/boss.dl", "{tree}");
    assert!(tree["rule"].as_str().unwrap().contains(":-"));
    let premises = tree["premises"].as_array().unwrap();
    assert_eq!(premises.len(), 2);
    assert_eq!(premises[0]["status"], "asserted");
    assert!(premises[0]["location"]["uri"]
        .as_str()
        .unwrap()
        .ends_with("data.ttl"));
    assert_eq!(premises[0]["location"]["range"]["start"]["line"], 2);
    assert_eq!(
        premises[1]["status"], "inferred",
        "ann is a Person by the ontology"
    );
    // An asserted triple is a leaf; a triple that does not hold says so.
    let leaf = c.ok("oxilite/why", json!({"s": iri("http://ex.org/ann"), "p": iri(scanner::RDF_TYPE), "o": iri("http://ex.org/Manager")}));
    assert_eq!(leaf["status"], "asserted");
    let absent = c.ok("oxilite/why", json!({"s": iri("http://ex.org/bo"), "p": iri(scanner::RDF_TYPE), "o": iri("http://ex.org/Person")}));
    assert_eq!(absent["status"], "absent");
}

const TEST_MANIFEST: &str = r#"
[[graph]]
iri = "https://ex.org/g/people"
files = ["data/*.ttl"]
[shapes]
files = ["shapes/*.ttl"]
[rules]
files = ["rules/*.dl"]

[[test]]
name = "people have names"
query = "tests/names.rq"
expect = "tests/names.srj"

[[test]]
name = "stale expectation"
query = "tests/names.rq"
expect = "tests/wrong.srj"

[[test]]
name = "bad person rejected"
data = "tests/bad.ttl"
expect_violations = ["ex:PersonShape"]

[[test]]
name = "project conforms"

[[test]]
name = "ancestry is transitive"
entails = "tests/yes.ttl"
not_entails = "tests/no.ttl"
"#;

fn test_workspace() -> PathBuf {
    let shapes = "@prefix ex: <http://ex.org/> .\n@prefix sh: <http://www.w3.org/ns/shacl#> .\nex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; sh:property [ sh:path ex:name ; sh:minCount 1 ] .\n";
    workspace(&[
        ("oxilite.toml", TEST_MANIFEST),
        ("data/people.ttl", "@prefix ex: <http://ex.org/> .\nex:ada a ex:Person ; ex:name \"Ada\" ; ex:parent ex:bob .\nex:bob a ex:Person ; ex:name \"Bob\" ; ex:parent ex:cy .\n"),
        ("shapes/person.ttl", shapes),
        ("rules/family.dl", "@prefix ex: <http://ex.org/> .\nex:ancestor(?x, ?y) :- ex:parent(?x, ?y).\nex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z).\n"),
        ("tests/names.rq", "PREFIX ex: <http://ex.org/> SELECT ?n WHERE { ?p ex:name ?n }"),
        ("tests/wrong.srj", r#"{"head": {"vars": ["n"]}, "results": {"bindings": [{"n": {"type": "literal", "value": "Zed"}}]}}"#),
        ("tests/bad.ttl", "@prefix ex: <http://ex.org/> .\nex:nobody a ex:Person .\n"),
        ("tests/yes.ttl", "@prefix ex: <http://ex.org/> .\nex:ada ex:ancestor ex:cy .\n"),
        ("tests/no.ttl", "@prefix ex: <http://ex.org/> .\nex:cy ex:ancestor ex:ada .\n"),
    ])
}

// @lat: [[tests#Studio server#Manifest tests run and snapshot]]
#[test]
fn manifest_tests_run_and_snapshot() {
    let mut c = Client::start(test_workspace());
    let tests = c.ok("oxilite/tests", Value::Null);
    assert_eq!(tests.as_array().unwrap().len(), 5);
    assert!(tests[0]["line"].as_u64().unwrap() > 0);
    let run = |c: &mut Client, name: &str| c.ok("oxilite/runTest", json!({"name": name}));
    // No expected file yet: the test fails until a snapshot is written.
    assert_eq!(run(&mut c, "people have names")["passed"], false);
    c.ok(
        "oxilite/updateSnapshot",
        json!({"name": "people have names"}),
    );
    assert_eq!(run(&mut c, "people have names")["passed"], true);
    let stale = run(&mut c, "stale expectation");
    assert_eq!(stale["passed"], false);
    assert!(
        stale["message"].as_str().unwrap().contains("missing"),
        "{stale}"
    );
    let bad = run(&mut c, "bad person rejected");
    assert_eq!(bad["passed"], true, "{bad}");
    assert_eq!(run(&mut c, "project conforms")["passed"], true);
    let entail = run(&mut c, "ancestry is transitive");
    assert_eq!(entail["passed"], true, "{entail}");
}

// @lat: [[tests#Studio server#Check reports everything and fails]]
#[test]
fn check_reports_everything_and_fails() {
    let dir = test_workspace();
    let (passed, report) = check::check(dir.clone()).unwrap();
    assert!(!passed, "the stale expectation fails");
    let failing: Vec<&str> = report["tests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["passed"] == false)
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(failing, ["people have names", "stale expectation"]);
    assert_eq!(report["conforms"], true);
    let text = check::render(&report);
    assert!(text.contains("FAIL    stale expectation"), "{text}");
    write(
        &dir,
        "data/people.ttl",
        "@prefix ex: <http://ex.org/> .\nex:ada a ex:Person .\n",
    );
    let (_, report) = check::check(dir.clone()).unwrap();
    assert_eq!(report["conforms"], false);
    assert_eq!(report["violations"][0]["file"], "data/people.ttl");
    assert_eq!(report["violations"][0]["line"], 2);
    let _ = std::fs::remove_dir_all(dir);
}

/// A stand-in for D1's `/raw` endpoint: runs each statement on a SQLite database and answers
/// in D1's JSON shape, with `meta` counts.
fn fake_d1() -> (String, std::thread::JoinHandle<()>, Arc<tiny_http::Server>) {
    use oxilite::rusqlite::RusqliteBackend;
    use oxilite_core::{Request as SqlRequest, SqlValue, Statement, SyncBackend};
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    let db = RusqliteBackend::open(":memory:").unwrap();
    let s = Arc::clone(&server);
    let handle = std::thread::spawn(move || {
        for mut request in s.incoming_requests() {
            assert_eq!(
                request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .unwrap()
                    .value
                    .as_str(),
                "Bearer secret"
            );
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let v: Value = serde_json::from_str(&body).unwrap();
            let mut results = Vec::new();
            let mut ok = true;
            let mut message = String::new();
            for sql in v["sql"].as_str().unwrap().split(";\n") {
                match db.execute(&SqlRequest::atomic(vec![Statement::new(sql)])) {
                    Ok(r) => {
                        let rs = r.into_iter().next().unwrap_or_default();
                        let rows: Vec<Value> = rs
                            .rows
                            .iter()
                            .map(|row| {
                                json!(row
                                    .iter()
                                    .map(|c| match c {
                                        SqlValue::Null => Value::Null,
                                        SqlValue::Integer(i) => json!(i),
                                        SqlValue::Real(f) => json!(f),
                                        SqlValue::Text(t) => json!(t),
                                    })
                                    .collect::<Vec<_>>())
                            })
                            .collect();
                        results.push(json!({
                            "results": {"columns": [], "rows": rows},
                            "meta": {"changes": rs.changes, "rows_read": rows.len(), "rows_written": rs.changes},
                            "success": true,
                        }));
                    }
                    Err(e) => {
                        ok = false;
                        message = e.to_string();
                        break;
                    }
                }
            }
            let answer = if ok {
                json!({"success": true, "errors": [], "result": results})
            } else {
                json!({"success": false, "errors": [{"message": message}], "result": []})
            };
            let _ = request.respond(tiny_http::Response::from_string(answer.to_string()));
        }
    });
    (url, handle, server)
}

// @lat: [[tests#Studio server#D1 over HTTP with billing]]
#[test]
fn d1_over_http_with_billing() {
    let (url, handle, server) = fake_d1();
    let mut c = Client::start(workspace(&[]));
    let list = c.ok(
        "oxilite/attachD1",
        json!({"account": "acc", "database": "db0123456789", "token": "secret", "readOnly": false, "endpoint": url}),
    );
    let d1 = list
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["kind"] == "d1")
        .unwrap()
        .clone();
    assert_eq!(d1["active"], true);
    assert_eq!(d1["label"], "D1 db012345");
    // An update asks first, with the rows it will write.
    let insert = json!({"query": "INSERT DATA { <http://ex.org/a> <http://ex.org/p> 1 . <http://ex.org/b> <http://ex.org/p> 2 }"});
    let r = c.call("oxilite/query", insert.clone());
    let e = r.error.unwrap();
    assert_eq!(e.code, NEEDS_CONFIRMATION);
    assert_eq!(
        e.data.unwrap()["estimate"],
        "about 10 billed rows written (2 quads)"
    );
    let mut confirmed = insert;
    confirmed["confirmed"] = json!(true);
    let r = c.ok("oxilite/query", confirmed);
    assert_eq!(r["delta"], 2);
    assert!(r["d1"]["rowsWritten"].as_u64().unwrap() > 0, "{r}");
    // 64-bit ids survive the JSON round trip, so reads decode.
    let rows = c.ok(
        "oxilite/query",
        json!({"query": "SELECT ?s ?o WHERE { ?s <http://ex.org/p> ?o } ORDER BY ?o"}),
    );
    assert_eq!(rows["rows"][1][0]["value"], "http://ex.org/b");
    assert_eq!(rows["rows"][1][1]["value"], "2");
    assert!(rows["d1"]["requests"].as_u64().unwrap() >= 1);
    let list = c.ok("oxilite/connections", Value::Null);
    let d1 = list
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["kind"] == "d1")
        .unwrap();
    assert!(d1["billing"]["requests"].as_u64().unwrap() > 2);
    drop(c);
    server.unblock();
    let _ = handle.join();
}

// @lat: [[tests#Studio server#MCP tools answer agents]]
#[test]
fn mcp_tools_answer_agents() {
    let dir = test_workspace();
    let mut m = mcp::Mcp::open(Some(dir.clone()), None).unwrap();
    let init = m.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})).unwrap();
    assert_eq!(init["result"]["capabilities"]["tools"], json!({}));
    assert!(m
        .handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .is_none());
    let list = m
        .handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "sparql_query",
            "datalog_query",
            "schema",
            "validate",
            "why",
            "reload"
        ]
    );
    let call = |m: &mut mcp::Mcp, name: &str, args: Value| {
        let r = m.handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": name, "arguments": args}})).unwrap();
        (
            r["result"]["isError"] == true,
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    };
    let (err, text) = call(
        &mut m,
        "sparql_query",
        json!({"query": "SELECT ?n WHERE { ?p <http://ex.org/name> ?n } ORDER BY ?n"}),
    );
    assert!(!err);
    assert_eq!(text, "?n\n\"Ada\"\n\"Bob\"");
    let (_, schema) = call(&mut m, "schema", json!({}));
    assert!(schema.contains("<http://ex.org/name> 2"), "{schema}");
    assert!(schema.contains("ex: <http://ex.org/>"), "{schema}");
    let (_, v) = call(&mut m, "validate", json!({}));
    assert_eq!(v, "The data conforms to the shapes.");
    let (_, why) = call(
        &mut m,
        "why",
        json!({"subject": "http://ex.org/ada", "predicate": "http://ex.org/ancestor", "object": "http://ex.org/cy"}),
    );
    assert!(why.contains("rules/family.dl"), "{why}");
    let (err, _) = call(&mut m, "sparql_query", json!({"query": "SELEC"}));
    assert!(err);
    let _ = std::fs::remove_dir_all(dir);
}

// @lat: [[tests#Studio server#ShEx results are diagnostics]]
#[test]
fn shex_results_are_diagnostics() {
    let data = "@prefix ex: <http://ex.org/> .\nex:ada ex:name \"Ada\" .\nex:nobody ex:age 3 .\n";
    let schema = "PREFIX ex: <http://ex.org/>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\nex:PersonShape { ex:name xsd:string }\n";
    let map = "<http://ex.org/ada>@<http://ex.org/PersonShape>,<http://ex.org/nobody>@<http://ex.org/PersonShape>";
    let mut c = Client::start(workspace(&[
        ("data.ttl", data),
        ("person.shex", schema),
        ("person.sm", map),
    ]));
    let report = c.wait_for("oxilite/validationChanged");
    assert_eq!(report["conforms"], false, "{report}");
    let results = report["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{report}");
    assert_eq!(results[0]["focus"], "http://ex.org/nobody");
    assert_eq!(results[0]["component"], "shex");
    assert!(results[0]["location"]["uri"]
        .as_str()
        .unwrap()
        .ends_with("data.ttl"));
    assert_eq!(results[0]["location"]["range"]["start"]["line"], 2);
    assert!(results[0]["shapeLocation"]["uri"]
        .as_str()
        .unwrap()
        .ends_with("person.shex"));
}

// @lat: [[tests#Studio server#Full-text search with the manifest index]]
#[test]
fn full_text_search_with_the_manifest_index() {
    let manifest =
        "text_index = true\n[[graph]]\niri = \"https://ex.org/g\"\nfiles = [\"*.ttl\"]\n";
    let data = "@prefix ex: <http://ex.org/> .\nex:a ex:note \"graph databases on SQLite\" .\nex:b ex:note \"a note about cats\" .\n";
    let mut c = Client::start(workspace(&[
        ("oxilite.toml", manifest),
        ("notes.ttl", data),
    ]));
    let r = c.ok(
        "oxilite/query",
        json!({"query": "PREFIX oxl: <https://oxilite.dev/ns#> SELECT ?s WHERE { ?s ?p ?o FILTER(oxl:textMatch(?o, \"sqlite\")) }"}),
    );
    assert_eq!(r["rows"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["rows"][0][0]["value"], "http://ex.org/a");
    let plan = c.ok("oxilite/explain", json!({"query": "PREFIX oxl: <https://oxilite.dev/ns#> SELECT ?s WHERE { ?s ?p ?o FILTER(oxl:textMatch(?o, \"sqlite\")) }"}));
    assert!(
        plan["text"].as_str().unwrap().contains("terms_fts"),
        "the index is used: {plan}"
    );
}

// @lat: [[tests#Studio server#Ontology diagram data]]
#[test]
fn ontology_diagram_data() {
    let onto = "@prefix ex: <http://ex.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\nex:Person a owl:Class .\nex:Employee rdfs:subClassOf ex:Person .\nex:Company a owl:Class .\nex:worksFor rdfs:domain ex:Employee ; rdfs:range ex:Company .\nex:name rdfs:domain ex:Person ; rdfs:range xsd:string .\n";
    let mut c = Client::start(workspace(&[("onto.ttl", onto), ("staff.ttl", STAFF)]));
    let o = c.ok("oxilite/ontology", Value::Null);
    let classes: Vec<&str> = o["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["iri"].as_str().unwrap())
        .collect();
    assert!(
        classes.contains(&"http://ex.org/Company") && classes.contains(&"http://ex.org/Employee"),
        "{o}"
    );
    assert!(
        !classes
            .iter()
            .any(|c| c.starts_with("http://www.w3.org/2002/07/owl#")),
        "no OWL vocabulary: {o}"
    );
    assert_eq!(
        o["subclass"],
        json!([["http://ex.org/Employee", "http://ex.org/Person"]])
    );
    let works = o["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["iri"] == "http://ex.org/worksFor")
        .unwrap();
    assert_eq!(works["datatype"], false);
    assert_eq!(works["range"], "http://ex.org/Company");
    let name = o["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["iri"] == "http://ex.org/name")
        .unwrap();
    assert_eq!(name["datatype"], true);
}

// @lat: [[tests#Studio server#Datalog debugger counts per rule]]
#[test]
fn datalog_debugger_counts_per_rule() {
    let family =
        "@prefix ex: <http://ex.org/> .\nex:ada ex:parent ex:bob .\nex:bob ex:parent ex:cy .\n";
    let mut c = Client::start(workspace(&[("family.ttl", family)]));
    let program = "@prefix ex: <http://ex.org/> .\nex:ancestor(?x, ?y) :- ex:parent(?x, ?y).\nex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z).\nex:orphan(?x, ?y) :- ex:adopted(?x, ?y).\n";
    let d = c.ok("oxilite/datalogDebug", json!({"program": program}));
    let rules = d["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 3);
    assert_eq!(rules[0]["bodyMatches"], 2);
    assert_eq!(rules[1]["bodyMatches"], 1, "ada -> bob -> cy");
    assert_eq!(rules[0]["facts"], 3);
    assert_eq!(rules[2]["bodyMatches"], 0, "an empty rule stands out");
    assert_eq!(rules[2]["line"], 3);
    assert!(!d["plan"].as_str().unwrap().is_empty());
}

// @lat: [[tests#Studio server#Why picks the rule whose premises hold]]
#[test]
fn why_picks_the_rule_whose_premises_hold() {
    let data =
        "@prefix ex: <http://ex.org/> .\nex:ada ex:manages ex:bob .\nex:bob ex:manages ex:eve .\n";
    let rules = "@prefix ex: <http://ex.org/> .\nex:reportsTo(?x, ?m) :- ex:manages(?m, ?x).\nex:reportsTo(?x, ?top) :- ex:manages(?m, ?x), ex:reportsTo(?m, ?top).\n";
    let mut c = Client::start(workspace(&[("data.ttl", data), ("rules/org.dl", rules)]));
    // eve reports to ada only through bob: the direct rule must not be chosen.
    let tree = c.ok(
        "oxilite/why",
        json!({"s": iri("http://ex.org/eve"), "p": iri("http://ex.org/reportsTo"), "o": iri("http://ex.org/ada")}),
    );
    let premises = tree["premises"].as_array().unwrap();
    assert_eq!(premises.len(), 2, "{tree}");
    assert!(premises.iter().all(|p| p["status"] != "absent"), "{tree}");
    assert_eq!(premises[0]["status"], "asserted");
    assert_eq!(premises[1]["status"], "inferred");
    assert_eq!(premises[1]["premises"][0]["status"], "asserted");
}
