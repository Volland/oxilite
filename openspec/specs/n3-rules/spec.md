# n3-rules Specification

## Purpose
Notation3 rules over the store, run by the eyeron reasoner: read-only reasoning, and
materialization of conclusions as inferences of a producer, on every backend.

## Requirements

### Requirement: Optional N3 engine
N3 reasoning SHALL be provided by the `oxilite-n3` crate, exposed by `oxilite` only under a
non-default `n3` feature, and SHALL use the published `eyeron` crate without its default features.

#### Scenario: Default build is unchanged
- **WHEN** `oxilite` is built with default features
- **THEN** it has no `n3` module and no dependency on `eyeron`

### Requirement: Read-only reasoning
`Store::n3_reason` SHALL return the conclusions of an N3 rule set over the store, without the input
facts and without writing anything.

#### Scenario: Conclusions without writes
- **WHEN** a grandparent rule is run with `n3_reason` over two parent chains
- **THEN** the two grandparent triples are returned and the store, inferences included, is unchanged

### Requirement: Scoped and pruned input
A run SHALL read every graph but the system graphs, merged, or only the graphs the options name;
inside that scope it SHALL read only the predicates the rule bodies use, unless a body has a
variable predicate or pruning is turned off.

#### Scenario: Only body predicates are read
- **WHEN** a rule set reading `ex:parent` runs over a store that also holds names and ages
- **THEN** only the `ex:parent` statements are loaded

#### Scenario: Graph scope
- **WHEN** the options name one graph
- **THEN** statements of other graphs neither match nor combine

### Requirement: Materialized conclusions
`Store::n3_materialize` SHALL store the storable conclusions in `quads_inf` attributed to the
options' producer (`n3` by default), replacing that producer's earlier inferences in one atomic
request and leaving other producers' inferences in place.

#### Scenario: Inferences for queries
- **WHEN** conclusions are materialized
- **THEN** a SPARQL query with `include_inferred` matches them, one without does not, and `inference_producers` names `n3`

#### Scenario: A rerun reflects the store
- **WHEN** a premise is deleted and the rules are materialized again
- **THEN** the conclusion that depended on it is gone

### Requirement: Own conclusions are never input
With `include_inferred`, a run SHALL read the inferences of other producers but not those only its
own producer derived.

#### Scenario: A stale conclusion does not feed itself
- **WHEN** a conclusion's premise is deleted and a rule copying that conclusion is materialized with `include_inferred`
- **THEN** nothing is inferred

### Requirement: Bounded and reproducible runs
eyeron's limits SHALL be settable per run; a run a limit cut short SHALL be reported as incomplete,
and SHALL fail when `require_complete` holds, which materialization always requires. `Options::now`
SHALL pin the instant `time:localTime` reports. Proofs SHALL be returned only on request.

#### Scenario: Incomplete runs are not stored
- **WHEN** a recursive rule set runs with a fixpoint limit of one step
- **THEN** `n3_reason` reports it incomplete and `n3_materialize` fails without writing

#### Scenario: Pinned clock
- **WHEN** a rule reading `time:localTime` runs twice with the same `now`
- **THEN** both runs conclude the same timestamp

### Requirement: Inference fuses
A fired inference fuse (`{ … } => false`) SHALL be reported by `n3_reason` and SHALL abort
`n3_materialize` before anything is written.

#### Scenario: Fuse aborts materialization
- **WHEN** the data matches a fuse's premise
- **THEN** `n3_materialize` fails with a fuse error and no inference is stored

### Requirement: Unstorable conclusions
A conclusion the store cannot hold (a literal, list or formula subject, a non-IRI predicate, a list
or formula object) SHALL be returned as N3 text by `n3_reason` and skipped and counted by
`n3_materialize`.

#### Scenario: Literal subjects are reported
- **WHEN** a rule concludes a triple whose subject is a literal
- **THEN** it is listed as unstorable and never stored

### Requirement: Every backend
Reasoning and materialization SHALL run as sans-IO jobs, on `Store` and `AsyncStore` alike.

#### Scenario: Async store
- **WHEN** the rules run on an `AsyncStore`
- **THEN** they conclude and store what the blocking store does

### Requirement: Command line
`oxilite n3` SHALL print the conclusions of a rule set as N-Triples, or with `--materialize` store
them, with options for the producer, graphs, inferences, pruning, proof, clock and fixpoint limit.

#### Scenario: Materialize from the command line
- **WHEN** `oxilite n3 --file rules.n3 --materialize` runs on a database file
- **THEN** it prints how many triples were inferred and `oxilite query --inferred` sees them
