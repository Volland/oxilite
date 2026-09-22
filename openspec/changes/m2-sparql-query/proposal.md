## Why

M1 compiles only BGPs, filters and solution modifiers; everything else goes through the per-pattern fallback. That fallback is slow on native backends and unavailable on D1. Real SPARQL workloads rely on OPTIONAL, UNION, aggregates and property paths, so these must compile to single SQL statements too.

## What Changes

- Compile OPTIONAL (LEFT JOIN with filter in `ON`), UNION, MINUS, FILTER EXISTS / NOT EXISTS, VALUES, BIND, and subqueries.
- Compile GROUP BY and aggregates: COUNT, SUM, AVG, MIN, MAX, SAMPLE, GROUP_CONCAT, and HAVING.
- Compile property paths:
  - sequence, alternative and inverse paths are rewritten into joins and unions;
  - `*`, `+` and `?` become recursive CTEs, seeded from constant endpoints or from the bindings of the left-hand block;
  - paths inside `GRAPH ?g` get a graph-carrying CTE.
- Complete the function tiers: B rewrites, and C through native UDFs or fallback.
- Add a finer-grained fallback: when only one operator can't be compiled, SQL still evaluates everything below it and Rust evaluates the rest.
- Add `explain()`, which returns the SQL, the chosen join order, the estimated cardinalities, and any fallback or performance warnings.

## Capabilities

### New Capabilities
- `sparql-full-query`: SPARQL 1.1 query semantics beyond BGPs, all compiled to SQL.
- `query-explain`: inspecting how a query is compiled and executed.

### Modified Capabilities
<!-- none (sparql-bgp-query is extended by new requirements in sparql-full-query) -->

## Impact

- `oxilite-core` compiler modules (patterns, paths, aggregates, functions).
- The `oxilite-compat` corpus gains M2 query families; the W3C SPARQL 1.1 query suite becomes the gating measure (≥ 95%).
