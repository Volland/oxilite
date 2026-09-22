# query-planner Specification

## Purpose
Chooses join orders for basic graph patterns from dataset statistics, so multi-pattern queries run in selective-first order instead of whatever order SQLite guesses.

## Requirements

### Requirement: Statistics collection
The system SHALL compute these statistics on demand through `optimize()`:
- per-predicate triple count
- per-predicate distinct subjects and distinct objects
- per-class instance count
- total quad count

Ordinary writes MUST NOT update statistics.

#### Scenario: Optimize computes stats
- **WHEN** `optimize()` runs on a store with 1 000 `rdf:type` triples and 10 `ex:rare` triples
- **THEN** the statistics report those counts

#### Scenario: Writes leave stats untouched
- **WHEN** quads are inserted after `optimize()`
- **THEN** statistics keep their previous values until the next `optimize()`

### Requirement: Selective-first join order
The system SHALL order the triple patterns of a basic graph pattern:
1. Start with the pattern with the lowest estimated cardinality.
2. Then repeatedly pick the cheapest pattern that shares a variable with patterns already chosen.

Only when no connected pattern remains may it pick an unconnected one. The chosen order MUST be enforced in the generated SQL.

#### Scenario: Rare predicate first
- **WHEN** a BGP joins `?x a ex:Common` with `?x ex:rare ?y` and statistics show `ex:rare` is rarer
- **THEN** the generated SQL scans the `ex:rare` pattern first

#### Scenario: No Cartesian product when avoidable
- **WHEN** a BGP's patterns form a connected join graph
- **THEN** every pattern after the first shares a variable with an earlier one

### Requirement: Heuristics without statistics
The system SHALL order patterns using static heuristics when statistics are missing: bound subject before bound object before bound predicate before unbound.

#### Scenario: Fresh store
- **WHEN** a query runs on a store where `optimize()` has never run
- **THEN** a pattern with a constant subject is scanned before a pattern with only a constant predicate

### Requirement: Plans never change results
The system SHALL return the same solutions whatever join order is chosen, including when the host SQL planner is asked to order joins instead.

#### Scenario: Planner on and off
- **WHEN** the same query runs with the oxilite planner and with SQLite planning
- **THEN** the solution multisets are equal
