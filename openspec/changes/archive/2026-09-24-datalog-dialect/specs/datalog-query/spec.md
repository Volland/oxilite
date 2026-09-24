## Purpose

Adds a Datalog dialect over the oxilite store: rule programs with stratified negation, constraints
and aggregation, compiled to SQL, with recursion that property paths cannot express.

## ADDED Requirements

### Requirement: Rule programs
The system SHALL accept a program of rules over RDF terms. A rule is `head :- body.` where the body
is a comma-separated conjunction of atoms and constraints. An atom is either an IRI predicate
applied to two arguments (one triple pattern), a derived predicate, or the built-in `triple/3` and
`triple/4` forms. Arguments are variables (`?x`), `_`, IRIs, CURIEs declared by `@prefix`, or RDF
literals. A program MAY end with a goal `?- atom, ... .` naming the relation to return.

#### Scenario: Derived relation from a triple pattern
- **WHEN** `parent(?x, ?y) :- ex:parent(?x, ?y). ?- parent(?x, ?y).` runs over two `ex:parent` triples
- **THEN** it returns both pairs

#### Scenario: Join of two atoms
- **WHEN** `gp(?x, ?z) :- ex:parent(?x, ?y), ex:parent(?y, ?z). ?- gp(?x, ?z).` runs
- **THEN** it returns exactly the grandparent pairs

#### Scenario: A unary atom is a class
- **WHEN** `adult(?x) :- ex:Person(?x).` runs
- **THEN** it returns exactly the subjects of `rdf:type ex:Person`

#### Scenario: Quantifying over the predicate
- **WHEN** a rule body uses `triple(?s, ?p, ?o)` with `?p` unbound
- **THEN** it matches every asserted triple, and `triple(?s, ?p, ?o, ?g)` also binds the graph

### Requirement: Single statement for non-recursive programs
The system SHALL compile a program without recursion into one SQL statement, using the statistics
planner's join order, whenever the equivalent SPARQL query would compile to one.

#### Scenario: Request count
- **WHEN** a three-atom non-recursive program with a goal runs
- **THEN** it uses at most two backend requests: the query and its term resolution

### Requirement: Constraints
The system SHALL accept constraints in a rule body, evaluated with SPARQL expression semantics and
pushed into the compiled statement rather than applied to its rows: comparison (`=`, `!=`, `<`,
`<=`, `>`, `>=`), arithmetic (`+`, `-`, `*`, `/`), boolean connectives, and the SPARQL function
library including string, regex, numeric, datatype and temporal functions.

#### Scenario: Numeric comparison
- **WHEN** `adult(?p) :- ex:age(?p, ?a), ?a >= 18.` runs over people aged 17 and 18
- **THEN** only the 18-year-old is returned

#### Scenario: Arithmetic and a typed literal
- **WHEN** a body contains `?a * 2 < 100` and `?d > "2024-01-01"^^xsd:date`
- **THEN** both are evaluated with SPARQL semantics for numeric promotion and date ordering

#### Scenario: Function call
- **WHEN** a body contains `REGEX(?n, "^A", "i")`
- **THEN** it filters on the regular expression, using the backend's regex support or the UDF

### Requirement: Stratified negation
The system SHALL support negated atoms (`not p(...)`), and SHALL evaluate a negated atom only
against a relation that is already complete. A program whose dependency graph has a negative edge
inside a strongly connected component MUST be rejected with an error naming that component.

#### Scenario: Negation over a derived relation
- **WHEN** `orphan(?x) :- ex:Person(?x), not ancestor(?x, _).` runs and `ancestor` is derived
- **THEN** exactly the people with no ancestor are returned

#### Scenario: Unstratified negation is rejected
- **WHEN** `p(?x) :- q(?x), not p(?x).` is submitted
- **THEN** compilation fails with an error identifying the cycle through `p`, and no SQL is run

#### Scenario: Negation compiles to NOT EXISTS
- **WHEN** a stratified negated atom is compiled
- **THEN** the generated SQL contains a `NOT EXISTS` subquery and no extra round trip

### Requirement: Safety
The system SHALL reject a rule unless every variable in its head, in a negated atom, and in a
constraint also occurs in a positive body atom of the same rule.

#### Scenario: Unbound head variable
- **WHEN** `p(?x, ?y) :- ex:a(?x).` is submitted
- **THEN** compilation fails naming `?y` as unbound

#### Scenario: Unbound negated variable
- **WHEN** `p(?x) :- ex:a(?x), not ex:b(?y).` is submitted
- **THEN** compilation fails naming `?y` as unbound

### Requirement: Linear recursion in one statement
The system SHALL compile a recursive component whose rules are linear into a single
`WITH RECURSIVE` common table expression, inlined as a relation subquery so it composes with the
rest of the program, and SHALL use a deduplicating `UNION` in the recursive term so that cyclic
data terminates.

#### Scenario: Transitive closure agrees with a property path
- **WHEN** `anc(?x,?y) :- ex:parent(?x,?y). anc(?x,?z) :- ex:parent(?x,?y), anc(?y,?z). ?- anc(?x,?y).` runs
- **THEN** its solutions equal those of the SPARQL query `SELECT ?x ?y WHERE { ?x ex:parent+ ?y }`

#### Scenario: Cycle terminates
- **WHEN** the same program runs over data containing a parent cycle
- **THEN** it terminates and returns each reachable pair once

#### Scenario: Recursion with a constraint
- **WHEN** a recursive rule body carries a constraint that a property path cannot express, such as `?d < 100`
- **THEN** the constraint is applied inside the recursion and the result is restricted accordingly

#### Scenario: One round trip
- **WHEN** a linear recursive program runs
- **THEN** it uses at most two backend requests, on native backends and on D1

### Requirement: Mutual recursion
The system SHALL evaluate a strongly connected component containing several predicates. When the
backend reports `compound_recursive_cte`, it SHALL compile the component into one common table
expression with a discriminant column and one recursive term per rule; otherwise it SHALL use the
fixpoint strategy.

#### Scenario: Even and odd
- **WHEN** `even(?x) :- ex:zero(?x). even(?x) :- ex:succ(?y,?x), odd(?y). odd(?x) :- ex:succ(?y,?x), even(?y).` runs over a successor chain
- **THEN** `even` and `odd` each return the correct members

#### Scenario: Backend without compound recursive CTEs
- **WHEN** the same program runs on a backend reporting `compound_recursive_cte` false
- **THEN** it still returns the correct result, and `explain()` reports the fixpoint strategy

### Requirement: Non-linear recursion
SQLite's recursive CTE allows exactly one self-reference per recursive term, so a rule with two
recursive body atoms has no single-statement form. The system SHALL evaluate such a component by
iterating it to a fixpoint in a work table: the component's non-recursive rules seed it, its
recursive rules are applied repeatedly, and the evaluation stops when no new fact is derived. Each
round is one request, so the system MUST bound the rounds by a configurable maximum and MUST
report the rounds taken. Rows written while iterating MUST be scoped to the evaluation and removed
when it ends, so concurrent programs do not see each other's work and nothing is left behind.

#### Scenario: Non-linear closure agrees with the linear formulation
- **WHEN** `path(?x,?z) :- path(?x,?y), path(?y,?z).` with a base rule runs
- **THEN** its solutions equal those of the linear formulation of the same closure

#### Scenario: Rounds are reported
- **WHEN** a non-linear program runs
- **THEN** the result reports one round count per iterated component

#### Scenario: Iteration bound
- **WHEN** a component has not converged after the configured maximum number of rounds
- **THEN** evaluation fails naming the component and the bound, rather than running unbounded

#### Scenario: Work rows do not outlive the evaluation
- **WHEN** the same non-linear program runs twice
- **THEN** the second run returns exactly what the first returned

#### Scenario: A component too wide to iterate is refused
- **WHEN** an iterated component's relation is wider than the work table
- **THEN** compilation fails saying so, before anything is written

### Requirement: Aggregation
The system SHALL support aggregates in a rule head — `COUNT`, `COUNT DISTINCT`, `SUM`, `MIN`,
`MAX` and `SAMPLE` — grouping by the head's non-aggregated arguments. An aggregating edge inside a
strongly connected component MUST be rejected.

A derived relation holds term ids, and a term id can be computed in SQL only for a value the
encoding stores inline, so an aggregate MUST produce such a value. `COUNT` and `SUM` yield
`xsd:integer`; `MIN`, `MAX` and `SAMPLE` return one of the ids they ranged over, under the store's
total order, which is exact value order for inline integers. An aggregate with no inline form —
`AVG` and `GROUP_CONCAT` — MUST be rejected with an error saying why.

#### Scenario: Count grouped by key
- **WHEN** `n(?x, COUNT(?y)) :- ex:knows(?x, ?y). ?- n(?x, ?c).` runs
- **THEN** each subject is returned with the number of people it knows

#### Scenario: Aggregate over a derived relation
- **WHEN** an aggregate is taken over a recursively derived relation in a later stratum
- **THEN** it is computed after that relation is complete

#### Scenario: Recursive aggregation is rejected
- **WHEN** an aggregating rule participates in its own recursive component
- **THEN** compilation fails naming the component

#### Scenario: An aggregate with no inline form is rejected
- **WHEN** a rule head uses `AVG` or `GROUP_CONCAT`
- **THEN** compilation fails explaining that the result has no inline term id

### Requirement: Explain for Datalog
The system SHALL provide `explain()` for a Datalog program, reporting the strata and their order,
the strategy chosen for each recursive component — single recursive member, tagged member for
mutual recursion, or iteration — and the generated SQL.

#### Scenario: Strategy is reported
- **WHEN** `explain()` is called on a program with a linear and a mutually recursive component
- **THEN** it names the single-member strategy for the first and the tagged-member strategy for
  the second

#### Scenario: Iteration states its cost
- **WHEN** `explain()` is called on a program with a non-linear component
- **THEN** it names the iteration strategy and says it costs one request per round

### Requirement: Datalog results
The system SHALL return solutions as RDF terms, resolved through the same term resolver the other
dialects use, and SHALL report the goal relation's argument names as the result's variables.

#### Scenario: Terms round-trip
- **WHEN** a goal returns IRIs and typed literals
- **THEN** each value is the RDF term that was stored, with its datatype and language tag intact
