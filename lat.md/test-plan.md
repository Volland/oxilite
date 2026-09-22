# Test plan

Planned tests, built around one principle: oxilite is correct when it behaves like Oxigraph. When a test is implemented, its section moves to [[tests]] and gains an `@lat:` reference in code.

## Oxigraph compatibility harness

The primary test asset: a dev-only crate `oxilite-compat` (`testsuite/`) that runs identical operations on oxilite and on in-memory Oxigraph and compares them. Specified in change `oxigraph-compat-harness`.

Both engines implement one `Engine` trait (load, query, update, pattern scan, dump, named graphs), so every test is written once. The `oxigraph` crate is a dev-dependency with `default-features = false` (in-memory, no RocksDB). Because oxilite returns the same `spareval::QueryResults` and `oxrdf` types ([[decisions#D9 Oxigraph-mirroring API, async first]]), Oxigraph's own comparison code applies unchanged.

### Reused Oxigraph test assets

Tests are ported from the Oxigraph repository rather than rewritten, so compatibility is measured against Oxigraph's own expectations.

| Oxigraph source | Use in oxilite |
|---|---|
| `testsuite/src/{manifest,sparql_evaluator,parser_evaluator,evaluator,report}.rs` | W3C manifest runner, made generic over `Engine` |
| `testsuite/rdf-tests` (submodule) | W3C syntax, SPARQL 1.1/1.2 query and update suites |
| `testsuite/oxigraph-tests` | Oxigraph-specific SPARQL regression manifests |
| `testsuite/tests/sparql.rs` ignore lists | Baseline: tests Oxigraph itself skips are "upstream", not oxilite failures |
| `lib/oxigraph/tests/store.rs` | Store API tests run on `oxilite::blocking::Store` with only the import changed |
| `lib/oxigraph/tests/optimizer_regression.rs` | Queries added to the differential corpus |
| `js/test/store.test.ts` | JS API tests run on `@oxilite/node` with only the import changed |

### Three verdicts per W3C test

Each evaluation test compares oxilite with the expected result, Oxigraph with the expected result (sanity), and oxilite with Oxigraph.

An oxilite failure is either "oxilite wrong while Oxigraph right" or "engines disagree". When both engines miss the expected result, the test is reported as upstream.

### Differential corpus

Seeded generated datasets and templated query families run on both engines over identical data; results compare as multisets, ordered sequences under ORDER BY, isomorphic graphs, or booleans.

Datasets cover every literal kind (inline and hashed integers, decimals, doubles, dates, booleans, language and directional strings), blank nodes, several named graphs, triples duplicated across graphs, deep class hierarchies and cycles. Query families grow per milestone (M1 BGP shapes × filters × dataset clauses × modifiers; M2 OPTIONAL/UNION/MINUS/aggregates/paths/subqueries; M3 updates, comparing resulting datasets). Every query runs with statistics absent and present, and with the oxilite planner and SQLite planning, since plans must never change results. A mismatch report includes the query, the data seed, both results and oxilite's `explain()` SQL.

### Allow-list and report

Accepted divergences live in `testsuite/allowlist.toml`, each with a reason and a link to a decision; unlisted divergences fail, and stale entries are reported.

A generated `COMPATIBILITY.md` shows pass rates per suite and the divergence table.

### Backend matrix

The harness runs on rusqlite and the dylib backend for every milestone, and on D1 through `wrangler dev --local` from M3.

## Conformance gates

Milestone done-criteria measured with the harness, see [[milestones]].

- M1: W3C Turtle, TriG, N-Triples, N-Quads syntax suites load → dump → isomorphic on both engines; ported store API tests pass.
- M2: ≥95% of W3C SPARQL 1.1 query evaluation tests; zero unlisted divergences on the corpus.
- M3: W3C SPARQL 1.1 update suite on rusqlite and D1; the corpus on D1.
- M5: rudof SHACL/ShEx suites over an oxilite-backed store equal rudof over an in-memory graph.
- M7: ≥80% of read-only openCypher TCK scenarios (96.4% today); the specification scenarios on rusqlite, dylib and D1.

## Storage unit tests

Focused tests for [[architecture#Storage schema]] and [[architecture#Write path]] that the harness cannot express.

- Insert is idempotent and reports newness; remove reports presence.
- The hash-collision trigger aborts the whole batch (simulated by pre-inserting a conflicting row).
- Named graphs created with `CREATE GRAPH` are listed while empty; `DROP` removes them.
- `quads_for_pattern` returns the same quads for all 16 bound/unbound combinations as a naive filter.

## Compiler unit tests

Focused tests for [[architecture#SPARQL to SQL compiler]].

- A compiled query is one SELECT plus at most one term-resolution round-trip (counted by a recording backend).
- SPARQL error semantics: `FILTER(?x > 5)` on a string is false, `!(error)` is false, `error || true` is true.
- OPTIONAL with BIND inside does not leak constants into unmatched rows.
- Recursive paths terminate on cycles; seeded and unseeded forms agree.

## Planner unit tests

Focused tests for [[architecture#Query planner]].

- A rare predicate is scanned before `rdf:type` when stats are present.
- Each pattern after the first shares a variable with earlier ones when the join graph is connected.

## D1 tests

Tests specific to the D1 backend and TypeScript driver.

- Ids above 2^53 survive the round-trip (transported as TEXT).
- A large update is one atomic batch; a failure leaves the store unchanged.
- No statement exceeds the configured size limit.
- The D1 driver passes its store API tests against a local D1 (Miniflare).

## Cypher tests

Tests of [[architecture#Property graph frontend]] (M7), implemented in `crates/oxilite-cypher/tests` and listed in [[tests#Cypher]].

- The openCypher TCK runs through the Gherkin runner in `tests/tck.rs`; failures are allow-listed with their message.
- The specification scenarios run on the bundled SQLite, the system `libsqlite3`, the D1 code path and Miniflare D1.
- A differential corpus poses the same questions in SPARQL and Cypher (`tests/differential.rs`).
- Still planned: TCK runs on the dylib and D1 engines.
