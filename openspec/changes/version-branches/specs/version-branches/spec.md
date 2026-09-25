## Purpose

Lets a versioned store hold several lines of history, so changes can be prepared, reviewed and validated on
a branch and merged into the main line with set-semantics conflict detection.

## ADDED Requirements

### Requirement: Branches
The system SHALL create a branch from any version reference, list branches with their heads, and delete a
branch other than the checked-out one. A new store SHALL start with the branch `main` checked out.

#### Scenario: Branch from a past commit
- **WHEN** `branch("fix", "main~2")` runs
- **THEN** `fix` exists and a query as of `fix` equals a query as of `main~2`

### Requirement: Commits on any branch
A write SHALL be able to target a named branch. Writing to a branch that is not checked out SHALL leave the
current-state results unchanged.

#### Scenario: Writing to another branch
- **WHEN** a quad is inserted on `fix` while `main` is checked out
- **THEN** a head query does not return it, and a query as of `fix` does

### Requirement: Checkout
Checking out a branch SHALL make current-state queries return that branch's state. The work SHALL be
proportional to the difference between the two states.

#### Scenario: Switching branches
- **WHEN** `fix` is checked out
- **THEN** head queries return exactly what a query as of `fix` returned before the checkout

### Requirement: Merge
Merging branch B into branch A SHALL compute, from their nearest common ancestor, the set union of both
sides' changes. It SHALL record a commit with two parents. A conflict exists only when one side adds a quad
the other side removes. Conflicts SHALL be reported, and resolved by an explicit policy (`ours`, `theirs`,
or fail), which defaults to fail.

#### Scenario: Disjoint changes merge cleanly
- **WHEN** `main` adds quad X and `fix` adds quad Y, then `fix` is merged into `main`
- **THEN** `main` contains X and Y, and its head commit has two parents

#### Scenario: Add versus remove
- **WHEN** `main` removes quad Z and `fix` adds Z again, then they are merged with the default policy
- **THEN** the merge fails, reports Z, and nothing changes

#### Scenario: Fast-forward
- **WHEN** `main` has no commits since `fix` branched, and `fix` is merged into `main`
- **THEN** `main` moves to `fix`'s head and no merge commit is created

### Requirement: Validated merge
A merge SHALL optionally require that the merged state conform to the registered SHACL shapes. If it does
not conform, the merge SHALL be aborted with the validation report.

#### Scenario: Merge blocked by shapes
- **WHEN** a merge would leave a node violating an `sh:maxCount 1` constraint and validation is required
- **THEN** the merge is aborted and the report names the node and the shape
