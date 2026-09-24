## Purpose

Makes the Datalog dialect reachable from JavaScript, with the same term shapes the other
dialects already return.

## ADDED Requirements

### Requirement: Datalog from JavaScript
The package SHALL expose `datalog(program, options)`, `datalogMaterialize(program, options)` and
`explainDatalog(program)`. Solutions MUST come back as RDF/JS terms in `rows`, keyed by the goal's
variables in `records`, with `null` for an unbound column, and MUST report the rounds any iterated
component took.

#### Scenario: Solutions are RDF/JS terms
- **WHEN** `datalog` runs a program whose goal returns IRIs
- **THEN** each value is an RDF/JS `NamedNode`, and `records[0]` is keyed by the goal's variables

#### Scenario: Recursion from JavaScript
- **WHEN** a transitive-closure program runs with a constant in the goal
- **THEN** it returns every reachable node, and `rounds` is empty because nothing was iterated

#### Scenario: Materializing from JavaScript
- **WHEN** `datalogMaterialize` runs and a SPARQL query then runs with `include_inferred`
- **THEN** the derived triples are returned, and a query without that option returns none

#### Scenario: A rejected program throws
- **WHEN** an unstratified program is submitted
- **THEN** the call throws, and the message says the program is not stratified

### Requirement: Datalog option names match the other dialect
The package's Datalog options SHALL use the same spelling as the Cypher options for the settings
they share, so the two dialects read the same from JavaScript.

#### Scenario: Graph scope
- **WHEN** `useDefaultGraphAsUnion` is set on a Datalog call
- **THEN** it has the effect it has on a Cypher call
