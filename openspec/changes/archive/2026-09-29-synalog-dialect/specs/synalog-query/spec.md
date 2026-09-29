## Purpose

Adds Synalog, a Datalog-family language for AI agents, as an optional second rule dialect: usable
on its own for any of its SQL engines, and runnable over the oxilite triple store on every backend.

## ADDED Requirements

### Requirement: Optional dialect
The Synalog dialect SHALL be provided by the `oxilite-synalog` crate and exposed by `oxilite` only
under a non-default `synalog` feature, so the default build carries no Synalog code.

#### Scenario: Default build is unchanged
- **WHEN** `oxilite` is built with default features
- **THEN** it has no `synalog` module and no dependency on the `synalog` crate

### Requirement: Checking and compiling without a store
The crate SHALL expose `check`, returning every verifier error of a program, and
`compile_for_engine`, returning the SQL Synalog produces for a named predicate and any engine it
supports, without opening a store.

#### Scenario: Compile for another engine
- **WHEN** a program is compiled for `duckdb` with `compile_for_engine`
- **THEN** the SQL Synalog produces for that engine is returned and no store is needed

#### Scenario: Verifier errors are reported
- **WHEN** a recursive predicate lacks a `@Recursive` bound
- **THEN** checking or running it fails with a verification error that names the predicate

### Requirement: The store as relational tables
A program run on the store SHALL be able to read a table `triples` with columns `subject`,
`predicate`, `object`, `kind`, `datatype`, `lang` and `graph`, one row per visible quad. Terms
SHALL decode to native SQL values: integers and other numeric literals as numbers, booleans as 1
and 0, blank nodes as `_:label`, IRIs and other literals as their lexical form; `kind` is `iri`,
`blank`, `literal` or `triple`, and `graph` is NULL for the default graph.

#### Scenario: Rule over triples
- **WHEN** a rule selects `subject` and `object` from `triples` with a fixed `predicate`
- **THEN** it returns the matching IRIs and literal values

#### Scenario: Numbers compare as numbers
- **WHEN** a rule filters `object > 18` over integer ages
- **THEN** only the subjects whose age is above 18 are returned

#### Scenario: Literal details are kept
- **WHEN** a language-tagged string and a blank node are read from `triples`
- **THEN** the string's row has `kind` `literal` and its `lang`, and the blank node reads as `_:label` with `kind` `blank`

### Requirement: Declared predicate and class tables
A comment line `# @table NAME <IRI>` SHALL declare a table `NAME(subject, object, kind, datatype,
lang, graph)` of the triples with that predicate, and `# @class NAME <IRI>` a table
`NAME(subject, graph)` of the instances of that class. The same declarations SHALL be possible
through the options. A declared table MUST select by term id, not by decoded string.

#### Scenario: Predicate table
- **WHEN** a program declares `# @table parent <http://example.org/parent>` and reads `parent`
- **THEN** it returns the parent pairs, and the SQL compares the predicate column with its term id

#### Scenario: Class table
- **WHEN** a program declares `# @class person <http://example.org/Person>` and reads `person`
- **THEN** it returns exactly the instances of that class

### Requirement: Same scope options as SPARQL and Datalog
Runs on the store SHALL read the default graph only unless `union_default_graph` is set, SHALL add
materialized inferences under `include_inferred`, and SHALL read a past version under `as_of`
exactly as the Datalog dialect does; `as_of` with `include_inferred` is rejected.

#### Scenario: Named graphs need the union option
- **WHEN** data exists only in a named graph
- **THEN** a program sees it only with `union_default_graph`, and its `graph` column is the graph IRI

#### Scenario: Inferences are opt-in
- **WHEN** a Datalog rule has been materialized
- **THEN** a Synalog program sees its conclusions only with `include_inferred`

#### Scenario: Time travel
- **WHEN** a program runs with `as_of` `HEAD~1` on a versioned store
- **THEN** it sees the data as it was before the last commit

### Requirement: Recursion and negation on the store
Synalog recursion (`@Recursive`) and negation (`~`) SHALL run on the store and on D1, with no
function the backend has to register.

#### Scenario: Recursion agrees with a property path
- **WHEN** a `distinct` recursive ancestor predicate runs with a bound larger than the chain
- **THEN** it returns exactly the pairs the SPARQL property path `ex:parent+` returns

#### Scenario: Negation needs no runtime function
- **WHEN** a rule negates a table
- **THEN** it returns the right rows and its SQL calls no `MagicalEntangle`

### Requirement: Constructs the store cannot run are rejected up front
A program whose SQL would call a Logica runtime function with no SQLite equivalent, or would run
more than one statement, SHALL fail before any request is sent with an `Unsupported` error naming
the construct.

#### Scenario: ArgMax is rejected
- **WHEN** a program aggregates with `ArgMax=`
- **THEN** it fails with an unsupported error naming `ArgMax`

#### Scenario: Ground is rejected
- **WHEN** a program uses `@Ground`
- **THEN** it fails with an unsupported error saying it would create tables

### Requirement: Results and pagination
A run SHALL return the predicate's head columns in order and one row of SQL values per result row,
in one request on every backend. The options SHALL accept `limit` and `offset`, combined with the
program's `@Limit` as Synalog combines them.

#### Scenario: Pagination
- **WHEN** a predicate ordered by `@OrderBy` runs with `limit` 1 and `offset` 1
- **THEN** exactly its second row is returned

### Requirement: Bindings
`@oxilite/node`, `@oxilite/d1` and the Python package SHALL expose `synalog(program, predicate,
options)` returning `columns`, `rows` and `records` of plain values, and `synalogSql`. In the
WebAssembly core Synalog SHALL be a non-default `synalog` feature, and the D1 driver MUST fail with
an error naming that feature when the core lacks it.

#### Scenario: Node and Python
- **WHEN** a recursive program over a declared table runs through the Node or Python binding
- **THEN** it returns the rows, with integer literals as numbers

#### Scenario: D1
- **WHEN** the WebAssembly core is built with `synalog` and a program with recursion and negation runs against a D1 binding
- **THEN** it returns the same rows as the native store

### Requirement: Command line
`oxilite synalog PREDICATE` SHALL run a program read from `--program`, `--file` or standard input
against a store and print the columns and rows, `--sql` SHALL print the store SQL instead, and
`--engine NAME` SHALL print the SQL for that engine without opening a store.

#### Scenario: Run from the command line
- **WHEN** `oxilite synalog Parent --program …` runs against a store holding parent triples
- **THEN** it prints the head columns, then one tab-separated line per row
