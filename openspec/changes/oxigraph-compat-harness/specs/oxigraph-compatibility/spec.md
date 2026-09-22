## Purpose

Checks mechanically that oxilite behaves like Oxigraph, by running Oxigraph's own tests and a differential comparison against an in-memory Oxigraph store.

## ADDED Requirements

### Requirement: Common engine interface
The harness SHALL drive oxilite and Oxigraph through one interface covering these operations, so every test runs unchanged on both:
- loading data in a given format into a given graph
- running a SPARQL query or update
- scanning quads by pattern
- dumping a dataset
- managing named graphs

#### Scenario: Same test, two engines
- **WHEN** a test case is written once against the common interface
- **THEN** it runs against Oxigraph's in-memory store and against every oxilite backend under test

### Requirement: W3C conformance through Oxigraph's runner
The harness SHALL execute the W3C `rdf-tests` manifests (RDF syntax suites, SPARQL 1.1 query and update, SPARQL 1.2 where Oxigraph runs them) and Oxigraph's own `oxigraph-tests` manifests. It SHALL use the manifest runner ported from Oxigraph's `testsuite` crate, with Oxigraph's list of ignored tests as the starting baseline.

#### Scenario: Expected-result check
- **WHEN** a W3C evaluation test runs on oxilite
- **THEN** its result is compared with the manifest's expected result, using the same comparison rules as Oxigraph (solution multisets, isomorphic graphs, ordered comparison for ORDER BY)

#### Scenario: Baseline is Oxigraph's own
- **WHEN** a test is on Oxigraph's ignore list
- **THEN** it is reported as "ignored upstream" rather than as an oxilite failure

### Requirement: Differential comparison
The harness SHALL run every query of its differential corpus on both engines over identical data and compare the results:
- SELECT: as multisets of solutions, or as sequences when the query has ORDER BY
- CONSTRUCT and DESCRIBE: as isomorphic graphs
- ASK: as booleans

For updates, it SHALL compare the resulting datasets.

#### Scenario: Divergence is reported
- **WHEN** oxilite and Oxigraph return different results for a corpus query
- **THEN** the harness fails and reports the query, the dataset, both results and the SQL oxilite generated

#### Scenario: Plans and backends do not change results
- **WHEN** a corpus query runs with statistics present, with statistics absent, and with SQLite planning
- **THEN** all runs return the same results as Oxigraph

### Requirement: Store API parity
The harness SHALL run Oxigraph's store API tests (`lib/oxigraph/tests/store.rs`) against `oxilite::blocking::Store`, changing only the import path.

#### Scenario: Drop-in compile
- **WHEN** the ported store tests are built
- **THEN** they compile against oxilite without any other source changes, except tests of RocksDB-specific methods, which are listed as excluded

### Requirement: JavaScript API parity
The harness SHALL run Oxigraph's TypeScript store tests (`js/test/store.test.ts`) against the oxilite Node.js package, with only the import changed.

#### Scenario: JS tests pass
- **WHEN** the ported JS store tests run under Node.js
- **THEN** every test passes, or is listed as a known difference with a reason

### Requirement: Known differences are explicit
The harness SHALL keep every accepted divergence from Oxigraph in an allow-list file, each with a reason and a link to the decision that justifies it. It SHALL generate a compatibility report from the test results.

#### Scenario: Unlisted divergence fails
- **WHEN** a divergence is not in the allow-list
- **THEN** the harness fails

#### Scenario: Stale allow-list entry
- **WHEN** an allow-listed divergence no longer occurs
- **THEN** the harness reports the entry as removable

### Requirement: D1 coverage
From milestone M3, the harness SHALL run the differential corpus and the W3C query and update suites against D1 through local `wrangler dev`, in addition to native backends.

#### Scenario: Same results on D1
- **WHEN** the corpus runs on the D1 backend
- **THEN** results equal Oxigraph's, except for allow-listed differences specific to D1 (for example, features that need the fallback evaluator)
