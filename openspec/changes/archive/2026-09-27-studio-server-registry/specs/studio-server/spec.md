## ADDED Requirements

### Requirement: Registry of a connection
`oxilite/registry` SHALL return the schema registry of the named or active connection as oxilite's
reader sees it, the graphs of the store with their sizes, the `owl:imports` asserted in registered
ontology graphs, the registry's problems and the state of the system graphs.

#### Scenario: Manifest ontology on the Project store
- **WHEN** the manifest maps the ontology graph `g/onto` to `g/staff`
- **THEN** `oxilite/registry` lists `g/onto` with role `ontology` and `appliesTo` `[g/staff]`, and
  `g/staff` among the graphs with its triple count

### Requirement: Registry edits
`oxilite/registryEdit` SHALL register, add a role, remap, activate, deactivate, unregister and drop
schema graphs and install the system graphs through the registry API. An edit on an attached store
SHALL need `confirmed` like an update, and a read-only connection SHALL refuse it.

#### Scenario: Edit an attached store
- **WHEN** `registryEdit` registers a graph on an attached read-write store without `confirmed`
- **THEN** it fails with code 1001, and succeeds once re-sent with `confirmed`

#### Scenario: Add a role
- **WHEN** an ontology graph gets the role `shacl` with `addRole`
- **THEN** the registry lists it once per role with the same targets
