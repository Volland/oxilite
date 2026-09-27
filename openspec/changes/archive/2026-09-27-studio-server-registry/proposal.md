## Why

oxilite studio gains a Schema Registry view (studio change `schema-registry-view`): the registry of
a connection, a mapping graph between its named graphs, and edits. The studio holds no engine
state, so the server has to read the registry with oxilite's own reader and apply edits through
the registry API, on the Project store and on attached SQLite and D1 stores alike.

## What Changes

- `oxilite/registry` (`connection` optional): the registry entries (one per role, the JSON of
  `schema_graph_to_json` with `graph` as an IRI string, `oxl:DefaultGraph` for the default graph),
  every graph with its triple count (`null` on remote D1), the `owl:imports` asserted in registered
  ontology graphs, `registry_problems`, which system graphs hold triples and whether they are
  current, and the connection's `ephemeral` and `readOnly` flags.
- `oxilite/registryEdit` (`connection`, `op`, `graph`, `role`, `appliesTo`, `confirmed`): `register`,
  `addRole`, `map`, `activate`, `deactivate`, `unregister`, `drop` and `installSystemGraphs`. Like
  an update, an edit on an attached store needs `confirmed` (code 1001), and a read-only connection
  refuses it. Answers `{changed, ephemeral, dropped?}` and refreshes the connection's views.
- Store Explorer graphs carry their registry roles and targets in the description, and use the
  `ontology`, `shapes` kinds for their icon.
- The studio server's `Handle` gains the registry methods for both backends.

## Capabilities

### Modified Capabilities
- `studio-server`

## Impact

`crates/oxilite-cli/src/studio/{mod.rs, conn.rs, d1.rs, explorer.rs, registry.rs (new), tests.rs}`.
No change to the engine or the registry vocabulary.
