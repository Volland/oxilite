## Why

N3 and Datalog are first-order. Some reasoning wants more: hypothetical questions ("would Ada be
an ancestor of Eve if Dan were Eve's parent?"), rules quantified over rules, higher-order helpers
(`std.map`, `std.fold`), cut and negation as failure, constraint handling. λProlog has all of it.

[lean-elpi](https://github.com/Volland/lean-elpi) implements the ELPI dialect of λProlog in Lean 4,
ships `libleanelpi` with a C interface, and publishes the Rust binding `leanelpi` (0.4.1) with host
predicates answered by a Rust closure during a run. tiramemsu added it as its third reasoning
engine (`tm-elpi`). oxilite can follow the same design: the store is read through host builtins,
and conclusions go to `quads_inf` like every other producer's.

## What Changes

- A new crate `oxilite-elpi`, behind an off-by-default `elpi` feature of `oxilite`. The engine is
  linked by `elpi-native` (crate feature `native`), which needs `libleanelpi` through
  `LEANELPI_DIST`; without it the crate builds and every run fails with `Unsupported`, so the
  workspace builds and tests without the library.
- **Host builtins** `ox.triple S P O` and `ox.quad S P O G`: each call is one pattern scan of the
  store, its ground arguments forming the pattern.
- **Typed RDF terms** as ELPI values: `(iri "…")`, `(str "…")`, `(num N)`, `(dbl F)`,
  `(lang "…" "tag")`, `(lit "…" "datatype")`, `(bnode "…")`, and `default-graph`.
- **Three uses**: `Store::elpi_query` (any goal, bindings back as RDF terms), `Store::elpi_check`
  (what `ox.violation Severity Name Message Focus` reports) and `Store::elpi_materialize` (what
  `ox.conclude (triple S P O)` concludes, stored as inferences of a producer, `elpi` by default).
- Bounded runs: fuel, a solution limit and a row budget for the host builtins.
- `oxilite elpi` on the command line (CLI feature `elpi`).

## Non-goals

- Cloudflare D1 and the async store: host calls are synchronous.
- tiramemsu's stored programs with includes, commit-time enforcement and valid-time sweeps.
- Node, Python and JVM bindings (`libleanelpi` exists for darwin and linux only).
- Goal calls from SPARQL or Cypher.
- A CI job with the native library (it needs a published `libleanelpi` build).

## Impact

- lat.md: [[architecture#ELPI reasoning]] (new), [[decisions#D45 ELPI programs run by lean-elpi over host builtins]] (new),
  [[tests#ELPI reasoning]] (new).
- Bound by [[decisions#D28 Inferences are attributed to producers]].
- Adds `[[openspec:elpi-programs#Optional ELPI engine]]` and the other requirements of `elpi-programs`.
- Uses `ops::reasoning_input_job` and `reason::inference_replace` from the N3 change.
