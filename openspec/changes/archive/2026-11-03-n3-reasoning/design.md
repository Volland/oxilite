## Context

eyeron parses N3 into a `Document` (facts and rules) and `reason_document` runs it in memory,
returning the derived triples, the completion status, the limits reached, an optional fired fuse
and, on request, proofs. `ReasonerOptions` carries the limits and `now`, the instant
`time:localTime` reports.

oxilite runs every operation through sans-IO jobs ([[architecture#Sans-IO core]]), and keeps
inferences in `quads_inf`, attributed per producer in `quads_inf_src`
([[decisions#D28 Inferences are attributed to producers]]). `oxilite-reason` already materializes
OWL 2 RL in memory this way.

## Decisions

### Reuse eyeron as published

`eyeron` 0.8.25 from crates.io with default features off: the `cli` feature (and its `ureq`
client) is left out, so nothing is fetched and the crate stays publishable. The engine is used
unchanged; oxilite only maps terms and chooses the input. Writing an N3 engine was rejected: eyeron
is EYE-compatible and maintained, and tiramemsu already depends on it.

- motivated_by: [[decisions#D42 Synalog is a guest language, run over the store as tables]] (a guest engine used as published)

### One read, reason in memory, one write

A run is a job of at most two requests: read the scope, reason, and (materialization) write
`inference_reset(producer)` + terms + `quads_inf` rows + `inference_attribute(producer)` atomically.
Lazy loading through callbacks was rejected: eyeron has no host-callback interface, and one read
keeps D1 at one round trip for the input.

### Prune by the predicates rule bodies read

The input is restricted to the predicates that appear in rule premises (and in formulas a
conclusion builds, which may be rules). A variable predicate turns pruning off; builtins
(`http://www.w3.org/2000/10/swap/…`) are never loaded. Facts in the rule text are part of the
input.

### Materialization is a full recompute per producer

Each run replaces its producer's conclusions. That makes non-monotonic rules
(`log:notIncludes`, `log:collectAllIn`, `log:forAllIn`) safe to materialize: the stored set is
always what the rules conclude from the store as it is now. tiramemsu's refusal of such rules (N11)
comes from its incremental support cascade, which oxilite does not have. The rule set's analysis
still reports its monotonicity.

### A producer never reads its own conclusions

With `include_inferred`, the input adds inferences claimed by some other producer only; a quad
only the running producer claims is left out. Otherwise a conclusion whose premises were deleted
would feed itself and never go.

### Completeness, fuses, clock

`require_complete` fails a run a limit cut short; materialization always requires it, so a partial
fixpoint is never stored. A fired fuse is reported by `reason` and aborts `materialize`.
`Options::now` pins `time:localTime` for reproducible runs.

### Storable conclusions only

Conclusions go to the default graph of `quads_inf`, as OWL 2 RL's do. A conclusion with a literal,
list or formula subject, or a non-IRI predicate, cannot be stored: `reason` returns it as N3 text
under `unstorable`, `materialize` counts and skips it. Blank nodes keep eyeron's deterministic
witness labels, so a rerun reproduces the same node.

## Risks / Trade-offs

- The whole pruned scope is held in memory; on large stores, scope by graph.
- eyeron's proof records builtin triples, not the facts a builtin read; proofs are informative,
  not support links.
