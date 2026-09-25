## Purpose

Keeps the full history of a store as an immutable log of commits, so any past state can be queried,
changes can be attributed, and the current state stays as fast to query as an unversioned store.

## ADDED Requirements

### Requirement: Versioning levels
The system SHALL offer versioning as a per-store option with four nested levels:
- `off`: the default, no history.
- `stamped`: insertion clock.
- `log`: immutable history with time travel.
- `history`: adds branches.

Each level SHALL provide everything the levels below it provide. The level SHALL be recorded in the store
and reported by the API.

#### Scenario: Versioned store records history
- **WHEN** a store is created at level `log` and quads are inserted
- **THEN** the insert is recorded as a commit that can be listed

#### Scenario: Level is reported
- **WHEN** a store created at level `stamped` is reopened
- **THEN** the API reports level `stamped`

### Requirement: Default store is unchanged
A store at level `off` SHALL have exactly the schema, generated SQL, query plans and rows written per quad
that it had before versioning existed. Every version-only construct SHALL fail on it with an error stating that
the store keeps no history. The version-only constructs are:
- the version option
- `SERVICE <oxilite:version/…>`
- `<oxilite:history>`
- the Datalog version syntax and history relations

#### Scenario: Same SQL as before
- **WHEN** the compatibility and BSBM queries are compiled for an `off` store
- **THEN** the SQL is identical to the SQL generated before this change

#### Scenario: Same write cost as before
- **WHEN** `write-cost` measures an `off` store on D1
- **THEN** rows written per quad equal the pre-change baseline

#### Scenario: History asked of an unversioned store
- **WHEN** a query with the version option `main~1` runs on an `off` store
- **THEN** it fails with an error stating that the store keeps no history

### Requirement: Insertion clock
A store at level `stamped` or higher SHALL keep a strictly monotonic store clock that advances once per atomic
write and maps each tick to the wall time of that write. Each quad SHALL carry the tick at which it was
added. Re-adding a present quad SHALL keep its original tick. Stamping SHALL NOT add rows written per quad.

#### Scenario: Added since
- **WHEN** quads are added in three separate writes and a query asks for quads added after the first write
- **THEN** it returns exactly the quads of the second and third writes

#### Scenario: Re-adding keeps the tick
- **WHEN** a quad added at tick 3 is inserted again at tick 5
- **THEN** its tick is still 3

#### Scenario: Stamping is not history
- **WHEN** a query with the version option runs on a `stamped` store
- **THEN** it fails with an error stating that the store keeps no history

### Requirement: Opening never changes the level
Opening an existing store SHALL use the level recorded in it. Opening with a lower level, or without the option,
SHALL NOT downgrade the store. Opening with a higher level SHALL fail and direct the user to the explicit
level change.

#### Scenario: Old configuration
- **WHEN** a store at level `log` is opened with default options
- **THEN** it opens at level `log` and keeps recording history

#### Scenario: Implicit upgrade refused
- **WHEN** a store at level `off` is opened with level `log`
- **THEN** opening fails with an error naming the explicit level change

### Requirement: Explicit level change
The system SHALL change a store's level only through an explicit operation. The operation SHALL be available
from the API, the CLI and the JavaScript engine, and as a migration script for D1. Each step SHALL be atomic
and SHALL be reported by the level status.

#### Scenario: Migration for D1
- **WHEN** a migration from `off` to `log` is generated and applied with the D1 migration tool
- **THEN** the store opens at level `log` and the next write is recorded as a commit

### Requirement: Upgrades keep all data
Raising the level SHALL never lose data:
- Quads present before stamping SHALL carry tick 0.
- History SHALL start at a genesis commit that records every quad present at the upgrade.
- As-of queries earlier than genesis SHALL fail.

#### Scenario: Stamping an existing store
- **WHEN** an `off` store holding quads is raised to `stamped`
- **THEN** the existing quads report tick 0 and later writes get higher ticks

#### Scenario: Before genesis
- **WHEN** a store raised to `log` is queried as of a tick before the genesis commit
- **THEN** the query fails with an error stating that the version is before the recorded history

#### Scenario: Genesis holds the store
- **WHEN** a store holding quads is raised to `log` and quads are removed afterwards
- **THEN** an as-of query at genesis returns every quad that was present at the upgrade

### Requirement: Downgrades are explicit about loss
By default, lowering the level SHALL keep recorded data:
- Lowering from `log` SHALL freeze history as read-only, and it SHALL remain queryable up to the freeze.
- Lowering from `stamped` SHALL keep existing ticks.
- Lowering from `history` SHALL be refused while branches other than `main` exist.

Deleting history, dropping ticks or discarding branches SHALL require an explicit allow-loss flag, and SHALL
leave an audit record.

#### Scenario: Frozen history
- **WHEN** a `log` store is lowered to `stamped` and then written to
- **THEN** as-of queries up to the freeze still answer, and later writes are not recorded as commits

#### Scenario: Branches block a downgrade
- **WHEN** a `history` store with branch `fix` is lowered to `log` without allow-loss
- **THEN** the change fails, naming `fix`

#### Scenario: Gap after re-upgrade
- **WHEN** a store lowered from `log` is later raised to `log` again, and a query asks for a time between the freeze and the new genesis
- **THEN** the query fails with an error naming the gap

### Requirement: Every write is a commit
In a store at level `log` or higher, every atomic write SHALL produce exactly one commit carrying a time, an
optional author and an optional message. This covers SPARQL UPDATE, insert, remove, a load batch, a Cypher
write and a document store. A commit SHALL record only effective changes: adding a quad already present, or
removing an absent one, SHALL NOT be recorded.

#### Scenario: One update, one commit
- **WHEN** `DELETE { ?s ex:v ?o } INSERT { ?s ex:v 2 } WHERE { ?s ex:v ?o }` runs with message "bump"
- **THEN** one new commit with message "bump" records the removed and the added quads

#### Scenario: No-op changes are not recorded
- **WHEN** a quad already in the store is inserted again
- **THEN** the commit records no addition for it

#### Scenario: Every writer is captured
- **WHEN** quads are written through Cypher `CREATE` or a JSON-LD document store
- **THEN** those changes appear in the history like SPARQL changes

### Requirement: History is immutable
Recorded changes and commits SHALL NOT be modifiable or deletable through any store operation except
`purge` and an explicit allow-loss downgrade. Attempts to alter them SHALL fail and leave the store unchanged.

#### Scenario: Altering history fails
- **WHEN** a statement tries to update or delete a recorded change directly
- **THEN** the request fails with an error naming history immutability and nothing changes

### Requirement: Version references
The system SHALL accept a version reference wherever a version is named. The accepted forms are:
- `HEAD` or `main` (the latest tick)
- `HEAD~n` (n commits back)
- `@<xsd:dateTime>` or `HEAD@<xsd:dateTime>` (the latest tick at or before that time)
- a tick, `#42` or `42`

An unknown reference, or one outside the recorded history, SHALL be an error.

#### Scenario: Relative reference
- **WHEN** three commits were made on `main` and a query names `main~1`
- **THEN** it reads the state after the second commit

#### Scenario: Time reference
- **WHEN** a query names `main@2026-09-01T00:00:00Z`
- **THEN** it reads the state of the last commit made at or before that instant

#### Scenario: Unknown tick
- **WHEN** a query names a tick the store never recorded
- **THEN** the query fails with an error naming the version

### Requirement: As-of queries
A SPARQL query or a Datalog program SHALL accept a version option and then return exactly the results it
would have returned on the store at that version. Queries without the option SHALL read the current head
with no added cost.

#### Scenario: Past state equals snapshot
- **WHEN** a random sequence of updates is applied, and for each commit a query runs as of that commit
- **THEN** each result equals the result recorded right after that commit

#### Scenario: Head query unaffected
- **WHEN** the same query runs without a version option
- **THEN** its plan and results are those of an unversioned store with the same data

#### Scenario: Inferences are not versioned
- **WHEN** an as-of query asks to include materialized inferences
- **THEN** it fails with an error explaining that inferences exist only for the head

### Requirement: Comparing versions within a SPARQL query
A SPARQL query SHALL be able to evaluate a group pattern at another version with
`SERVICE <oxilite:version/REF> { … }`, joined with the rest of the query read at the query's own version.
`GRAPH` inside the group SHALL keep its meaning.

#### Scenario: Changed values
- **WHEN** `SELECT ?s ?old ?new { ?s ex:status ?new SERVICE <oxilite:version/main~1> { ?s ex:status ?old } FILTER(?old != ?new) }` runs
- **THEN** it returns the subjects whose status changed in the last commit with both values

### Requirement: Diff and change feed
The system SHALL report the net quads added and removed between any two versions, and every change
recorded after a tick in order. With only the store clock, the change feed SHALL report the current quads
added after the tick.

#### Scenario: Diff of two versions
- **WHEN** one update replaces a status and `diff("HEAD~1", "HEAD")` runs
- **THEN** it reports the new status as added and the old one as removed

#### Scenario: Change feed
- **WHEN** quads are added and removed in several commits and the changes after the first commit are read
- **THEN** they are returned in tick order, each marked as added or removed

### Requirement: Writes target the head
Writes SHALL apply to the current state. An update SHALL NOT read a past version: a
`SERVICE <oxilite:version/…>` pattern in an update SHALL fail with an error stating that versions are read
in queries only.

#### Scenario: Reading the past in an update
- **WHEN** `INSERT { ?s ?p ?o } WHERE { SERVICE <oxilite:version/HEAD~1> { ?s ?p ?o } }` runs
- **THEN** it fails with an error stating that versions are read in queries, and nothing is written

### Requirement: Purge
The system SHALL provide an explicit `purge` operation that removes given quads from the head and from all
history. It SHALL record an audit entry (who, when, why) without the purged content.

#### Scenario: Erasure request
- **WHEN** `purge` removes every quad about `ex:alice`
- **THEN** no current or as-of query returns them, and the history lists a purge entry without her data
