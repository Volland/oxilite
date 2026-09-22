## Context

See proposal.md for the motivation. The main constraint is that the target engine may be remote: on Cloudflare D1 each request is a network round-trip, billed per row read and written, capped at about 100 KB per statement and 100 bound parameters, and atomic only within one `batch()`. The design also has to work on native SQLite in-process, where round-trips are cheap and interactive transactions exist. Interview decisions D1–D10 are recorded in `lat.md/decisions.md`.

## Goals / Non-Goals

**Goals:**
- One sans-IO core shared by native, D1 and JavaScript hosts.
- Read-free writes and lookup-free query compilation, both enabled by deterministic ids.
- Index-only scans for every triple pattern.
- A single SQL statement per BGP query, with a join order we choose.
- API parity with `oxigraph::store::Store`, checked by the compatibility harness (change `oxigraph-compat-harness`).

**Non-Goals (for M1):**
- OPTIONAL, UNION, MINUS, aggregates, property paths and subqueries all compile in M2. In M1 they go through the fallback on native backends.
- SPARQL UPDATE compilation comes in M3. M1 only provides store-level writes.
- D1 and JavaScript drivers (M3 / TypeScript bindings), reasoning (M4) and validation (M5).
- Garbage collection of unreferenced terms.

## Decisions

### Sans-IO step machines
Every operation returns either `Execute(Request)` or `Done(output)` and is resumed with the `Response`. The drivers are tiny: a loop for sync backends, an `.await` loop for async ones, a JavaScript loop for wasm. *Alternative:* separate sync and async implementations of the Store. Rejected because every feature would need to be written twice and D1 would inevitably fall behind.

### Id layout
The sign bit is 0, followed by 4 tag bits and a 59-bit payload.
- **Hashed kinds** use xxh3-64 of a canonical key (tag byte + NUL-separated parts), masked to 59 bits.
- **Inline integers** use `value + 2^58`, so ids are order-preserving.

*Alternatives:*
- 128-bit keys (two columns): wider indexes and slower comparisons, for a collision risk that detection already covers.
- Autoincrement ids: need a read-back on every insert (decision D2).

Collisions are caught by a `BEFORE INSERT` trigger on `terms` that calls `RAISE(ABORT)`, which rolls back the whole batch on both SQLite and D1.

### Schema
`quads` is `WITHOUT ROWID` with PK (s,p,o,g), indexes (p,o,s,g) and (o,s,p,g), and an optional (g,s,p,o). Secondary indexes on a WITHOUT ROWID table include every PK column, so all four are covering. `terms` uses `INTEGER PRIMARY KEY` (the rowid) for the fastest lookups, plus partial indexes on `num` and `ts`. The default graph is `g = 0`, because NULL would break PK uniqueness. Tables are `STRICT` so a wrong type fails loudly.

### SQL generation style
All constants are inlined as literals: ids are integers and strings are escaped, and strings containing NUL are written as `CAST(X'..' AS TEXT)`. This sidesteps D1's 100-parameter limit and lets a writer pack many rows per statement. It's safe because we never interpolate unescaped text. Variables compile to columns `vN`. Hashed term attributes are read with correlated scalar subqueries on `terms` rather than LEFT JOINs, because SQLite evaluates a WHERE term at the earliest loop level where its dependencies are available. A filter on the first scanned pattern is therefore applied before the next pattern is scanned.

### Join order enforcement
The greedy planner's order is emitted as `quads q1 CROSS JOIN quads q2 …`. SQLite never reorders across `CROSS JOIN`, but it still picks the best index for each alias given the bound columns. Blocks merged from different sub-patterns are joined with a plain `JOIN`, so SQLite may interleave them.

### Result decoding
The main query returns ids. They go back as TEXT when the capability `int64_as_text` is set, because JavaScript numbers can't hold 60-bit ids. Inline ids decode arithmetically, and constants from the query decode from a map built at compile time. Remaining ids are deduplicated and resolved in one extra request (`SELECT … FROM terms WHERE id IN (…)`, chunked). Deduplication makes this cheaper than joining `terms` for every row and column.

### Fallback
`Unsupported` from the compiler triggers `spareval` evaluation over a `QueryableDataset` implemented on the sync backend. `internalize_term` is just the hash; `externalize_term` is a cached `terms` lookup; `internal_quads_for_pattern` is one SQL scan. This provides full SPARQL 1.1 coverage on native backends from day one.

### Dynamic libsqlite3
`libloading` resolves only the C functions oxilite uses:
- `sqlite3_open_v2`, `close_v2`, `prepare_v2`, `step`, `finalize`
- the `column_*` functions
- `errmsg`, `changes`, `exec`
- `create_function_v2`, for UDFs when the library supports them

Everything else is unnecessary because oxilite doesn't use bound parameters.

## Risks / Trade-offs

- **Hash collision rejects a legitimate write.** At about 1e-9 odds for 190k terms this is acceptable; the error names the colliding id. A salted-rehash scheme can come later.
- **Decimals are stored as REAL for comparisons, so precision is lost beyond 15 digits.** Only the comparison value is approximate; the lexical form is exact and results round-trip exactly.
- **The first scalar-subquery lookups may be slower than a join on wide projections.** Final decoding is batched separately; `explain()` shows the SQL for profiling.
- **Parenthesised joins and row-value `IN` need SQLite ≥ 3.15.** Opening the store checks the version.
- **The dlopen backend's ABI mismatches with exotic builds.** Only the stable C API is used, and symbols are checked at open.
- **Stale statistics lead to bad plans.** `optimize()` runs automatically after the bulk loader, and heuristics cover a store that has never been optimized.

## Migration Plan

This is a new project. The schema version is written to `oxilite_meta`; later milestones add tables idempotently (`CREATE … IF NOT EXISTS`).

## Open Questions

- Should `optimize()` also run `PRAGMA optimize`/`ANALYZE` on backends that allow it? This only helps index selection and can be decided while benchmarking in M6.
