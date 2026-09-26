## Why

The schema registry (which named graphs hold an ontology, SHACL shapes or a ShEx schema) is a
relational table, `schema_graphs`. That has three costs:

- **Only oxilite's SQL can read it.** A dataset dumped to N-Quads loses its registrations, and
  the same setup cannot be expressed on plain Oxigraph or any other SPARQL store.
- **SPARQL cannot query or change it.** Tools that only speak RDF cannot see which graphs are
  schema.
- **It cannot say where a schema applies.** Every registered ontology applies to every graph.
  An application that keeps several datasets side by side, each with its own ontology, cannot
  express that.

The registry is also only reachable from Rust: the shell, the `oxilite` subcommands and the
JavaScript bindings cannot register a graph.

## What Changes

- **The registry becomes RDF in a well-known system graph, `<oxilite:schema>`,** described
  with a published vocabulary in the `oxl:` namespace (`https://oxilite.dev/ns#`). Each
  registered graph is a resource in that graph:
  - its role: `oxl:OntologyGraph`, `oxl:ShapesGraph` or `oxl:ShExGraph`;
  - its state: `oxl:active`;
  - what is known about it: `oxl:ontologyIri`, `oxl:version`, `oxl:sha256`, `oxl:loadedAt`,
    `owl:imports`;
  - where it applies: `oxl:appliesTo`, naming target graphs, `oxl:DefaultGraph` or
    `oxl:AllGraphs`. With no `oxl:appliesTo`, the schema applies to all graphs.
- **The `schema_graphs` table is removed.** Registering, listing, activating and dropping are
  plain SPARQL updates and queries over `<oxilite:schema>`. They run unchanged on Oxigraph, and
  the core generates them so every surface agrees.
- **Query-time reasoning honours the mapping.** An ontology that applies to graph G entails
  triples only for quads in G. Ontologies without a mapping apply everywhere. The TBox closure
  gains a `scope` column: one closure for all graphs, plus one per graph that has ontologies of
  its own. Schema scopes are loaded with the planner statistics, like transitive properties.
- **The derived caches stay SQL:** `tbox_closure` and the shape index. They are rebuilt from
  the registry graph by pure SQL, whenever the registry graph or a schema axiom changes and on
  `optimize()`.
- **Existing stores are migrated.**
  - Opening a store with schema version 1 converts its `schema_graphs` rows into registry
    triples, drops the table, rebuilds `tbox_closure` with its new column, and records schema
    version 2.
  - D1 databases migrate on `D1Store.open()`.
- **The vocabulary ships as Turtle** (`oxilite_core::registry::VOCABULARY`) and is documented
  with the SPARQL that registers, maps and lists schema graphs on any store.
- **Every surface reaches the registry:**
  - **Shell:** `.register`, `.map`, `.unregister`, `.activate`, `.deactivate`, `.registry`,
    `.shapes`, plus `.reasoning`, `.inferred`, `.schemagraphs` and `.materialize`.
  - **CLI:** `oxilite registry list|register|map|unregister|activate|deactivate|drop|shapes`,
    `oxilite materialize`, and `--reasoning` / `--inferred` / `--no-schema-graphs` on `query`,
    `explain` and `serve`.
  - **Bindings:** `@oxilite/node` and `@oxilite/d1` gain the registry methods (with
    `appliesTo`) and `include_schema_graphs`.
  - **Studio:** a manifest ontology graph may declare `applies_to`.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `schema-registry`: RDF registry graph, vocabulary, mapping, SPARQL portability, migration,
  command line.
- `reasoning`: scoped TBox closure per mapping.
- `cli-shell`: registry, mapping and query-option commands.
- `node-bindings`, `d1-typescript-driver`: registry methods, `include_schema_graphs`.
- `studio-server`: `applies_to` for ontology graphs.

## Impact

- **Storage:**
  - schema version 2;
  - `schema_graphs` dropped;
  - `tbox_closure(kind, scope, sub, sup)`;
  - a new named graph `<oxilite:schema>` in stores that register anything.
- **Behaviour:**
  - With no registration, stores behave as before: every graph contributes axioms, and to all
    data.
  - `<oxilite:schema>` is an ordinary named graph: `GRAPH ?g` sees it, and
    `include_schema_graphs: false` hides it together with the graphs it registers.
- **API:**
  - `Registration` gains `applies_to`.
  - `loadedAt` becomes an `xsd:dateTime` string.
  - The Rust registry functions keep their names.
- **Not changed:**
  - OWL 2 RL materialization still reasons over the merged dataset.
  - The shape index compiles every active shapes graph; mappings of shapes graphs are recorded
    for validators to use.
