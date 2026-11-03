//! Running a rule set over the store: [`ReasonJob`] answers "what follows" without writing,
//! [`MaterializeJob`] stores the conclusions as inferences of a producer. Both are sans-IO
//! jobs: one read request for the input, eyeron in memory, and (for materialization) one atomic
//! write, so they run unchanged on every backend, Cloudflare D1 included.

use crate::error::{N3Error, Result};
use crate::rules::{RuleSet, RuleSetInfo};
use crate::term::{triple_from_n3, triple_to_n3};
use eyeron::{reason_document, ReasonerOptions, ReasonerResult};
use oxilite_core::ops::{reasoning_input_job, ReasoningInput, ScanJob};
use oxilite_core::sql::{Capabilities, Response};
use oxilite_core::{Job, Step};
use oxrdf::{GraphName, NamedNode, Quad, Triple};
use std::collections::HashSet;

/// The producer name materialized N3 conclusions are attributed to by default.
pub const N3_PRODUCER: &str = "n3";

/// eyeron's limits for one run; `None` keeps eyeron's default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    /// Fixpoint steps (roughly: facts derived). eyeron's default is one million.
    pub max_iterations: Option<usize>,
    /// Matcher steps in one premise search.
    pub max_match_steps: Option<usize>,
    /// Depth of backward (`<=`) rule calls.
    pub max_backward_depth: Option<usize>,
    /// Bytes of term text one derived fact may carry.
    pub max_term_bytes: Option<usize>,
}

/// What a run reads and how it reasons.
#[derive(Debug, Clone)]
pub struct Options {
    /// The graphs to read; `None` reads every graph but the system graphs, merged.
    pub graphs: Option<Vec<GraphName>>,
    /// Also read materialized inferences of other producers (never the run's own).
    pub include_inferred: bool,
    /// Load only the predicates the rule bodies read (off: load the whole scope).
    pub prune: bool,
    /// The producer conclusions are stored under (materialization).
    pub producer: String,
    /// The instant `time:localTime` reports, in milliseconds since the epoch (`None`: the clock).
    pub now: Option<i64>,
    /// Return eyeron's proof of the conclusions as N3 text.
    pub proof: bool,
    /// Fail rather than return or store a result a limit cut short. Materialization always
    /// refuses an incomplete run unless this is turned off.
    pub require_complete: bool,
    pub limits: Limits,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            graphs: None,
            include_inferred: false,
            prune: true,
            producer: N3_PRODUCER.to_owned(),
            now: None,
            proof: false,
            require_complete: false,
            limits: Limits::default(),
        }
    }
}

/// What a read-only run concluded.
#[derive(Debug, Clone)]
pub struct N3Result {
    /// The conclusions the store could hold (IRI or blank subject, IRI predicate), sorted,
    /// without the input facts.
    pub derived: Vec<Triple>,
    /// Conclusions with a list, formula or literal subject, as N3 text (never stored).
    pub unstorable: Vec<String>,
    /// Statements read from the store.
    pub loaded: usize,
    /// False when a limit stopped the run or a builtin failed (see `limits` and `errors`).
    pub complete: bool,
    pub limits: Vec<String>,
    pub errors: Vec<String>,
    /// The report of an inference fuse (`{ … } => false`) that fired, which stops the run.
    pub fuse: Option<String>,
    /// eyeron's proof, when `Options::proof` asked for it.
    pub proof: Option<String>,
    pub info: RuleSetInfo,
}

/// What a materialization stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeStats {
    /// Distinct inferred quads now attributed to the producer.
    pub inferred: u64,
    /// Statements read from the store.
    pub loaded: usize,
    /// Conclusions skipped because the store cannot hold them.
    pub unstorable: usize,
    pub producer: String,
}

// @tg: implements:: [[openspec:n3-rules#Scoped and pruned input]] {since: "2026-11-03"}
fn input_job(rules: &RuleSet, options: &Options, caps: &Capabilities) -> Result<ScanJob> {
    let predicates: Option<Vec<NamedNode>> = match (options.prune, rules.pruning()) {
        (true, Some(ps)) => Some(
            ps.iter()
                .filter_map(|p| NamedNode::new(p.as_str()).ok())
                .collect(),
        ),
        _ => None,
    };
    let input = ReasoningInput {
        subject: None,
        object: None,
        predicates: predicates
            .as_ref()
            .map(|ps| ps.iter().map(NamedNode::as_ref).collect()),
        graphs: options
            .graphs
            .as_ref()
            .map(|gs| gs.iter().map(GraphName::as_ref).collect()),
        include_inferred: options.include_inferred,
        exclude_producer: Some(options.producer.as_str()),
    };
    Ok(reasoning_input_job(&input, caps))
}

/// Runs eyeron over the loaded quads (graphs merged) and the rule set's own facts.
// @tg: implements:: [[openspec:n3-rules#Bounded and reproducible runs]] {since: "2026-11-03"}
pub fn reason_over(rules: &RuleSet, quads: &[Quad], options: &Options) -> N3Result {
    let mut doc = rules.document.clone();
    let facts: HashSet<Triple> = quads.iter().map(|q| Triple::from(q.clone())).collect();
    let loaded = facts.len();
    let mut facts: Vec<Triple> = facts.into_iter().collect();
    facts.sort_by_cached_key(ToString::to_string);
    doc.facts.extend(facts.iter().filter_map(triple_to_n3));
    let mut eo = ReasonerOptions {
        proof: options.proof,
        include_explicit: options.proof,
        now: options.now,
        ..ReasonerOptions::default()
    };
    let l = &options.limits;
    if let Some(x) = l.max_iterations {
        eo.max_iterations = x;
    }
    if let Some(x) = l.max_match_steps {
        eo.max_match_steps = x;
    }
    if let Some(x) = l.max_backward_depth {
        eo.max_backward_depth = x;
    }
    if let Some(x) = l.max_term_bytes {
        eo.max_term_bytes = x;
    }
    let result: ReasonerResult = reason_document(&doc, &eo);
    let mut derived = Vec::new();
    let mut unstorable = Vec::new();
    for t in &result.derived {
        match triple_from_n3(t) {
            Some(rdf) => derived.push(rdf),
            None => unstorable.push(eyeron::triples_to_n3(
                &doc.prefixes,
                std::slice::from_ref(t),
            )),
        }
    }
    derived.sort_by_cached_key(ToString::to_string);
    derived.dedup();
    N3Result {
        derived,
        unstorable,
        loaded,
        complete: result.is_complete(),
        limits: result
            .limits_reached
            .iter()
            .map(ToString::to_string)
            .collect(),
        errors: result.errors.iter().map(ToString::to_string).collect(),
        fuse: result
            .fuse
            .as_ref()
            .map(|f| eyeron::fuse_report(&doc.prefixes, f)),
        proof: options
            .proof
            .then(|| eyeron::proof_to_n3(&doc.prefixes, &result)),
        info: rules.info().clone(),
    }
}

// @tg: implements:: [[openspec:n3-rules#Inference fuses]] {since: "2026-11-03"}
fn check_complete(r: &N3Result, required: bool) -> Result<()> {
    if let Some(f) = &r.fuse {
        return Err(N3Error::Fuse(f.trim().to_owned()));
    }
    if required && !r.complete {
        let mut parts = r.limits.clone();
        parts.extend(r.errors.iter().cloned());
        return Err(N3Error::Incomplete(parts.join("; ")));
    }
    Ok(())
}

/// Reasons over the store without writing (see [`reason_over`]).
// @tg: implements:: [[openspec:n3-rules#Read-only reasoning]] {since: "2026-11-03"}
pub struct ReasonJob {
    rules: RuleSet,
    options: Options,
    scan: ScanJob,
}

impl ReasonJob {
    pub fn new(rules: RuleSet, options: Options, caps: &Capabilities) -> Result<Self> {
        let scan = input_job(&rules, &options, caps)?;
        Ok(Self {
            rules,
            options,
            scan,
        })
    }

    /// Parses the rules and prepares the run.
    pub fn parse(n3: &str, options: Options, caps: &Capabilities) -> Result<Self> {
        Self::new(RuleSet::parse(n3)?, options, caps)
    }
}

impl Job for ReasonJob {
    type Output = N3Result;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<N3Result>> {
        match self.scan.step(response)? {
            Step::Execute(r) => Ok(Step::Execute(r)),
            Step::Done(quads) => {
                let r = reason_over(&self.rules, &quads, &self.options);
                if self.options.require_complete && !r.complete {
                    let mut parts = r.limits.clone();
                    parts.extend(r.errors.iter().cloned());
                    return Err(oxilite_core::Error::Other(format!(
                        "N3 reasoning is incomplete: {}",
                        parts.join("; ")
                    )));
                }
                Ok(Step::Done(r))
            }
        }
    }
}

/// Stores a rule set's conclusions in `quads_inf` (default graph), attributed to the producer:
/// the producer's earlier inferences are replaced in the same atomic request, so a run reflects
/// the store as it is now. An inference fuse, or a limit when `require_complete` holds (the
/// default here), aborts before anything is written.
// @tg: implements:: [[openspec:n3-rules#Materialized conclusions]] {since: "2026-11-03"}
pub struct MaterializeJob {
    rules: RuleSet,
    options: Options,
    caps: Capabilities,
    scan: Option<ScanJob>,
    stats: Option<MaterializeStats>,
}

impl MaterializeJob {
    pub fn new(rules: RuleSet, options: Options, caps: &Capabilities) -> Result<Self> {
        let scan = input_job(&rules, &options, caps)?;
        Ok(Self {
            rules,
            options,
            caps: caps.clone(),
            scan: Some(scan),
            stats: None,
        })
    }

    /// Parses the rules and prepares the run; completeness is required.
    pub fn parse(n3: &str, mut options: Options, caps: &Capabilities) -> Result<Self> {
        options.require_complete = true;
        Self::new(RuleSet::parse(n3)?, options, caps)
    }
}

impl Job for MaterializeJob {
    type Output = MaterializeStats;

    fn step(&mut self, response: Option<Response>) -> oxilite_core::Result<Step<MaterializeStats>> {
        if let Some(scan) = &mut self.scan {
            return match scan.step(response)? {
                Step::Execute(r) => Ok(Step::Execute(r)),
                Step::Done(quads) => {
                    self.scan = None;
                    let r = reason_over(&self.rules, &quads, &self.options);
                    check_complete(&r, self.options.require_complete)
                        .map_err(|e| oxilite_core::Error::Other(e.to_string()))?;
                    let quads: Vec<Quad> = r
                        .derived
                        .iter()
                        .map(|t| t.clone().in_graph(GraphName::DefaultGraph))
                        .collect();
                    let (request, inferred) = oxilite_core::reason::inference_replace(
                        &self.options.producer,
                        &quads,
                        &self.caps,
                    );
                    self.stats = Some(MaterializeStats {
                        inferred,
                        loaded: r.loaded,
                        unstorable: r.unstorable.len(),
                        producer: self.options.producer.clone(),
                    });
                    Ok(Step::Execute(request))
                }
            };
        }
        let stats = self
            .stats
            .take()
            .ok_or_else(|| oxilite_core::Error::Other("materialization resumed twice".into()))?;
        Ok(Step::Done(stats))
    }
}
