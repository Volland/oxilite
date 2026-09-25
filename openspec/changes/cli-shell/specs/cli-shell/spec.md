## Purpose

The interactive shell of the `oxilite` binary: SPARQL over a store opened by name or held in
memory, in the style of `sqlite3`.

## ADDED Requirements

### Requirement: The shell is the default command
Running `oxilite` without a subcommand SHALL open the shell. Without a database argument the
store SHALL be a transient in-memory store with the full schema; with a file argument (`oxilite
FILE` or `-l FILE`) the store SHALL be that SQLite file, created with the schema when missing.
The store flags of the other commands SHALL apply.

#### Scenario: New file gets the schema
- **WHEN** `oxilite new.sqlite` runs an `INSERT DATA` and exits
- **THEN** `new.sqlite` exists with the oxilite tables and a later `oxilite query -l new.sqlite`
  sees the inserted triple

#### Scenario: In-memory by default
- **WHEN** `oxilite` runs `INSERT DATA { <a:s> <a:p> <a:o> }` then a `SELECT` of all triples
- **THEN** the query returns the triple, and no file is created

#### Scenario: Subcommands are unchanged
- **WHEN** `oxilite query -q 'ASK {}'` runs
- **THEN** it prints the JSON result and exits without opening the shell

### Requirement: Multi-line statements
The shell SHALL accumulate lines until the statement is complete: brackets balanced outside
strings, IRIs and comments, and either the text ends with `;`, or an empty line follows, or the
statement is a single line that parses as a query or update. It SHALL run queries and updates
alike.

#### Scenario: One line runs at once
- **WHEN** `SELECT * { ?s ?p ?o }` is entered on one line
- **THEN** the query runs

#### Scenario: Several lines wait for the terminator
- **WHEN** the lines `SELECT * WHERE {`, `?s ?p ?o ;`, `  ?q ?r }`, `LIMIT 5;` are entered
- **THEN** nothing runs until the fourth line, then the query runs once

#### Scenario: Broken statement reports an error
- **WHEN** a balanced statement that does not parse is followed by an empty line
- **THEN** the parse error is printed and the shell accepts a new statement

### Requirement: Exit commands
`.exit` and `.quit` SHALL end the shell immediately, `.exit N` with status `N`; end of input
(Ctrl-D) SHALL end it too, and Ctrl-C SHALL discard the pending statement without exiting.

#### Scenario: Nothing runs after .exit
- **WHEN** a script contains `.exit` followed by an `INSERT DATA`
- **THEN** the insert does not run and the exit status is 0

### Requirement: Session prefixes
The shell SHALL predeclare well-known prefixes, remember `PREFIX` declarations of successful
statements and the prefixes of loaded Turtle-family files, add to each statement the
declarations it uses but lacks, and show IRIs in results as prefixed names where it can.

#### Scenario: Prefix remembered
- **WHEN** `PREFIX ex: <http://example.com/>` is declared in one statement
- **THEN** a later statement using `ex:a` without declaring `ex:` runs, and results show `ex:a`

### Requirement: Completion
Tab SHALL complete dot-commands, their fixed arguments and file names, SPARQL keywords,
prefixes, variables, and the store's predicates and classes in the triple position they fit,
using the text of the whole pending statement.

#### Scenario: Store predicates complete
- **WHEN** the store holds `ex:alice ex:knows ex:bob` and the input is `SELECT * { ?s ex:kn`
- **THEN** completion offers `ex:knows`

#### Scenario: Dot-command completes
- **WHEN** the input is `.ex`
- **THEN** completion offers `.exit` and `.explain`

### Requirement: Result display
Results SHALL print in the current `.mode`: `table` by default (fitted to the terminal width,
coloured only on a terminal without `NO_COLOR`, at most `.maxrows` rows followed by the count of
the rest), or `json`, `xml`, `csv`, `tsv`, or an RDF format for graph results. Each result
SHALL be followed by its row count and, with `.timer on` (default), the elapsed time.

#### Scenario: Wide table fits
- **WHEN** a result has a literal longer than the terminal
- **THEN** no printed line is wider than the terminal and the cell ends with `…`

### Requirement: Store commands
The shell SHALL provide `.open`, `.save`, `.load`, `.read`, `.dump`, `.explain`, `.datalog`,
`.graphs`, `.stats`, `.optimize` and `.help`, working on every backend.

#### Scenario: Save an in-memory session
- **WHEN** data is inserted into the in-memory store and `.save out.sqlite` runs
- **THEN** `out.sqlite` holds the same quads

### Requirement: Script mode
When standard input is not a terminal the shell SHALL run it without banner, prompt, colours or
history, report errors on standard error, and exit with status 1 if any statement failed.

#### Scenario: Failing script
- **WHEN** a script holds a valid query and an invalid one
- **THEN** the valid query's result is printed and the exit status is 1
