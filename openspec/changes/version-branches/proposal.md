## Status

Planned, gated. Phase 2 of versioning, after the archived phase 1 (`2026-09-25-versioned-store`). It starts
when the gate in `tasks.md` passes: as-of latency confirmed on BEAR-B, and a decision on checkpoints.

## Why

A versioned store records history, but all changes land on one line. Curated knowledge graphs and agent
workflows need to prepare changes apart, review them, validate them against SHACL, and merge them.

## What Changes

- Branches: create from any version, list, delete, write to a named branch.
- Checkout by diff.
- A three-way merge with set semantics. Conflicts are only add-versus-remove, resolved by an explicit
  policy (fail by default). Fast-forward when possible.
- An optional SHACL gate on merges.
- Branch-aware version references (`fix~2`, `fix@…`), CLI commands, and the studio's branch switcher and
  merge review.

## Capabilities

### New Capabilities
- `version-branches`: branches, checkout, merge, validated merge.

## Impact

- **Code:** `oxilite-core::version` (refs, ancestor visibility, merge), the stores, the CLI, the studio.
- **Cost:** branch levels create the as-of index (8.82 rows per triple on D1) and, if chosen, checkpoints.
