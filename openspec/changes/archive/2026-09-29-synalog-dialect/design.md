## Context

Synalog compiles a named predicate to one SQL string for a chosen engine. Tables are referenced by
lowercase name (`orders(amount:)`), predicates are PascalCase and become common table expressions
(`t_N_Name`), recursion is unrolled into `@Recursive(P, N)` chained CTEs, and negation compiles to a
correlated `MIN(MagicalEntangle(1, …)) IS NULL`. Its SQLite dialect assumes the Logica Python
runtime has registered helper functions on the connection (`MagicalEntangle`, `ArgMax`,
`IN_LIST`…).

oxilite stores quads as tagged 64-bit term ids ([[architecture#Term encoding]]) and runs every
query through the sans-IO job protocol, on backends that cannot register functions (D1).

## Goals / Non-Goals

Goals: run unmodified Synalog programs over the store on every backend; keep Synalog usable as a
plain dialect for its other engines; one request per program; errors before execution for anything
the store cannot run.

Non-goals: materialization, forking or patching Synalog.

## Decisions

### Depend on the published crate, never fork

`synalog = { version = "1.2", default-features = false }` pulls no pyo3, wasm-bindgen or database
drivers — only `indexmap`, `regex`, `serde(_json)`, `thiserror`, `anyhow`. Edition 2024 needs rustc
1.85, which is the workspace MSRV. Everything oxilite-specific happens around the compiled SQL, so a
Synalog upgrade is a version bump.

### The store is relational tables injected as CTEs

A program on the store reads tables that do not exist in the schema; they are prepended to the
compiled statement as `name AS NOT MATERIALIZED (…)`. Persisted views were rejected: they would be a
schema change (a D1 migration), could not follow per-call options, and would appear in dumps. CTEs
cost nothing when unused — only the tables a statement references are injected — and follow the
options of each call:

- the quad source is `quads`, `quads ∪ quads_inf` under `include_inferred`, or the time-travel
  relation under `as_of` — the same three the Datalog compiler chooses between;
- the default graph only, unless `union_default_graph`, as for SPARQL.

`NOT MATERIALIZED` lets SQLite push a caller's filters into a table referenced more than once
instead of building it in full.

### Terms become native SQL values

Synalog compares and computes with SQL semantics, and SQLite orders every TEXT after every number,
so `age > 18` over lexical forms would be silently wrong. The `object` column is therefore a native
value: inline integers decode from the id with no lookup, other numeric literals read `terms.num`,
booleans become 1/0, blank nodes `_:label`, and IRIs and other literals their lexical form. The RDF
information is kept beside it: `kind` (`iri`, `blank`, `literal`, `triple`), `datatype`, `lang`.
Triple terms decode to NULL in this version.

### Declared tables use term ids, not strings

Filtering `triples` by a predicate string decodes every quad before comparing. A predicate table
declared with `# @table name <iri>` instead compiles to `WHERE q.p = <id>` with the id computed in
Rust, which the `posg` index answers; `# @class name <iri>` is `rdf:type` with a fixed object. The
pragmas are Synalog comments, so the same program still checks and compiles with Synalog's own CLI.
`Options::tables` declares the same tables from code. Synalog rejects unknown `@` annotations, which
ruled out a directive.

### Rewrite what SQLite can express, reject what it cannot

The compiled SQL is scanned (string- and parenthesis-aware) for Logica runtime calls:

| Call | Rewritten to |
|---|---|
| `MagicalEntangle(a, b)` | `(a)` |
| `IN_LIST(x, l)` | `(x) IN (SELECT value FROM JSON_EACH(l))` |
| `JOIN_STRINGS(l, s)` | `GROUP_CONCAT` over `JSON_EACH(l)` in key order |
| `DistinctListAgg(x)` | `JSON_GROUP_ARRAY(DISTINCT x)` |
| `SortList(l)` | `JSON_GROUP_ARRAY` over `JSON_EACH(l)` in value order |

`ArgMin`/`ArgMax` (aggregates with no SQLite form), `ReadFile`, `WriteFile`, `Fingerprint`,
`Intelligence`, `RunClingo`, `AssembleRecord`, `DisassembleRecord`, `PrintToConsole` and
BigQuery-only `ARRAY_AGG` are rejected with an `Unsupported` error naming the call. So is any
output that is more than one statement — `@Ground`, `@AttachDatabase`, and any `@Recursive` bound
above 20, which Synalog evaluates iteratively through tables it creates — because a read must not
write. `Printf` needs nothing: SQLite function names are case-insensitive.

### One request, D1 limits checked before sending

`SynalogJob` sends one read request and decodes nothing: values are already SQL values, returned
with the predicate's head columns. Before sending, the statement is checked against the backend's
`max_sql_len` and `max_compound_select` (D1: 100 KB, 5 terms), since unrolled recursion grows with
its bound.

## Risks / Trade-offs

- Multiset semantics: without `distinct`, unrolled recursion over cyclic data duplicates rows. This
  is Synalog's documented behaviour, kept.
- Decoding costs a `terms` lookup per non-inline term; declared tables avoid scanning, not decoding.
- Synalog is young; its compiler is wrapped so a panic becomes an error rather than an abort.
