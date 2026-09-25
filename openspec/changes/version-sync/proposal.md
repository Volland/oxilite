## Status

Planned, deferred. Phase 3 of versioning, built when a user needs to synchronize stores. It depends on
`version-branches`.

## Why

A knowledge graph edited on a laptop and served from D1, or shared by several devices, needs to move
history between stores without re-encoding it and without losing commit identity.

## What Changes

- Content-addressed commit ids.
- Push and pull of commit ranges between stores: files first, then D1.
- An RDF Patch-style text format.
- Diverged histories merged through `version-branches`.

## Capabilities

### New Capabilities
- `version-sync`: content-addressed commits, push and pull, the patch format.

## Impact

- **Code:** `oxilite-core::version` (ids, patch export and import), the CLI (`push`, `pull`, remotes), and the
  D1 connection code shared with the studio.
