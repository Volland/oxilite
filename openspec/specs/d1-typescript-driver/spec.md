# d1-typescript-driver Specification

## Purpose
Lets TypeScript Cloudflare Workers use oxilite on a D1 binding with a Promise-based version of the same API as the Node package.

## Requirements

### Requirement: Open on a D1 binding
The package SHALL open a store on a `D1Database` binding without native code, running the oxilite core as WebAssembly.

#### Scenario: Worker query
- **WHEN** a Worker creates a store from `env.DB` and awaits a SELECT query
- **THEN** it receives the same result shape as the Node package

### Requirement: Async API parity
The package SHALL provide `query`, `update`, `load`, `dump`, `add`, `delete`, `has`, `match`, `size`, `explain` and `optimize` as async functions with the same arguments and result types as the Node package.

#### Scenario: Update then query
- **WHEN** an INSERT DATA update is awaited and then a query runs
- **THEN** the query sees the inserted data

### Requirement: Batch atomicity
The package SHALL send every atomic request as one D1 `batch()` call.

#### Scenario: Failed update
- **WHEN** an update fails on D1
- **THEN** none of its changes are visible

### Requirement: Schema setup
The package SHALL export the schema as SQL for `wrangler d1 migrations`, and SHALL provide an idempotent `init()` for databases that haven't been migrated.

#### Scenario: Init twice
- **WHEN** `init()` runs twice on the same database
- **THEN** the second call makes no changes
