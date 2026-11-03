## Why

oxilite reasons three ways today: RDFS/OWL-QL rewriting at query time, OWL 2 RL materialization,
and Datalog rules ([[architecture#Reasoning]], [[architecture#Datalog frontend]]). None of them
speaks Notation3, the rule language of the Semantic Web stack: `{ … } => { … }` rules over
triples, quoted formulas, builtins for math, strings, lists, time and scoped negation
(`log:notIncludes`), backward rules, queries, and inference fuses (`{ … } => false`).

tiramemsu, the bitemporal sibling store, added N3 reasoning with
[eyeron](https://crates.io/crates/eyeron), an EYE-compatible reasoner in Rust, published on
crates.io (0.8.25). oxilite can reuse the same engine and the same shape of API, adapted to an
atemporal store whose inferences already have a home: `quads_inf` with producer attribution
([[decisions#D28 Inferences are attributed to producers]]).

## What Changes

- A new crate `oxilite-n3`, behind an off-by-default `n3` feature of `oxilite`, depending on
  `eyeron = { version = "0.8.25", default-features = false }` (no HTTP client, nothing fetched).
- **Read-only reasoning**: `Store::n3_reason(rules)` answers "what follows", writing nothing.
- **Materialization**: `Store::n3_materialize(rules)` stores the conclusions in `quads_inf` under a
  producer (`n3` by default), replacing that producer's earlier conclusions in one atomic request;
  SPARQL, Cypher and Datalog read them with `include_inferred`.
- **Scoped, pruned input**: a run reads every graph but the system graphs (or chosen graphs) and,
  inside that scope, only the predicates the rule bodies use, in one read request.
- **Bounded and reproducible**: eyeron's limits, a `require_complete` switch (always on for
  materialization), a pinned clock for `time:localTime`, proofs on request; a fired fuse aborts
  materialization.
- Sans-IO `ReasonJob` and `MaterializeJob`, so `AsyncStore` (Cloudflare D1) runs them unchanged.
- `oxilite n3` on the command line.

## Non-goals

- Stored rule sets with history (tiramemsu's N8): rules are passed per call, as Datalog programs
  are; storing them in the schema registry is a follow-up.
- tiramemsu's valid-time sweeps and support cascade: oxilite has no valid time, and a full
  recompute per producer replaces the cascade.
- Node, Python and JVM bindings, and the interactive shell (follow-ups).

## Impact

- lat.md: [[architecture#N3 reasoning]] (new), [[architecture#Reasoning]],
  [[decisions#D44 N3 rules run by eyeron, materialized per producer]] (new), [[tests#N3 reasoning]] (new).
- Bound by [[decisions#D28 Inferences are attributed to producers]] and
  [[decisions#D1 SQL executor trait instead of the SQLite C API]] (sans-IO, one request per step).
- Adds `[[openspec:n3-rules#Optional N3 engine]]` and the other requirements of `n3-rules`.
- `oxilite-core` gains `ops::reasoning_input_job` and `reason::inference_replace`; no schema change.
