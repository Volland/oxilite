## Purpose

Moves history between versioned stores, for example a local SQLite file and a D1 database, so they can
share branches and stay in sync without re-encoding or losing commit identity.

## ADDED Requirements

### Requirement: Content-addressed commits
A commit's id SHALL be derived from its parents and its changes, so the same history has the same ids in
every store.

#### Scenario: Same history, same ids
- **WHEN** the same sequence of changes is committed in two fresh stores with the same metadata
- **THEN** both stores report identical commit ids

### Requirement: Push and pull
The system SHALL send the commits another store lacks (with every term they mention) and receive the
commits it lacks, for a named branch. Repeating a transfer SHALL change nothing.

#### Scenario: Round trip
- **WHEN** a local store pushes `main` to an empty D1 store
- **THEN** every as-of query returns the same results on both stores

#### Scenario: Idempotent pull
- **WHEN** a pull is repeated with no new commits
- **THEN** nothing is written

### Requirement: Diverged branches
If both sides committed since they last synchronized, pull SHALL merge using the merge rules of
`version-branches`, and push SHALL be rejected until the local branch contains the remote head.

#### Scenario: Rejected push
- **WHEN** the remote `main` has a commit the local store lacks and a push is attempted
- **THEN** the push fails and asks for a pull first

### Requirement: Portable patch format
Transferred changes SHALL be expressible as a text patch of added and removed quads in N-Quads syntax
(RDF Patch style), with commit metadata, so a transfer can be inspected and replayed from a file.

#### Scenario: Replay from a file
- **WHEN** commits are exported to a patch file and imported into a fresh store
- **THEN** the fresh store has the same commits and as-of results
