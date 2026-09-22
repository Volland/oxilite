# sparql-full-query Specification

## Purpose
Extends query compilation to all SPARQL 1.1 query operators, so complete queries run as SQL inside SQLite on every backend, including D1.

## Requirements

### Requirement: Optional patterns
The system SHALL evaluate OPTIONAL with SPARQL left-join semantics, including filters that reference both sides and nested OPTIONALs. Values bound only in an unmatched optional part MUST be unbound.

#### Scenario: Optional with constant BIND
- **WHEN** `OPTIONAL { ?s ex:p ?o BIND(1 AS ?one) }` finds no match for a solution
- **THEN** both `?o` and `?one` are unbound in that solution

#### Scenario: Filter inside optional
- **WHEN** `OPTIONAL { ?s ex:age ?a FILTER(?a > 18) }` runs and a subject's age is 10
- **THEN** the subject is returned with `?a` unbound

### Requirement: Union, minus and exists
The system SHALL evaluate UNION, MINUS, FILTER EXISTS and FILTER NOT EXISTS with SPARQL semantics. MINUS MUST remove nothing when the domains of the two sides are disjoint.

#### Scenario: Disjoint minus
- **WHEN** `{ ?a ex:p ?b } MINUS { ?c ex:q ?d }` runs
- **THEN** all solutions of the left side are returned

#### Scenario: Exists sees outer bindings
- **WHEN** `FILTER EXISTS { ?s ex:q ?x FILTER(?x = ?y) }` runs and `?y` is bound outside
- **THEN** the inner filter uses the outer value of `?y`

### Requirement: Aggregates
The system SHALL evaluate GROUP BY with COUNT (including DISTINCT and `*`), SUM, AVG, MIN, MAX, SAMPLE and GROUP_CONCAT, applying SPARQL numeric type promotion. HAVING MUST be supported. A query that aggregates with no GROUP BY over no matching solutions MUST return exactly one solution.

#### Scenario: Count over empty input
- **WHEN** `SELECT (COUNT(*) AS ?n) WHERE { ?s ex:none ?o }` runs
- **THEN** one solution with `?n = 0` is returned

#### Scenario: Sum promotes types
- **WHEN** SUM is taken over `1` and `"2.5"^^xsd:decimal`
- **THEN** the result is `"3.5"^^xsd:decimal`

#### Scenario: Max returns the stored term
- **WHEN** MAX is taken over `xsd:dateTime` values
- **THEN** the returned literal is the latest stored dateTime with its original lexical form

### Requirement: Property paths
The system SHALL evaluate every SPARQL 1.1 property path form, in any graph scope:
- sequence
- alternative
- inverse
- negated property sets
- zero-or-one (`?`), zero-or-more (`*`) and one-or-more (`+`)

Recursive paths MUST terminate on cyclic data and MUST follow SPARQL's set semantics.

#### Scenario: Cycle terminates
- **WHEN** `ex:a ex:next+ ?x` runs over a three-node cycle
- **THEN** exactly three solutions are returned

#### Scenario: Zero-length path on a constant
- **WHEN** `ex:missing ex:p* ?x` runs and `ex:missing` isn't in the data
- **THEN** one solution with `?x = ex:missing` is returned

### Requirement: Subqueries and values
The system SHALL evaluate nested SELECT subqueries with their own modifiers, and inline VALUES blocks including UNDEF.

#### Scenario: Subquery limit
- **WHEN** a subquery has `ORDER BY ?x LIMIT 1` and is joined with an outer pattern
- **THEN** the outer join sees only the first subquery solution

### Requirement: Conformance
The system SHALL pass at least 95% of the W3C SPARQL 1.1 query evaluation tests on native backends, and every remaining failure MUST be listed with a reason in the compatibility allow-list.

#### Scenario: Suite gate
- **WHEN** the W3C SPARQL 1.1 query suite runs in the compatibility harness
- **THEN** the pass rate is at least 95% and there are no divergences that aren't allow-listed
