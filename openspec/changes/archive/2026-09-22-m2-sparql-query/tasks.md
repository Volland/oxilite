## 1. Graph patterns

- [x] 1.1 OPTIONAL as a LEFT JOIN, with parenthesised joins for multi-pattern right sides and sealing of right sides that have computed columns
- [x] 1.2 UNION ALL with NULL padding and mixed Id/Val variable handling
- [x] 1.3 MINUS and NOT EXISTS with compatibility and domain-overlap conditions
- [x] 1.4 FILTER EXISTS with outer-binding substitution (outer scope stack)
- [x] 1.5 VALUES (including UNDEF, zero rows, zero variables) and BIND
- [x] 1.6 Subqueries: sealing with ORDER BY/LIMIT inside, variable scoping

## 2. Aggregates

- [x] 2.1 GROUP BY over Id and Val variables; one solution for implicit grouping over empty input
- [x] 2.2 COUNT / COUNT DISTINCT / COUNT(*) producing inline integer ids
- [x] 2.3 SUM / AVG with numeric type promotion; MIN / MAX / SAMPLE using the bare-column rule; GROUP_CONCAT
- [x] 2.4 HAVING and ORDER BY on aggregates

## 3. Property paths

- [x] 3.1 Rewrite sequence, alternative and inverse paths into joins and unions
- [x] 3.2 Negated property sets
- [x] 3.3 Recursive CTEs for `*`, `+`, `?`, seeded from constants
- [x] 3.4 Seeding from left-hand bindings (magic-set style) for correlated paths
- [x] 3.5 Graph-carrying CTEs for paths inside `GRAPH ?g`

## 4. Functions and fallback

- [x] 4.1 Tier B rewrites (simple REGEX, langMatches, casts)
- [x] 4.2 Tier C through UDFs on native backends
- [x] 4.3 Partial fallback: SQL below the unsupported operator, Rust above it
- [x] 4.4 `explain()` with join order, estimates, fallback reasons and warnings

## 5. Verification

- [x] 5.1 Compatibility harness M2 query families: OPTIONAL, UNION, MINUS, aggregates, paths, subqueries
- [x] 5.2 W3C SPARQL 1.1 query suite ≥ 95%, remainder allow-listed
- [x] 5.3 Update `lat.md/` (compiler sections, tests) and run `lat check`
