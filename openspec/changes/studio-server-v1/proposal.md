## Why

The studio's milestones S1 to v1.1 (see the `oxilite-studio` repository's `lat.md`) build on
the walking skeleton: language intelligence, the modelling loop, rules and graphs, trust, edge
connections and agents.

## What Changes

- Language features: a lenient scanner, a source-position index, completion, hover,
  definitions, references, outlines and live diagnostics for SPARQL, Turtle, Datalog and Cypher.
- Connections: attached SQLite files, local and remote D1 (HTTP API, billed-row meter,
  confirmation with estimates); updates, Cypher writes and imports confirmed.
- The manifest `oxilite.toml`, per-graph reload, reasoning profiles, ontology registration,
  rule producers, background SHACL and ShEx validation mapped to source lines.
- The Store Explorer, resource view data with inferred marks, justifications ("why?"), the
  ontology diagram and the Datalog debugger.
- Knowledge-graph tests, `oxilite check` and `oxilite mcp`.

## Capabilities

### Modified Capabilities
- `studio-server`

## Impact

`crates/oxilite-cli` only, plus `inference-provenance` in the core.
