## ADDED Requirements

### Requirement: Versioned atoms
An atom that reads the store (an IRI predicate, `triple` or `quad`) SHALL accept a suffix `at "REF"` to read
that version, or `at ?c` to read the version of the commit bound to `?c` by a positive atom of the same rule
(typically `commit`). An `at` variable no positive atom binds SHALL be rejected as unsafe, naming the variable.
`at` with materialized inferences SHALL be refused.

#### Scenario: Comparing two versions
- **WHEN** `changed(?t, ?old, ?new) :- ex:status(?t, ?new), ex:status(?t, ?old) at "HEAD~1", ?old != ?new.` runs after one commit changed a status
- **THEN** it returns that subject with its old and new status

#### Scenario: The value at every commit
- **WHEN** `status_at(?c, ?v) :- commit(?c, _, _, _), ex:status(ex:t1, ?v) at ?c.` runs
- **THEN** it returns, for every commit, the status `ex:t1` had at that commit

#### Scenario: Unsafe version variable
- **WHEN** a goal `ex:status(ex:t1, ?v) at ?c` runs and nothing binds `?c`
- **THEN** the program is rejected as unsafe, naming `?c`

### Requirement: History relations
On a store that keeps history, a program SHALL be able to read these built-in relations. A commit is its tick,
an integer.
- `commit(?c, ?parent, ?time, ?author)`: every commit, the commit before it, its time and its author
- `added(?s, ?p, ?o, ?g, ?c)`, `removed(?s, ?p, ?o, ?g, ?c)`: the changes of the log; the default graph's
  `?g` is unbound
- `branch(?name, ?c)`: `"main"` and its latest tick (until branches exist)

A program that defines rules for one of these names SHALL keep its own relation. On a store without
history, using them SHALL be refused.

#### Scenario: Who removed each value
- **WHEN** `removed_by(?t, ?v, ?who) :- removed(?t, ex:status, ?v, _, ?c), commit(?c, _, _, ?who).` runs
- **THEN** it returns every removed status with the author of the commit that removed it

#### Scenario: A program's own relation
- **WHEN** a program defines `commit(?x) :- triple(?x, _, _).`
- **THEN** `commit` is that relation, not the built-in

#### Scenario: History on an unversioned store
- **WHEN** a program uses `commit/4` on a store created without versioning
- **THEN** it fails with an error stating that the store keeps no history
