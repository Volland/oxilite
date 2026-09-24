## Context

`oxilite-cypher` proved the frontend recipe: a hand-written lexer and parser, a dialect AST, a
lowering pass to `spargebra` algebra, then `core::query::compile_query` and the existing planner,
with a Rust tail for what SQL cannot express ([[decisions#D16 Lowering to SPARQL algebra, with a
Rust tail]]). Datalog is a third frontend on that same recipe, with one part that genuinely has no
precedent: recursion that is not a property path.

Everything else already exists. Constraints are `spargebra::algebra::Expression`, which
`compiler/expr.rs` lowers in full. Negation is `GraphPattern::Minus`, which becomes `NOT EXISTS`.
Aggregation is `GraphPattern::Group`. Multi-round-trip iteration is the `Job`/`Step` machine that
`run_sync` and `run_async` drive identically. Persisted derivations are `quads_inf`, which
`reason.rs` already creates, clears and refills.

## Goals / Non-Goals

Goals:

- Rules as a first-class artefact: named, composable, reusable derived predicates.
- Recursion with body atoms and constraints, which property paths cannot express.
- One SQL statement, one round trip, for every program whose recursion is linear — the same
  invariant SPARQL and Cypher hold, because D1 charges per round trip.
- Stratified negation and aggregation, checked before any SQL is built, with errors that name the
  offending cycle.
- Derived facts that SPARQL and Cypher can read, through the `inferred` option that already exists.

Non-goals: existential rules, well-founded/stable semantics, recursive aggregation, rules stored as
RDF. Unstratified programs are rejected rather than interpreted.

## Decisions

### D23 Datalog as a third frontend, sharing the SPARQL compiler

A Datalog program's non-recursive parts are lowered to `spargebra` algebra and compiled by the
unchanged SPARQL compiler; only recursion gets new code.

This is [[decisions#D13 Cypher as a second frontend over the RDF store]] applied again, and it is
what makes the change affordable: joins, filters, negation, aggregation, `ORDER BY`/`LIMIT`,
statistics-driven join ordering, constant encoding, reasoning rewrites, D1 batching and the
spareval fallback all come free. The alternative — a native Datalog engine with its own relational
operators over its own tables, as Soufflé, Nemo and Cozo do — would duplicate the planner and four
backends, and, decisively, would have to *read the graph out of the store to evaluate*, which on
D1 is the network. Rules must run where the data is.

### D24 SQLite's recursive-CTE restrictions are the safety conditions

Stratification and linearity are not extra rules we impose; they are what SQLite already enforces,
so the compiler surfaces them as diagnostics instead of working around them.

`WITH RECURSIVE` permits exactly one reference to the CTE in the recursive term's `FROM` and
nowhere else — not in a subquery, not under `NOT EXISTS`, not on the null-padded side of a
`LEFT JOIN` — and no aggregate or window function in the recursive term. Read as a Datalog spec:
one self-reference means rules must be **linear**; no self-reference under `NOT EXISTS` means
negation must be **stratified**; no aggregate in the recursive term means aggregation must be
**stratified**. A program that violates any of them is rejected by `program.rs` with the cycle
named, before SQL exists. Rejected: emitting SQL and letting SQLite produce "recursive reference
in a subquery" at execution time, which would surface an implementation detail as a user error.

### D25 One statement where SQLite allows it, iteration where it does not

A recursive component compiles to a `WITH RECURSIVE` member of the same statement. A component
whose rules are non-linear cannot, so it is iterated to a fixpoint in a `datalog_work` table, one
request per round.

`compiler/ops.rs` already builds property-path CTEs this way and `reason.rs` already nests a
`WITH RECURSIVE` inside a subquery, so the in-statement shape is proven on all four backends.
`UNION` rather than `UNION ALL` deduplicates, which is both Datalog's set semantics and what makes
cyclic data terminate — the same choice `reason.rs`'s transitive closure makes. Iteration is the
fallback rather than the default because each round is a network hop on D1, which is the cost this
project is built to avoid; the compiler therefore keeps everything it can in one statement and
`explain()` says when it could not. The rounds are naive rather than semi-naive: the work table's
primary key covers every column, so `INSERT OR IGNORE` deduplicates and the row count is monotone,
which makes the fixpoint detectable with a count instead of a delta relation. A delta would cut
re-derivation but not round trips, which are what dominate here.

### D25a The work table is scoped by run, not by connection

Iteration stages its rows in a persistent `datalog_work` table keyed by a run id, rather than in a
`TEMP` table.

A fixpoint spans several requests, and on D1 those requests are not one connection, so a temporary
table would be gone by the second round. A persistent table with a `run` column survives, keeps
concurrent evaluations from seeing each other, and makes cleanup exact — the evaluation deletes
its own rows and nobody else's. The cost is that a Datalog program with a non-linear component
needs write access; that is stated in the spec rather than discovered at runtime.

### D26 Mutual recursion is one CTE with a discriminant

An SCC with several predicates becomes a single CTE carrying a `tag` column, with one recursive
term per rule, each referencing the CTE exactly once.

SQLite has no mutually recursive CTEs, but since 3.34.0 the recursive term may be a compound, and
each arm may hold its own single self-reference. Tagging is the standard encoding and keeps mutual
recursion on the one-round-trip path instead of demoting it to the fixpoint loop. Because 3.34.0
is not universal and D1's version is not ours to assume, `Capabilities` gains
`compound_recursive_cte`; when it is false, a multi-predicate SCC takes the fixpoint fallback
rather than emitting SQL that will fail.

### D27 Derived facts live in `quads_inf`, not a new table

`datalog_materialize` writes into the inference table that OWL-RL materialization already uses, and
reuses `materialize_reset`.

`openspec/specs/reasoning/spec.md` already requires inferences to be stored separately from
asserted data and re-running to replace the previous set; SPARQL and Cypher already read that table
through the `inferred` option. Sharing it means a user rule is visible to every dialect the moment
it is materialised, with no new schema, no new option and no new cache to invalidate — rules become
an extension of the reasoner rather than a second inference universe. The trade-off is that
Datalog materialization and OWL-RL materialization share one table and therefore one lifecycle:
running either replaces the whole inferred set. That is stated in the spec rather than hidden.

## Risks / Trade-offs

- **Scope.** This is the largest frontend since Cypher (~13.3k lines). Mitigated by shipping in
  five slices, each independently useful: non-recursive first, then linear recursion, then
  negation and aggregation, then mutual/non-linear recursion, then materialization and bindings.
- **A non-linear program costs a round trip per round.** Worst on D1, which is the headline
  target. Mitigated by keeping every expressible component in one statement, by bounding the
  rounds, and by reporting both the strategy and the rounds taken so the cost is never a surprise.
  Non-linear rules also converge by doubling, so the round count is usually small.
- **Iteration needs write access.** A read-only store cannot evaluate a non-linear component.
  Stated in the spec; every other program remains read-only.
- **Two materialization sources, one table.** Stated in the spec; a future change can namespace
  `quads_inf` by producer if it proves painful.
- **A new surface to keep Oxigraph-compatible.** It is not: Datalog is additive and Oxigraph has no
  equivalent, so `oxigraph-compatibility` is untouched.

## Migration Plan

Additive throughout. The crate is behind a `datalog` feature that is off by default, the one
`Capabilities` field defaults to the conservative value, and no storage schema changes. A store
that never calls a Datalog entry point behaves identically, and existing databases need no
migration.
