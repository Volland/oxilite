# d1-backend Specification

## Purpose
Runs oxilite on Cloudflare D1 from Rust Workers and from JavaScript/TypeScript Workers, within D1's statement, batch and number-precision limits.

## Requirements

### Requirement: D1 execution
The system SHALL run every store operation on a D1 database binding. Atomic requests MUST be executed as a single D1 batch.

#### Scenario: Query from a Rust Worker
- **WHEN** a Worker opens an oxilite store on `env.DB` and runs a SPARQL SELECT
- **THEN** the solutions are returned without any extension or native library

### Requirement: Exact identifiers
The system SHALL transport term ids between D1 and the driver without loss of precision, even though JavaScript numbers can't represent integers above 2^53.

#### Scenario: Large id round-trip
- **WHEN** a term whose id exceeds 2^53 is stored and queried on D1
- **THEN** the query returns that exact term

### Requirement: D1 limits
The system SHALL keep every statement below D1's maximum statement size. It SHALL NOT rely on bound parameters, and SHALL split bulk loads so no request exceeds the configured statement count.

#### Scenario: Large insert
- **WHEN** 50 000 triples are loaded with the bulk loader on D1
- **THEN** every statement is under the size limit and every request is under the statement-count limit

### Requirement: Schema as migration
The system SHALL provide its schema as a SQL migration file that `wrangler d1 migrations apply` can run. Opening a store on an already-migrated database MUST NOT change it.

#### Scenario: Migrated database
- **WHEN** the migration has been applied and a Worker opens the store
- **THEN** the store works without issuing DDL

### Requirement: Unsupported on D1 is explicit
The system SHALL return an error naming the feature when a query needs the synchronous fallback evaluator, which isn't available on D1.

#### Scenario: Custom function on D1
- **WHEN** a query calls a function that has no SQL form on D1
- **THEN** an unsupported-feature error names the function
