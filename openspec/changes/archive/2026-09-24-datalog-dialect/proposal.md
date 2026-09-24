## Why

oxilite can query a graph two ways and reason about it one way, and none of the three lets a user
write a rule.

SPARQL and openCypher both compile through spargebra algebra to one SQL statement, and both stop at
the same wall: **recursion**. SPARQL's only escape hatch is a property path (`ex:parent+`), which
the core lowers to `WITH RECURSIVE` in `compiler::ops`. A property path walks one predicate under a
fixed regex. It cannot carry an extra body atom, cannot filter part-way through the traversal,
cannot join two derived relations, and cannot be mutually recursive. Cypher's variable-length
patterns have the same ceiling.

Recursion that goes beyond that ceiling exists in the codebase already — but only as hand-written
Rust. `reason.rs` is a fixed RDFS/OWL-RL rule set with bespoke transitive-closure SQL and a
round-based `materialize_round()` loop. A user who wants "manager-of is transitive within a
department", "a part is critical if any sub-part is critical", or "propagate provenance along
derivation edges" cannot express it. They must patch the engine or pull the graph into application
code — which on D1 means pulling it over the network.

This is a missing concept, not a missing function: **a rule is not a first-class thing in oxilite.**

The theory says the gap is worth closing here rather than elsewhere. SPARQL is equivalent in
expressive power to non-recursive safe Datalog with negation, and Datalog with stratified negation
can express every SPARQL query. So a Datalog dialect is a strict superset of the algebra the core
already compiles, plus recursion — not a parallel engine.

## What Changes

- A new `oxilite-datalog` crate: an RDF-native Datalog dialect, parsed by a hand-written lexer and
  recursive-descent parser, in the shape `oxilite-cypher` established.
- **Non-recursive rules lower to `spargebra::algebra::GraphPattern`** and go through the existing
  `core::query::compile_query`. Joins, `NOT EXISTS`, aggregation, the statistics planner, constant
  encoding and D1 batching are reused, not rebuilt.
- **Constraints are `spargebra::algebra::Expression`**, so the whole SPARQL FILTER language —
  comparison, arithmetic, string and regex, datatype and temporal functions — is available in a
  rule body and is pushed into the join rather than applied after it.
- A `program` module: predicate dependency graph, Tarjan SCCs, stratification, and safety checks.
  Unstratified negation or aggregation is a compile error that names the offending cycle.
- Recursion compiled into the same statement wherever SQLite allows it. A linear component becomes
  **one `WITH RECURSIVE` member** — a single round trip, D1-safe. A mutually recursive component
  becomes one member with a discriminant column and one recursive term per rule. A non-linear
  component has no single-statement form, so it is iterated to a fixpoint in a `datalog_work`
  table, one request per round, driven by the existing `Job`/`Step` machine and therefore working
  unchanged on D1.
- A `datalog` subcommand on `oxilite-cli`, and the dialect in the WebAssembly core and both
  JavaScript packages, behind an off-by-default feature so a Worker that does not use rules does
  not carry them.
- Two entry points on `Store`, behind a `datalog` feature, named as `cypher_store.rs` names its
  own: `datalog`, `datalog_with`, `explain_datalog`, and `datalog_materialize`.
- `datalog_materialize` writes derived facts into the **existing `quads_inf` table** using
  `reason::materialize_reset`, so rules become an extension of the reasoner and their conclusions
  are visible to SPARQL and Cypher under the `inferred` option that already exists.
- `Capabilities` gains `compound_recursive_cte`, because multiple recursive terms in one CTE need
  SQLite 3.34.0 (2020-12-01) and D1's version is not ours to assume.

## Capabilities

### New Capabilities
- `datalog-query`: a Datalog dialect with stratified negation, constraints and aggregation,
  compiled to SQL, with recursion compiled to `WITH RECURSIVE` wherever SQLite permits.
- `datalog-rules`: rule programs as a materialization source, sharing the inference store with
  RDFS/OWL-RL reasoning.

### Modified Capabilities
- `sqlite-backends`: backends declare whether compound recursive CTEs are available.
- `query-explain`: `explain()` reports strata, per-SCC recursion strategy, and why a fallback was
  chosen.

## Non-goals

Deliberately out of scope, to keep this change reviewable:

- Existential rules / tuple-generating dependencies (value invention, `owl:sameAs` style
  skolemisation). Every rule here is range-restricted.
- Well-founded or stable-model semantics for unstratified negation. Unstratified programs are
  rejected, not interpreted.
- Semi-naive iteration. A non-linear component is evaluated by naive rounds — correct, and
  deduplicated by the work table's primary key — rather than by tracking a delta relation.
- `AVG` and `GROUP_CONCAT`, whose results have no inline term id (see the query spec).
- Aggregation *inside* a recursive SCC (`min`-style lattice recursion).
- A federated or streaming rule evaluator.
- Rule storage in the graph (rules are submitted as text, not stored as RDF).

## Impact

- New crate `oxilite-datalog`; new workspace member; a `datalog` feature on `oxilite`.
- `oxilite-core`: one additive field on `Capabilities`, defaulting conservatively, and one added
  table, `datalog_work`, created `IF NOT EXISTS` like every other and empty until a program needs
  to iterate.
- No change to SPARQL or Cypher behaviour: derived facts reuse `quads_inf`, which already exists
  and is already cleared and rebuilt by materialization.
- A store that never calls a Datalog entry point is bit-for-bit unaffected.
