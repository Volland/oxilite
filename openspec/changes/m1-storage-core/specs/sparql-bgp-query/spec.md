## Purpose

Evaluates SPARQL queries built from basic graph patterns, filters, GRAPH and solution modifiers by compiling each one to a single SQL statement that runs inside SQLite.

## ADDED Requirements

### Requirement: Query forms
The system SHALL evaluate SELECT, ASK, CONSTRUCT and DESCRIBE queries, returning Oxigraph-compatible results: solutions with ordered variables, a boolean, or triples.

#### Scenario: SELECT
- **WHEN** `SELECT ?s WHERE { ?s a <http://example.com/C> }` runs on a store with two instances of the class
- **THEN** two solutions are returned, each binding `?s`

#### Scenario: CONSTRUCT
- **WHEN** a CONSTRUCT query with a template runs
- **THEN** the template is instantiated for every solution, with fresh blank nodes per solution

#### Scenario: DESCRIBE
- **WHEN** `DESCRIBE <x>` runs
- **THEN** every default-graph triple whose subject is `<x>` is returned

### Requirement: Single-statement compilation
The system SHALL compile each supported query to one SQL SELECT statement, followed by at most one round-trip that resolves term ids to terms. It MUST NOT issue one statement per solution or per triple pattern.

#### Scenario: Round-trip count
- **WHEN** a BGP with five triple patterns and a filter is evaluated on a backend that counts requests
- **THEN** at most two requests are made

### Requirement: Filter semantics
The system SHALL evaluate FILTER expressions with SPARQL semantics. Expression errors MUST make the filter reject the solution, and error propagation through `&&`, `||` and `!` MUST follow SPARQL's rules. Comparisons MUST use values for numeric, string, boolean and date-time literals.

#### Scenario: Numeric comparison across datatypes
- **WHEN** the store holds `1`, `"1.5"^^xsd:decimal` and `"2.0E0"^^xsd:double` and a query filters `?x > 1`
- **THEN** the decimal and the double are returned

#### Scenario: Type error rejects
- **WHEN** a query filters `?x > 5` and `?x` is bound to a string
- **THEN** that solution is rejected

#### Scenario: Error or true
- **WHEN** a filter is `(?x > 5) || true` and `?x` is a string
- **THEN** the solution is kept

### Requirement: Graphs and datasets
The system SHALL evaluate patterns against the default graph unless told otherwise. It SHALL support `GRAPH <iri>`, `GRAPH ?g`, `FROM`, `FROM NAMED`, and a query option that treats the default graph as the union of all graphs. Triples present in several graphs of a merged default graph MUST be counted once.

#### Scenario: Union default graph deduplicates
- **WHEN** the same triple is in two named graphs and a query runs with the union-default-graph option
- **THEN** the triple matches once

#### Scenario: GRAPH variable excludes default graph
- **WHEN** `GRAPH ?g { ?s ?p ?o }` runs
- **THEN** only quads in named graphs are returned, with `?g` bound to their graph name

### Requirement: Solution modifiers
The system SHALL support projection, DISTINCT, REDUCED, ORDER BY (SPARQL ordering of unbound values, blank nodes, IRIs and literals), LIMIT and OFFSET.

#### Scenario: Order by numeric value
- **WHEN** `ORDER BY ?x` runs over the integers 10, 9 and 100
- **THEN** the solutions come back as 9, 10, 100

### Requirement: Unsupported features are explicit
The system SHALL report an "unsupported" condition for features it can't compile. On synchronous backends it SHALL evaluate those queries with the Rust fallback evaluator. On other backends it MUST return an error naming the feature.

#### Scenario: Fallback on a native backend
- **WHEN** a query uses a feature the compiler doesn't support and runs on a native backend
- **THEN** correct results are returned through the fallback evaluator
