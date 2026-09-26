//! The Project store: the workspace's RDF files loaded into a scratch SQLite database, each into
//! the graph the manifest (or the conventions) gives it, with ontologies registered, shapes and
//! rules kept aside, and inferences materialized by producer.
//!
// @lat: [[architecture#Studio server#Project store]]

use super::conn::Target;
use super::index::SourceIndex;
use super::manifest::{Layout, Profile, Role, MANIFEST};
use oxilite::io::{RdfFormat, RdfParser, RdfSerializer};
use oxilite::model::{GraphName, NamedNode, Quad};
use oxilite::schema::{Registration, SchemaRole};
use oxilite::sparql::{QueryOptions, Reasoning};
use oxilite::store::Store;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A problem with a file, with zero-based line and column positions when known.
#[derive(Debug, Clone)]
pub struct LoadError {
    pub message: String,
    pub start: Option<(u32, u32)>,
    pub end: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct LoadedFile {
    pub path: PathBuf,
    pub uri: String,
    pub role: Role,
    pub graph: String,
    pub triples: usize,
    pub error: Option<LoadError>,
}

/// What changed in a reload, so the caller knows what to recompute.
#[derive(Debug, Default, Clone, Copy)]
pub struct Changed {
    pub data: bool,
    pub shapes: bool,
    pub rules: bool,
}

pub struct Project {
    root: Option<PathBuf>,
    location: String,
    store: Store,
    layout: Layout,
    layout_error: Option<String>,
    /// The profile chosen in the editor, used when the manifest does not set one.
    profile_setting: Profile,
    files: Vec<LoadedFile>,
    index: Arc<SourceIndex>,
    /// Shapes files as N-Triples, by URI: validated against, never loaded as data.
    shapes: BTreeMap<String, String>,
    /// Rule programs by URI, materialized under their relative path as producer name.
    rules: BTreeMap<String, String>,
    rule_producers: BTreeSet<String>,
    /// ShEx schemas and shape maps by URI.
    shex: BTreeMap<String, String>,
    shape_maps: BTreeMap<String, String>,
    materialized_owl: bool,
    /// Errors raised while materializing rules, by URI.
    reasoning_errors: BTreeMap<String, String>,
    /// Bumped on every change; background results for an older generation are dropped.
    pub generation: u64,
}

impl Project {
    /// Creates the scratch store at `location` (deleting any previous one) and loads the root.
    pub fn open(root: Option<PathBuf>, location: &str) -> Result<Self> {
        if location != ":memory:" {
            let path = Path::new(location);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
                // The default location is inside the workspace: keep it out of version control.
                if dir.file_name().is_some_and(|n| n == ".oxilite") {
                    let ignore = dir.join(".gitignore");
                    if !ignore.exists() {
                        std::fs::write(ignore, "*\n")?;
                    }
                }
            }
            // The store is derived data: start from nothing so a stale schema never survives.
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let _ = std::fs::remove_file(format!("{location}{suffix}"));
            }
        }
        let mut project = Self {
            root,
            location: location.to_string(),
            store: Store::open(location)?,
            layout: Layout::convention(),
            layout_error: None,
            profile_setting: Profile::None,
            files: Vec::new(),
            index: Arc::default(),
            shapes: BTreeMap::new(),
            rules: BTreeMap::new(),
            rule_producers: BTreeSet::new(),
            shex: BTreeMap::new(),
            shape_maps: BTreeMap::new(),
            materialized_owl: false,
            reasoning_errors: BTreeMap::new(),
            generation: 0,
        };
        // The manifest decides whether the store gets a full-text index, so read it first.
        project.read_layout();
        if project.manifest().is_some_and(|m| m.text_index) {
            project.store = Store::open_with_options(
                location,
                oxilite::core::StoreOptions {
                    text_index: true,
                    ..Default::default()
                },
            )?;
        }
        project.load_all()?;
        Ok(project)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn files(&self) -> &[LoadedFile] {
        &self.files
    }

    pub fn index(&self) -> &SourceIndex {
        &self.index
    }

    pub fn index_arc(&self) -> Arc<SourceIndex> {
        Arc::clone(&self.index)
    }

    pub fn has_manifest(&self) -> bool {
        self.layout.manifest.is_some()
    }

    pub fn manifest(&self) -> Option<&super::manifest::Manifest> {
        self.layout.manifest.as_ref()
    }

    /// The manifest's profile, else the editor's.
    pub fn profile(&self) -> Profile {
        self.layout.profile().unwrap_or(self.profile_setting)
    }

    /// Sets the editor's profile; returns whether the effective profile changed.
    pub fn set_profile_setting(&mut self, p: Profile) -> Result<bool> {
        let before = self.profile();
        self.profile_setting = p;
        if self.profile() == before {
            return Ok(false);
        }
        self.generation += 1;
        self.reason()?;
        Ok(true)
    }

    pub fn shapes_ntriples(&self) -> String {
        self.shapes.values().cloned().collect::<Vec<_>>().join("\n")
    }

    pub fn shape_files(&self) -> Vec<String> {
        self.shapes.keys().cloned().collect()
    }

    pub fn has_shapes(&self) -> bool {
        !self.shapes.is_empty() || !self.shex_jobs().is_empty()
    }

    pub fn has_shacl(&self) -> bool {
        !self.shapes.is_empty()
    }

    /// Each ShEx schema with its shape map: from the manifest, or by convention the `.sm` file
    /// next to it with the same name.
    pub fn shex_jobs(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (uri, schema) in &self.shex {
            let map = match self.manifest().and_then(|m| {
                m.shex.iter().find(|x| {
                    self.root
                        .as_ref()
                        .and_then(|r| file_uri(&r.join(&x.schema)).ok())
                        .as_deref()
                        == Some(uri.as_str())
                })
            }) {
                Some(spec) => spec.map.clone().or_else(|| {
                    let f = self.root.as_ref()?.join(spec.shape_map.as_ref()?);
                    self.shape_maps.get(&file_uri(&f).ok()?).cloned()
                }),
                None => {
                    let stem = uri.rsplit_once('.').map_or(uri.as_str(), |(s, _)| s);
                    self.shape_maps
                        .iter()
                        .find(|(m, _)| m.rsplit_once('.').is_some_and(|(s, _)| s == stem))
                        .map(|(_, t)| t.clone())
                }
            };
            if let Some(map) = map {
                out.push((uri.clone(), schema.clone(), map));
            }
        }
        out
    }

    pub fn validate_inferred(&self) -> bool {
        self.manifest().is_none_or(|m| m.validation.inferred)
    }

    pub fn validation_enabled(&self) -> bool {
        self.manifest().is_none_or(|m| m.validation.enabled)
    }

    pub fn layout_error(&self) -> Option<&str> {
        self.layout_error.as_deref()
    }

    pub fn reasoning_errors(&self) -> &BTreeMap<String, String> {
        &self.reasoning_errors
    }

    pub fn rules(&self) -> &BTreeMap<String, String> {
        &self.rules
    }

    /// The query options every view of the Project store uses.
    pub fn options(&self) -> QueryOptions {
        QueryOptions {
            union_default_graph: true,
            reasoning: match self.profile() {
                Profile::Rdfs => Reasoning::Rdfs,
                Profile::Owlql => Reasoning::OwlQl,
                _ => Reasoning::None,
            },
            include_inferred: self.materialized_owl || !self.rule_producers.is_empty(),
            ..QueryOptions::default()
        }
    }

    pub fn target(&self) -> Target {
        Target {
            store: super::d1::Handle::Sqlite(self.store.clone()),
            options: self.options(),
            ephemeral: true,
            read_only: false,
        }
    }

    fn manifest_path(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|r| r.join(MANIFEST))
    }

    fn read_layout(&mut self) {
        self.layout_error = None;
        self.layout = match self.manifest_path().filter(|p| p.exists()) {
            Some(p) => match std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|t| Layout::from_toml(&t))
            {
                Ok(l) => l,
                Err(e) => {
                    self.layout_error = Some(e);
                    Layout::convention()
                }
            },
            None => Layout::convention(),
        };
    }

    /// Rebuilds everything from the files on disk.
    pub fn reload(&mut self) -> Result<()> {
        self.store.clear()?;
        self.load_all()
    }

    fn load_all(&mut self) -> Result<()> {
        self.generation += 1;
        self.read_layout();
        self.files.clear();
        self.shapes.clear();
        self.rules.clear();
        self.shex.clear();
        self.shape_maps.clear();
        self.rule_producers.clear();
        self.materialized_owl = false;
        self.index = Arc::default();
        let mut paths = Vec::new();
        if let Some(root) = &self.root {
            discover(root, &mut paths);
        }
        paths.sort();
        for path in paths {
            self.load_path(&path)?;
        }
        self.register_ontologies()?;
        self.reason()?;
        self.store.optimize()?;
        Ok(())
    }

    /// Reloads what changed on disk: every graph a changed file belonged to or now belongs to is
    /// cleared and refilled from its files; a manifest change reloads everything.
    pub fn reload_paths(&mut self, changed: &[PathBuf]) -> Result<Changed> {
        if changed
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == MANIFEST))
        {
            self.reload()?;
            return Ok(Changed {
                data: true,
                shapes: true,
                rules: true,
            });
        }
        self.generation += 1;
        let mut out = Changed::default();
        let mut graphs = BTreeSet::new();
        for path in changed {
            let uri = file_uri(path)?;
            if let Some(old) = self.files.iter().find(|f| f.uri == uri) {
                match old.role {
                    Role::Data | Role::Ontology => {
                        graphs.insert(old.graph.clone());
                    }
                    Role::Shapes | Role::Shex | Role::ShapeMap => out.shapes = true,
                    Role::Rules => out.rules = true,
                }
            }
            if let Some(a) = self.assignment(path) {
                match a.role {
                    Role::Data | Role::Ontology => {
                        graphs.insert(a.graph);
                    }
                    Role::Shapes | Role::Shex | Role::ShapeMap => out.shapes = true,
                    Role::Rules => out.rules = true,
                }
            }
        }
        // Everything that must be (re)loaded: the changed files, and every other file of a
        // graph that is about to be cleared.
        let mut reload: BTreeSet<PathBuf> =
            changed.iter().filter(|p| p.is_file()).cloned().collect();
        for f in &self.files {
            if graphs.contains(&f.graph) && matches!(f.role, Role::Data | Role::Ontology) {
                reload.insert(f.path.clone());
            }
        }
        let gone: BTreeSet<String> = changed
            .iter()
            .chain(reload.iter())
            .filter_map(|p| file_uri(p).ok())
            .collect();
        self.files.retain(|f| !gone.contains(&f.uri));
        let index = Arc::make_mut(&mut self.index);
        for uri in &gone {
            index.remove_file(uri);
            self.shapes.remove(uri);
            self.rules.remove(uri);
            self.shex.remove(uri);
            self.shape_maps.remove(uri);
        }
        for g in &graphs {
            self.store.clear_graph(&NamedNode::new(g.as_str())?)?;
        }
        for path in &reload {
            if path.is_file() {
                self.load_path(path)?;
            }
        }
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        out.data = !graphs.is_empty();
        if out.data {
            self.register_ontologies()?;
        }
        if out.data || out.rules {
            self.reason()?;
            self.store.optimize()?;
        }
        Ok(out)
    }

    fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        self.root
            .as_ref()
            .and_then(|r| path.strip_prefix(r).ok())
            .unwrap_or(path)
    }

    fn assignment(&self, path: &Path) -> Option<super::manifest::Assignment> {
        let uri = file_uri(path).ok()?;
        let content = std::fs::read(path).ok()?;
        self.layout.assign(
            self.relative(path),
            &uri,
            &String::from_utf8_lossy(&content),
        )
    }

    fn load_path(&mut self, path: &Path) -> Result<()> {
        let uri = file_uri(path)?;
        let Ok(data) = std::fs::read(path) else {
            return Ok(());
        };
        let text = String::from_utf8_lossy(&data).into_owned();
        let Some(a) = self.layout.assign(self.relative(path), &uri, &text) else {
            return Ok(());
        };
        let mut file = LoadedFile {
            path: path.to_path_buf(),
            uri: uri.clone(),
            role: a.role,
            graph: a.graph.clone(),
            triples: 0,
            error: None,
        };
        match a.role {
            Role::Shex => {
                match oxilite_validate::shex_schema(&text, &uri) {
                    Ok(_) => {}
                    Err(e) => {
                        file.error = Some(LoadError {
                            message: e.to_string(),
                            start: None,
                            end: None,
                        })
                    }
                }
                self.shex.insert(uri.clone(), text.clone());
                // ShExC names shapes with IRIs and prefixed names the scanner understands.
                Arc::make_mut(&mut self.index).set_file(&uri, &text);
            }
            Role::ShapeMap => {
                self.shape_maps.insert(uri.clone(), text);
            }
            Role::Rules => match oxilite::datalog::parse(&text) {
                Ok(_) => {
                    self.rules.insert(uri.clone(), text);
                }
                Err(e) => file.error = Some(datalog_error(&e)),
            },
            Role::Data | Role::Ontology | Role::Shapes => {
                let format = format_of(path).unwrap_or(RdfFormat::Turtle);
                match parse(&data, format, &uri, &a.graph) {
                    Ok(quads) => {
                        file.triples = quads.len();
                        if a.role == Role::Shapes {
                            self.shapes.insert(uri.clone(), to_ntriples(&quads)?);
                        } else {
                            let mut loader = self.store.bulk_loader();
                            loader.load_quads(quads)?;
                            loader.commit()?;
                        }
                    }
                    Err(e) => file.error = Some(e),
                }
                // Positions come from text formats; RDF/XML loads without them.
                if format != RdfFormat::RdfXml {
                    Arc::make_mut(&mut self.index).set_file(&uri, &text);
                }
            }
        }
        self.files.push(file);
        Ok(())
    }

    /// With a manifest, ontology graphs are registered so reasoning reads their axioms only,
    /// each for the graphs its `applies_to` names (every graph without it).
    fn register_ontologies(&self) -> Result<()> {
        let Some(m) = self.manifest() else {
            return Ok(());
        };
        for g in m.graphs.iter().filter(|g| g.role == Role::Ontology) {
            let applies_to = g
                .applies_to
                .iter()
                .map(|t| Ok(NamedNode::new(t.as_str())?.into()))
                .collect::<Result<Vec<GraphName>>>()?;
            self.store.register_schema_graph(
                &NamedNode::new(g.iri.as_str())?,
                SchemaRole::Ontology,
                &Registration::new().applies_to(applies_to),
            )?;
        }
        Ok(())
    }

    /// Materializes what the profile and the rules call for, each under its own producer:
    /// OWL 2 RL first, then every rule file, then OWL 2 RL again when rules may feed it.
    fn reason(&mut self) -> Result<()> {
        self.reasoning_errors.clear();
        let owl = self.profile() == Profile::Owl2rl;
        if owl {
            self.store.materialize()?;
        } else if self.materialized_owl {
            self.store
                .clear_inferences_of(oxilite::core::reason::OWL_PRODUCER)?;
        }
        self.materialized_owl = owl;
        let mut producers = BTreeSet::new();
        let rules: Vec<(String, String)> = self
            .rules
            .iter()
            .map(|(u, t)| (u.clone(), t.clone()))
            .collect();
        for (uri, text) in &rules {
            let producer = self.producer_name(uri);
            let options = oxilite::datalog::Options {
                union_default_graph: true,
                include_inferred: true,
                producer: producer.clone(),
                ..Default::default()
            };
            match self.store.datalog_materialize_with(text, &options) {
                Ok(_) => {
                    producers.insert(producer);
                }
                Err(e) => {
                    self.reasoning_errors.insert(uri.clone(), e.to_string());
                    self.store.clear_inferences_of(&producer)?;
                }
            }
        }
        for stale in self.rule_producers.difference(&producers) {
            self.store.clear_inferences_of(stale)?;
        }
        self.rule_producers = producers;
        if owl && !self.rule_producers.is_empty() {
            self.store.materialize()?;
        }
        Ok(())
    }

    /// A rule file's producer name: its path relative to the workspace root.
    pub fn producer_name(&self, uri: &str) -> String {
        url::Url::parse(uri)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .map(|p| self.relative(&p).display().to_string())
            .unwrap_or_else(|| uri.to_string())
    }

    pub fn status(&self) -> Result<Value> {
        Ok(json!({
            "id": "project",
            "store": self.location,
            "root": self.root.as_ref().map(|r| r.display().to_string()),
            "triples": self.store.len()?,
            "profile": self.profile().name(),
            "manifest": self.has_manifest(),
            "shapes": self.shapes.len(),
            "rules": self.rules.len(),
            "files": self.files.iter().map(|f| json!({
                "uri": f.uri,
                "role": f.role.name(),
                "graph": f.graph,
                "triples": f.triples,
                "error": f.error.as_ref().map(|e| e.message.clone())
                    .or_else(|| self.reasoning_errors.get(&f.uri).cloned()),
            })).collect::<Vec<_>>(),
        }))
    }
}

pub fn file_uri(path: &Path) -> Result<String> {
    Ok(url::Url::from_file_path(path)
        .map_err(|()| format!("not an absolute path: {}", path.display()))?
        .to_string())
}

/// Parses a whole file into quads, its default graph renamed to `graph`; a syntax error
/// loads nothing from the file.
fn parse(
    data: &[u8],
    format: RdfFormat,
    base: &str,
    graph: &str,
) -> std::result::Result<Vec<Quad>, LoadError> {
    let plain = |message: String| LoadError {
        message,
        start: None,
        end: None,
    };
    let parser = RdfParser::from_format(format)
        .with_base_iri(base)
        .map_err(|e| plain(e.to_string()))?
        .with_default_graph(GraphName::from(
            NamedNode::new(graph).map_err(|e| plain(e.to_string()))?,
        ))
        // Blank node labels are scoped to their file.
        .rename_blank_nodes();
    let mut quads = Vec::new();
    for q in parser.for_slice(data) {
        match q {
            Ok(q) => quads.push(q),
            Err(e) => {
                let range = e.location();
                return Err(LoadError {
                    message: e.to_string(),
                    start: range
                        .as_ref()
                        .map(|r| (r.start.line as u32, r.start.column as u32)),
                    end: range.map(|r| (r.end.line as u32, r.end.column as u32)),
                });
            }
        }
    }
    Ok(quads)
}

fn to_ntriples(quads: &[Quad]) -> Result<String> {
    let mut s = RdfSerializer::from_format(RdfFormat::NTriples).for_writer(Vec::new());
    for q in quads {
        s.serialize_triple(oxilite::model::TripleRef::new(
            q.subject.as_ref(),
            q.predicate.as_ref(),
            q.object.as_ref(),
        ))?;
    }
    Ok(String::from_utf8(s.finish()?)?)
}

fn datalog_error(e: &oxilite::datalog::DatalogError) -> LoadError {
    match e {
        // Spans are one-based.
        oxilite::datalog::DatalogError::Parse { span, message } => {
            let at = (span.line.saturating_sub(1), span.column.saturating_sub(1));
            LoadError {
                message: message.clone(),
                start: Some(at),
                end: None,
            }
        }
        other => LoadError {
            message: other.to_string(),
            start: None,
            end: None,
        },
    }
}

/// The RDF format of a file from its extension; `.owl` files are read as RDF/XML.
pub fn format_of(path: &Path) -> Option<RdfFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "ttl" => RdfFormat::Turtle,
        "nt" => RdfFormat::NTriples,
        "nq" => RdfFormat::NQuads,
        "trig" => RdfFormat::TriG,
        "n3" => RdfFormat::N3,
        "rdf" | "owl" => RdfFormat::RdfXml,
        _ => return None,
    })
}

fn discover(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => discover(&path, out),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
}
