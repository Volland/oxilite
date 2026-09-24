## Purpose

Extends explain to rule programs, where the interesting choice is not only join order but which
recursion strategy each component got, and why.

## ADDED Requirements

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
