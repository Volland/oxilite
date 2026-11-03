## 1. Registry graph (core)

- [x] 1.1 `registry`: vocabulary constants, `VOCABULARY` Turtle, SPARQL builders (register, map, unregister, activate, drop, list), listing parser, `SchemaGraph` with `applies_to`
- [x] 1.2 SQL scoping over `<oxilite:schema>`: active graphs of a role, hiding, ontology axioms tagged by scope
- [x] 1.3 Registry writes count as schema changes (quads, update patterns) and refresh both caches
- [x] 1.4 Remove `schema_graphs`, its statements and jobs

## 2. Scoped reasoning

- [x] 2.1 `tbox_closure(kind, scope, sub, sup)`; closure statements per scope
- [x] 2.2 Specific scopes in `Stats`; scope condition in the rewrite, the fallback and transitive walks
- [x] 2.3 Tests: mapped ontologies, global plus mapped, unmapped store unchanged

## 3. Migration

- [x] 3.1 Schema version 2; `open` drops an old `tbox_closure`, converts `schema_graphs` rows, drops the table
- [x] 3.2 Test with a version 1 database

## 4. Stores and surfaces

- [x] 4.1 `Store` / `AsyncStore` registry over SPARQL; `Registration::applies_to`; `schema_graphs_for`
- [x] 4.2 JSON forms; WASM builders; native addon; `@oxilite/common`, `@oxilite/node`, `@oxilite/d1`
- [x] 4.3 CLI: `registry` (with `map`, `--applies-to`), `materialize`, query flags; shell commands
- [x] 4.4 Studio: `applies_to` in the manifest; shapes stay aside
- [x] 4.5 Portability test: the registry SPARQL on Oxigraph

## 5. Docs

- [x] 5.1 `lat.md`: schema registry, reasoning, storage schema, decisions (supersede D22), shell, command line, bindings, studio, tests
- [x] 5.2 Vocabulary reference and SPARQL recipes (`docs/`), READMEs

## 6. System graphs

- [x] 6.1 `oxl:SystemGraph`, `<oxilite:vocabulary>`; `system_quads`, `system_graphs_update`, ready query
- [x] 6.2 `StoreOptions::system_graphs`: bootstrap of blank stores in `open`, schema scripts; `install_system_graphs`
- [x] 6.3 CLI on by default (`--no-system-graphs`, `registry init`); Node / D1 option and method
- [x] 6.4 Website: article, vocabulary page and Turtle at `/ns/`
