# query-explain Specification

## Purpose
Lets users see how a query becomes SQL, which join order was chosen, and why part of it might not run inside SQLite. For a Datalog rule program it also reports the strata and the strategy each recursive component got.

## Requirements

### Requirement: Explain output
The system SHALL return, for any query:
- the SQL statements it would execute
- the join order chosen for each basic graph pattern, with estimated cardinalities
- whether statistics were available
- which operators, if any, are evaluated outside SQL

#### Scenario: Fully compiled query
- **WHEN** `explain()` is called for a BGP query
- **THEN** the output contains one SQL SELECT and reports no fallback

#### Scenario: Fallback reason
- **WHEN** `explain()` is called for a query that uses a function unavailable on the backend
- **THEN** the output names the function and the evaluation strategy used

### Requirement: Performance warnings
The system SHALL warn, in the explain output:
- when a recursive path has no constant or bound endpoint to start from
- when the join graph forces a Cartesian product
- when a Rust-side filter would read an unbounded number of rows from a remote backend

#### Scenario: Unseeded closure warning
- **WHEN** `explain()` is called for `?a ex:p+ ?b` with both ends unbound
- **THEN** the output warns that the whole transitive closure is computed

### Requirement: Explain for rule programs
The system SHALL return, for a Datalog program:
- the strata and the order they are evaluated in
- for each recursive component, the strategy chosen — single recursive CTE, tagged CTE for mutual
  recursion, or iteration in the work table
- the SQL statements it would execute, and the join order with estimated cardinalities

#### Scenario: Strategy per component
- **WHEN** `explain()` is called on a program with a linear and a non-linear component
- **THEN** the output names the single-CTE strategy for one and the iteration strategy for the
  other

#### Scenario: Iteration round trips are called out
- **WHEN** `explain()` is called on a program that has to iterate
- **THEN** the output says that evaluation costs one request per round
