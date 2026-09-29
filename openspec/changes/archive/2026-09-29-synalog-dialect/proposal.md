## Why

oxilite's Datalog dialect ([[architecture#Datalog frontend]]) is RDF-native: positional atoms over
triple patterns, set semantics, a true least fixpoint. It is the right tool for rules *about the
graph*, but it is not the language agents are being taught to write.

[Synalog](https://github.com/SynaLinks/synalog) is a Datalog-family language (a Rust rewrite of
Logica) built for AI agents: named arguments, full expressions, head aggregation (`+=`, `Min=`,
`List=`…), functors, `@OrderBy`/`@Limit` for pageable results, and a compile-time verifier whose
errors are written to be fed back to a model. It compiles to SQL — SQLite among seven engines — and
ships an agent skill. Version 1.2.0 is published on crates.io with no default features, so it can
be a normal, publishable dependency.

The two do not compete: ours reasons over RDF terms and materializes into `quads_inf`; Synalog is a
semantic layer over *relational* data. What is missing is the bridge — a way to run a Synalog
program where oxilite's data lives, on every backend oxilite supports, D1 included.

## What Changes

- A new crate `oxilite-synalog`, behind an off-by-default `synalog` feature of `oxilite`, depending
  on `synalog = { version = "1.2", default-features = false }`. Synalog's parser, verifier and
  compiler are used as they are; nothing is forked.
- **The dialect on its own**: `check` (the Synalog verifier) and `compile_for_engine` (SQL for any
  of Synalog's seven engines), usable without a store.
- **The dialect over the triple store**: the store appears to a program as relational tables —
  `triples(subject, predicate, object, kind, datatype, lang, graph)` with terms decoded to native
  SQL values, plus predicate and class tables declared with `# @table` / `# @class` pragmas (a
  Synalog comment, so the program stays valid Synalog). The tables are common table expressions
  injected in front of the compiled statement: no schema change, no persisted views, and they honour
  the same graph scope, inference and time-travel options as SPARQL and Datalog.
- Synalog's SQLite output leans on Logica runtime functions that D1 cannot register. The portable
  ones are rewritten to plain SQLite; the runtime-only ones (`ArgMax`, `ReadFile`, `@Ground`…) are
  rejected before anything runs, naming the construct.
- `Store::synalog`, `Store::synalog_with`, `Store::synalog_sql` and the async equivalents, driven by
  a sans-IO `SynalogJob`: one request per program on every backend.
- `oxilite synalog PREDICATE` on the command line: run against a store, print the store SQL, or
  compile for another engine with `--engine`.
- `synalog(program, predicate, options)` and `synalogSql` in `@oxilite/node`, `@oxilite/d1` and the
  Python package, over JSON forms of the options and results; an opt-in `synalog` feature of the
  WebAssembly core (it adds about 2 MB, so the default D1 bundle does not carry it).

## Non-goals

- Materializing Synalog results into `quads_inf`: results are SQL values, not RDF terms, and a
  mapping back is a separate design.
- Changing Synalog's semantics (multisets, bounded recursion by unrolling) — the store adapts to
  the language, not the other way round.

## Impact

- New crate `crates/oxilite-synalog`; new optional dependency `synalog` (Apache-2.0, compatible
  with oxilite's MIT OR Apache-2.0).
- `oxilite` gains the `synalog` and `synalog-json` features; `oxilite-cli`, the Node and Python
  bindings enable them; `oxilite-wasm` gains an opt-in `synalog` feature.
- No change to the storage schema, the default build, or any existing API.
