## Purpose

λProlog (ELPI) programs over the store, run by lean-elpi: goal queries, constraint checks and
materialization of conclusions as inferences of a producer.

## ADDED Requirements

### Requirement: Optional ELPI engine
ELPI programs SHALL be provided by the `oxilite-elpi` crate, exposed by `oxilite` only under a
non-default `elpi` feature. The native engine SHALL be linked only by a further opt-in feature;
without it every run SHALL fail with an `Unsupported` error.

#### Scenario: Build without the library
- **WHEN** `oxilite` is built with `elpi` but without `elpi-native`
- **THEN** it builds without `libleanelpi` and a query fails with `Unsupported`

### Requirement: Store access through host builtins
A program SHALL read the store through `ox.triple S P O` (graphs merged, each triple once) and
`ox.quad S P O G`; each call SHALL be one pattern scan whose pattern is the call's ground
arguments, over every graph but the system graphs or the graphs the options name, optionally with
other producers' inferences.

#### Scenario: Bound arguments select
- **WHEN** `ox.triple` is called with a ground subject and predicate
- **THEN** one scan with that subject and predicate answers it, and a triple stored in two graphs answers once

### Requirement: RDF terms as typed values
RDF terms SHALL cross as `(iri S)`, `(str S)`, `(num N)` for `xsd:integer` within 64 bits,
`(dbl F)` for `xsd:double`, `(lang S T)`, `(lit S Datatype)` and `(bnode S)`, and the default graph
as `default-graph`; bindings that are such terms SHALL come back as RDF terms.

#### Scenario: Arithmetic on stored numbers
- **WHEN** a rule compares `(num A)` read from the store with `A >= 18`
- **THEN** only the subjects whose integer value is at least 18 are answers

### Requirement: Goal queries
`Store::elpi_query` SHALL solve any goal against a program and return each solution's bindings,
including goals that use ELPI's hypothetical implication (`=>`), which never writes.

#### Scenario: Hypothetical goal
- **WHEN** a goal assumes an extra edge with `=>` before asking for reachability
- **THEN** it succeeds, the same question without the assumption fails, and the store is unchanged

### Requirement: Constraint checks
`Store::elpi_check` SHALL return every solution of `ox.violation Severity Name Message Focus`, with
severity `error` or `warning`, the name, the message and the focus terms.

#### Scenario: Missing name is an error
- **WHEN** a program reports persons without a name as `error` and minors as `warning`
- **THEN** both violations are returned with their focus nodes

### Requirement: Materialized conclusions
`Store::elpi_materialize` SHALL store the storable triples of `ox.conclude (triple S P O)` in
`quads_inf` attributed to the options' producer (`elpi` by default), replacing that producer's
earlier inferences in one atomic request, and SHALL count conclusions that are no storable triple.

#### Scenario: Conclusions as inferences
- **WHEN** a program concludes ancestors and one triple with a literal subject
- **THEN** the ancestors are queryable with `include_inferred`, attributed to `elpi`, and one conclusion is counted unstorable

### Requirement: Bounded runs
A run SHALL be bounded by fuel (resolution steps), a solution limit and a budget of rows the host
builtins may return; exceeding fuel or the row budget SHALL fail the run with its own error.

#### Scenario: Endless recursion runs out of fuel
- **WHEN** a looping goal runs with a fuel bound
- **THEN** it fails with an out-of-fuel error

### Requirement: Command line
`oxilite elpi` SHALL print a goal's solutions, list violations with `--check` (exit code 1 when one
is an `error`), or store conclusions with `--materialize`, on builds with the `elpi` feature.

#### Scenario: Check from the command line
- **WHEN** `oxilite elpi --file rules.elpi --check` finds an error
- **THEN** it prints each violation and exits with status 1
