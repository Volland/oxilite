# host-functions Specification

## Purpose
User functions written in the host language, registered on a store and callable from SPARQL,
Cypher and Datalog with the same meaning, without losing SQL compilation for the parts of a query
that do not call them.

## Requirements

### Requirement: Registering host functions
The store SHALL let the application register a function from RDF terms to an optional RDF term
under an IRI, with an optional Cypher name, an optional arity range and a description, list the
registered functions, and unregister one. Returning no term SHALL be an evaluation error with the
language's usual consequence (an unbound value in SPARQL, `null` in Cypher, no row in Datalog).
Registration SHALL apply to the store handle and its clones, and SHALL NOT be persisted.

#### Scenario: Listing
- **WHEN** two functions are registered and the functions are listed
- **THEN** both appear with their IRIs, Cypher names, arities and descriptions

#### Scenario: Clash with a built-in
- **WHEN** a function is registered under an IRI the engine already interprets (`oxl:textMatch`, an
  `xsd:` cast) or a Cypher name that is a built-in function
- **THEN** registration fails naming the clash

### Requirement: Host functions in SPARQL
A SPARQL function call whose IRI is a registered host function SHALL call it, in queries (`FILTER`,
`BIND`, `SELECT` expressions, `ORDER BY`, `GROUP BY`, aggregates' arguments) and in the `WHERE` of
updates. The parts of the query that do not depend on the call SHALL still run as SQL.

#### Scenario: BIND and FILTER
- **WHEN** `SELECT ?s ?slug WHERE { ?s ex:name ?n BIND(fn:slugify(?n) AS ?slug) FILTER(fn:isLong(?n)) }`
  runs
- **THEN** it returns the slugs of the long names

#### Scenario: Update with a host function
- **WHEN** `INSERT { ?s ex:slug ?slug } WHERE { ?s ex:name ?n BIND(fn:slugify(?n) AS ?slug) }` runs
- **THEN** every named subject gets its slug

#### Scenario: Unregistered function
- **WHEN** a query calls an IRI that is neither built in nor registered
- **THEN** the query fails with an error naming the function, as Oxigraph does

#### Scenario: Wrong number of arguments
- **WHEN** a registered function with arity 1 is called with two arguments
- **THEN** the call is an evaluation error for each solution, so a `BIND` leaves the variable
  unbound

### Requirement: Host functions in Cypher
A Cypher function call whose name is the Cypher name of a registered function (case-insensitive)
SHALL call it wherever an expression is allowed, whether the clause compiles to SQL or runs in Rust,
converting Cypher values to RDF terms and back.

#### Scenario: In WHERE and RETURN
- **WHEN** `MATCH (p:Person) WHERE ex.isLong(p.name) RETURN ex.slugify(p.name) AS slug` runs
- **THEN** it returns the slugs of the people with long names

#### Scenario: In a clause evaluated in Rust
- **WHEN** `MATCH (p:Person) RETURN collect(ex.slugify(p.name)) AS slugs` runs
- **THEN** it returns the list of slugs

#### Scenario: Unknown function
- **WHEN** a statement calls a function that is neither built in nor registered
- **THEN** it fails naming the function

### Requirement: Host functions in Datalog
A Datalog rule or goal SHALL be able to call a registered function as an expression in a
constraint (`?s = fn:slugify(?n)`, which binds `?s` when it is otherwise unbound, and
`fn:score(?x) > 0.5`), or as an atom whose predicate is the function IRI (`fn:slugify(?n, ?s)`, the
last argument being the result; `fn:isLong(?n)` a filter). A rule that calls one SHALL be evaluated
before the rules that depend on it, and its results SHALL be visible to later rules, negation and
aggregation. A rule that calls a host function and depends on its own head SHALL be rejected.

#### Scenario: Binding a value
- **WHEN** `slug(?p, ?s) :- ex:name(?p, ?n), ?s = fn:slugify(?n). ?- slug(?p, ?s).` runs
- **THEN** it returns every person with their slug

#### Scenario: Results feed later rules
- **WHEN** a rule joins `slug(?p, ?s)` with another relation, and another rule negates it
- **THEN** both see exactly the slugs the host rule derived

#### Scenario: Atom form
- **WHEN** `long(?p) :- ex:name(?p, ?n), fn:isLong(?n).` runs
- **THEN** it returns the people with long names

#### Scenario: Recursive host rule is rejected
- **WHEN** `r(?x, ?y) :- r(?x, ?z), ?y = fn:next(?z).` is submitted
- **THEN** it fails with an error saying a rule calling a host function cannot be recursive

### Requirement: Cypher names default from the IRI
A function registered without a Cypher name SHALL be callable from Cypher by the local name of its
IRI (after the last `#` or `/`).

#### Scenario: Default name
- **WHEN** `http://example.com/fn#slugify` is registered without a Cypher name
- **THEN** `RETURN slugify('A B')` calls it
