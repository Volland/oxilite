## 1. Turso backend

- [x] 1.1 `crates/oxilite-turso`: workspace member, `turso = "=0.8.0-pre.14"` without default features
- [x] 1.2 `TursoBackend`: `memory`, `open`, `from_database`; sync `execute` over `block_on`; async
      `execute`; savepoint-scoped atomic requests; interactive transactions
- [x] 1.3 Dialect shim: `WITHOUT ROWID, STRICT` → `STRICT`; FTS5 → unsupported error
- [x] 1.4 `Capabilities::vectors` and `vector_index_methods` (false by default, true on Turso)
- [x] 1.5 Umbrella `turso` feature: `Store::new_turso`, `open_turso`, `open_turso_with_options`
- [x] 1.6 Tests: SPARQL, updates, property paths, registry, Cypher and Datalog on Turso; rollback;
      reopen; the text index refusal

## 2. Vector indexes

- [x] 2.1 `oxilite_core::vector`: `VectorIndex`, `Metric`, `ElementType`, validation, RDF form
      (`to_quads`, `from_quads`), vocabulary terms in `oxl:`
- [x] 2.2 DDL: table, insert/delete triggers with the dimension check, back-fill, sparse IVF index,
      drop statements
- [x] 2.3 `Stats`: definitions from `<oxilite:vectors>` and built tables, loaded only with `vectors`
- [x] 2.4 Sync plan: definitions vs. tables → create / drop / rebuild statements
- [x] 2.5 Store API: `create_vector_index`, `drop_vector_index`, `vector_indexes`,
      `sync_vector_indexes`, `vector_search`; SPARQL updates of `<oxilite:vectors>` sync; sync at
      open when out of date
- [x] 2.6 Tests: RDF round trip, triggers on every write path, dimension errors, rollback, back-fill,
      SPARQL-defined indexes, redefinition, unsupported backends

## 3. Vector search

- [x] 3.1 `vector::knn_sql`: dense, class-restricted, by node, sparse (index-method shape); score
      expressions per metric
- [x] 3.2 SPARQL: `SERVICE <oxilite:vector/NAME>` in the compiler as a derived table; errors for
      unknown index and predicates; partial evaluation keeps it in SQL
- [x] 3.3 Cypher: `CALL db.index.vector.queryNodes` lowered into the SQL stage with node and value
      bindings; `CREATE VECTOR INDEX`, `DROP INDEX`, `SHOW VECTOR INDEXES` as schema commands run
      by the store
- [x] 3.4 Datalog: `nearest/4` and `nearest/5` built-ins compiled with a rank column; index
      definitions passed through `Options`
- [x] 3.5 Tests: ordering, class, by node, dimensions error, SPARQL joined search in one statement,
      Cypher node binding, Datalog rank, sparse Jaccard

## 4. Host functions

- [x] 4.1 `oxilite_core::functions`: `HostFunction`, `FunctionRegistry` (IRI, Cypher name, arity,
      description), clash checks
- [x] 4.2 SPARQL: registry functions in the fallback and partial evaluators, queries and updates
- [x] 4.3 Cypher: lowering to `Function::Custom`; Rust-tail evaluation with value conversion
- [x] 4.4 Datalog: expression and atom forms; host-rule split, Rust evaluation, fact injection in
      dependency order; recursion check; goal constraints
- [x] 4.5 Store API: `register_function`, `unregister_function`, `functions`
- [x] 4.6 Tests per language, including unknown functions and the recursion rejection

## 5. Command line and studio

- [x] 5.1 `--turso` on every command (`Db::Turso`)
- [x] 5.2 Shell `.vector list|create|drop|search` and `.functions`
- [x] 5.3 Studio: `oxilite/attach` with `engine: "turso"`; `oxilite/vectorIndexes`,
      `vectorIndexCreate`, `vectorIndexDrop`, `vectorSearch`, `functions`
- [x] 5.4 Tests for the shell commands and the studio requests

## 6. Documentation

- [x] 6.1 `lat.md`: architecture (Turso backend, vector indexes, host functions), decisions D38–D41,
      test specifications; `lat check` passes
- [x] 6.2 Crate README for `oxilite-turso`; README table entry
