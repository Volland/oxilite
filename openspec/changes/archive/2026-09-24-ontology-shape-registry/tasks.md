## 1. Registry

- [x] 1.1 `schema_graphs` table in the core schema, with a `(role, active)` index
- [x] 1.2 `registry` module: `SchemaRole`, `SchemaGraph`, register / unregister / set-active /
      drop statement builders, load request and response decoding
- [x] 1.3 Scope predicates (`role`-filtered, empty registry means every graph) as SQL helpers
- [x] 1.4 Jobs in `ops`, and `Store` / `AsyncStore` methods

## 2. Ontology-scoped reasoning

- [x] 2.1 `closure_statements()` reads the active ontology graphs
- [x] 2.2 Recompute the closure when a registration changes
- [x] 2.3 `QueryOptions::include_schema_graphs`, applied through `Entailment::base()`

## 3. Compiled shape index

- [x] 3.1 `shapes_index` and `shapes_in` tables
- [x] 3.2 `shapes` module: refresh statements (`INSERT … SELECT`, recursive `sh:in` walk),
      load request, `ShapeIndex` / `PropertyShape` decoding
- [x] 3.3 `is_shape_quad` / `update_touches_shapes`; refresh inside writes, `optimize()`,
      `clear`, `clear_graph` and registration changes
- [x] 3.4 `Store::shape_index()` / `AsyncStore::shape_index()`

## 4. Consumers

- [x] 4.1 `oxilite-cypher`: `Shapes::from_index`, used by `cypher_schema()` and by the
      per-statement load in `exec`
- [x] 4.2 `oxilite-validate`: `shacl_schema_from_store`, `validate_shacl_stored`

## 5. Verification

- [x] 5.1 Registry tests: register, list, active flag, unregister, drop, reopen
- [x] 5.2 Reasoning scoped to registered ontologies; unregistered store unchanged
- [x] 5.3 Schema-graph hiding
- [x] 5.4 Shape index: refresh in the write, `sh:in`, merged shapes, deletion
- [x] 5.5 Agreement between the shape index and the shapes SPARQL query
- [x] 5.6 Validation from stored shapes equals validation from text
- [x] 5.7 Existing suites still pass (reasoning, cypher, validation)
- [x] 5.8 Update `lat.md/` and run `lat check`
