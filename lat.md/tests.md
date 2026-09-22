---
lat:
  require-code-mention: true
---
# Tests

Test specifications that are implemented in code; each leaf is referenced by exactly one `@lat:` comment next to its test. Planned tests live in [[test-plan]].

## Encoding

Unit tests of the tagged 64-bit term encoding described in [[architecture#Term encoding]].

### Inline integers sort by value

Canonical integers in ±2^58 encode inline, ids are positive and ordered like the values, decode back exactly, and out-of-range values are not inlined.

### Non-canonical literals are hashed

`"012"^^xsd:integer` is a different term from `"12"^^xsd:integer`: it is hashed (Typed tag) with its numeric value 12 in the side columns, preserving term identity.

### Simple literal equals xsd string

A simple literal and the same lexical form typed `xsd:string` get the same id (RDF 1.1), while a language-tagged literal with that form gets a different one.

### Tags partition the id space

An IRI and a blank node with the same text get different ids, and each id falls in the range reserved for its tag, so kind checks are integer range checks.

## Write path

Unit tests of statement generation for inserts, see [[architecture#Write path]].

### Statements respect the size limit

Inserting 500 quads under a 2 000-byte statement limit yields several statements, none longer than the limit and none using bound parameters.

## Planner

Unit tests of the greedy join-order planner, see [[architecture#Query planner]].

### Rare predicate first

With statistics showing `ex:rare` has 10 triples and `rdf:type ex:Common` 1 000, the `ex:rare` pattern is ordered first.

### Connected patterns are preferred

In a chain of patterns, every pattern after the first shares a variable with an earlier one, so no Cartesian product is introduced.

### Heuristics without statistics

Without statistics, a pattern with a constant subject is ordered before one with only a constant predicate.

## Backends

Tests of the native backends, see [[architecture#Backends]].

### Dylib loads a system SQLite

A system `libsqlite3` found at a well-known path is loaded at runtime; values of every storage class round-trip and a failing atomic request rolls back.

### Invalid library is reported

Opening the dylib backend with a nonexistent library path fails with an error naming the library.

## Store

Integration tests of the blocking store against the M1 specifications.

### Queries are single statements

A five-pattern BGP with a filter uses at most two backend requests (the query plus one term-resolution round-trip), and `explain()` reports it fully compiled.

### Collision aborts the batch

A forged dictionary row that collides with a term being inserted makes `extend` fail with a collision error, and none of the batch's quads are stored.

### Reopen keeps data

Quads and named graphs written to a database file are still present after reopening it, and `validate()` passes.

### Graph index is optional

A store created with `graph_index: false` has only the `posg` and `ospg` secondary indexes on `quads`.

### Insert and remove report changes

Inserting an existing quad or removing an absent one reports `false`; the non-canonical literal `"012"^^xsd:integer` round-trips exactly.

### Pattern scans match a naive filter

For all 16 combinations of bound and unbound positions, `quads_for_pattern` returns exactly the quads a naive filter selects.

### Planner uses statistics

After `optimize()`, the generated SQL scans the rare predicate before the `rdf:type` pattern, and results are unchanged.

### Union default graph deduplicates

With the union-default-graph option a triple present in two named graphs matches once, while `GRAPH ?g` still returns both graphs.

### Dylib store works end to end

A store opened on a system `libsqlite3` through `Store::open_with_library` loads Turtle and answers an aggregate query with a filter.

## Oxigraph compatibility

Suites of the compatibility harness, see [[test-plan#Oxigraph compatibility harness]].

### Ported Oxigraph store API tests

Oxigraph 0.5.11 `lib/oxigraph/tests/store.rs`, copied with only `oxigraph::` replaced by `oxilite::`, passes against `oxilite::store::Store`; RocksDB-only tests are compiled out by their feature gates.

### Differential corpus matches Oxigraph

About 120 query families over a seeded dataset give the same results on Oxigraph and on every oxilite variant, for two seeds; divergences must be allow-listed.

The dataset covers every literal kind, blank nodes, named graphs with cross-graph duplicates, a class hierarchy, cycles and triple terms.

### Optimizer regression query

Oxigraph's OPTIONAL-on-foreign-key regression (20 persons × 20 orders) returns Oxigraph's results, and the OPTIONAL's BGP is planned starting from the foreign-key lookup on the bound `?c`.

### Update corpus matches Oxigraph

Every SPARQL UPDATE in the differential update corpus, applied to the seeded dataset, leaves oxilite and Oxigraph with the same quads; divergences must be allow-listed.

## D1

`@oxilite/d1` tests running the wasm core against a Miniflare D1 database, see [[architecture#Backends#Cloudflare D1]].

### Store API on Miniflare D1

`add`, `has`, `delete`, `size` and `match` on `D1Store` behave like Oxigraph's JavaScript store API against a real D1 binding.

### Ids above 2^53 survive D1

Fifty hashed terms (60-bit ids) round-trip through D1 and JavaScript intact, proving ids travel as TEXT and no precision is lost above 2^53.

### Failed update leaves D1 unchanged

An update whose later operation fails (`CREATE GRAPH` on an existing graph) aborts its whole batch, so the earlier `INSERT DATA` is not visible.

### Large loads respect D1 limits

A 6000-triple bulk load is split into batches under D1's statement limits, all triples arrive, and statistics are refreshed for `explain()`.

## Node

`@oxilite/node` tests over the napi-rs addon, next to the verbatim port of Oxigraph's `js/test/store.test.ts`, see [[architecture#Bindings]]. The port's failures must match `js:` entries of `testsuite/allowlist.toml`.

### File store persists across processes

A child Node process writes a quad to a SQLite file; a store reopened on that file in the test process sees it.

### Explain returns SQL

`explain()` returns the generated SQL for a SELECT, and `explainUpdate()` describes a compiled update.

## Planner benchmark

`cargo run --release -p oxilite --example planner_bench` loads 350 010 quads (50 000 people) and compares the oxilite planner with SQLite's planner on three join-heavy queries.

Measured on an Apple Silicon laptop (M1 milestone):

| query | oxilite planner | SQLite planner |
|---|---|---|
| rare-badge-star | 75 µs | 7.2 ms |
| friends-of-badged | 96 µs | 87 µs |
| city-age-filter | 478 µs | 7.8 ms |
