## Why

An ontology has no identity in oxilite today. It is triples in `quads`, and `tbox_closure` is
built with `FROM quads` and no graph filter at all: schema axioms are found by sniffing
predicates (`reason::is_schema_quad`), never by provenance. You cannot list what ontologies are
loaded, version one, deactivate one, drop one, or reason under one rather than the union of all.

SHACL shapes have a worse problem: two sources of truth. `oxilite-cypher` reads them *from the
dataset* with a SPARQL query (`schema::schema_query`), re-run once per writing statement — a
round trip per write on D1. `oxilite-validate` takes them as a `&str` from *outside* the store
(`shacl_schema`). The same shapes can be stored and ignored by the validator, or used by the
validator and absent from the store. Unlike the TBox, shapes have no compiled cache.

Both are the same missing concept: a graph whose *role* is schema rather than data.

## What Changes

- A `schema_graphs(g, role, iri, version, sha256, imports, active, loaded_at)` registry in the
  core schema. Roles: ontology, SHACL, ShEx. The RDF stays in `quads` — the registry labels a
  graph, it does not move triples. This mirrors `oxilite-jsonld`'s `jsonld_graphs`.
- The TBox closure reads only **active ontology graphs** when any is registered, and every
  graph otherwise (so existing stores are unaffected). The filter is a SQL subquery inside the
  existing `WITH RECURSIVE … INSERT`, so nothing gains a round trip.
- `QueryOptions::include_schema_graphs` (default `true`). Set to `false`, registered schema
  graphs drop out of every pattern scan, so a query over the data is not drowned in axioms.
- A compiled `shapes_index` / `shapes_in` cache: the SHACL property shapes of registered
  shapes graphs, pre-resolved to `(target, path, datatype, min, max, pattern, relationship,
  sh:in)`. Filled by pure `INSERT … SELECT` statements over `quads`, refreshed inside the same
  atomic request as any write that touches a SHACL predicate — exactly how `tbox_closure` is
  maintained.
- `oxilite-cypher` reads its schema from `shapes_index` instead of running a SPARQL query per
  writing statement.
- `oxilite-validate` gains `shacl_schema_from_store` / `validate_shacl_stored`, so rudof can be
  driven by shapes that live in the store.
- Store API (blocking and async): `register_schema_graph`, `schema_graphs`,
  `set_schema_graph_active`, `unregister_schema_graph`, `drop_schema_graph`, `shape_index`.

## Capabilities

### New Capabilities
- `schema-registry`: ontology and shapes graphs registered by role, with a compiled shape index
  and query-time schema-graph hiding.

### Modified Capabilities
- `reasoning`: the closure is scoped to registered ontology graphs.
- `shape-validation`: shapes may come from the store rather than a string.
- `ontology-aware-cypher`: the Cypher schema comes from the compiled index.

## Non-goals

Deliberately out of scope, to keep this change reviewable:

- `owl:imports` resolution and a persisted ontology-document cache.
- Deriving shapes from ontology axioms (or the reverse).
- Validate-on-write for SPARQL UPDATE.
- A `schema` CLI subcommand and TypeScript bindings.

## Impact

- Schema additions (`schema_graphs`, `shapes_index`, `shapes_in`), all `IF NOT EXISTS`, empty
  on existing stores, so opening an old database is a no-op migration.
- No new dependency.
- No write-cost change on D1 unless shapes graphs are registered: the shape refresh runs only
  when a write touches a SHACL predicate, and the registry itself is one row per graph.
- Default behaviour is unchanged for a store that registers nothing.
