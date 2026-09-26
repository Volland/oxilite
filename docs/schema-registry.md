# Schema registry: reference

A store can say which of its named graphs hold schema rather than data: RDFS/OWL ontologies, SHACL shapes, ShEx schemas. It can also say which data graphs each schema describes.

oxilite uses the registry to:
- scope query-time reasoning to the right ontologies, per data graph;
- compile SHACL shapes into the index Cypher uses;
- hide schema from queries over the data.

The registry is plain RDF in a well-known named graph, so it travels with the dataset and works on any SPARQL 1.1 store, Oxigraph included.

- [The registry graph](#the-registry-graph)
- [System graphs](#system-graphs)
- [Vocabulary](#vocabulary)
- [Mapping schemas to graphs](#mapping-schemas-to-graphs)
- [What oxilite does with it](#what-oxilite-does-with-it)
- [SPARQL recipes (any store)](#sparql-recipes-any-store)
- [APIs](#apis)
- [Upgrading from 0.4](#upgrading-from-04)
- [Limits](#limits)

## The registry graph

Registrations live in the system graph `<oxilite:schema>`. Each registered graph is described there, with the graph's own IRI as subject:

```turtle
PREFIX oxl: <https://oxilite.dev/ns#>
PREFIX owl: <http://www.w3.org/2002/07/owl#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

GRAPH <oxilite:schema> {
  <https://ex.org/onto/hr> a oxl:OntologyGraph ;
      oxl:appliesTo <https://ex.org/data/staff> , <https://ex.org/data/contractors> ;
      oxl:active true ;
      oxl:version "2.1" ;
      oxl:ontologyIri <https://ex.org/hr#> ;
      owl:imports <http://xmlns.com/foaf/0.1/> ;
      oxl:sha256 "9f2c…" ;
      oxl:loadedAt "2026-09-26T10:00:00Z"^^xsd:dateTime .

  <https://ex.org/shapes/hr> a oxl:ShapesGraph ;        # no oxl:appliesTo: every graph
      oxl:active true .
}
```

The triples of `<https://ex.org/onto/hr>` itself stay in that graph. Registering never moves, copies or rewrites them.

System graphs use the `oxilite:` IRI scheme. `<oxilite:schema>` is an ordinary, stored named graph. `<oxilite:history>` (see [Versioning](versioning.md#history-as-data)) is a virtual graph over the change log.

## System graphs

oxilite maintains two system graphs of its own, both described in the registry as `oxl:SystemGraph`:

| Graph | Holds |
|---|---|
| `<oxilite:schema>` | The registry: one description per registered graph, plus its own |
| `<oxilite:vocabulary>` | The `oxl:` vocabulary itself ([Turtle](https://oxilitedb.com/ns/oxl.ttl)), declared to apply to `<oxilite:schema>` |

A *blank* store can start with both installed: the vocabulary graph plus these registry triples.

```turtle
GRAPH <oxilite:schema> {
  <oxilite:schema>     a oxl:SystemGraph ; rdfs:label "schema registry" .
  <oxilite:vocabulary> a oxl:SystemGraph ; rdfs:label "oxilite vocabulary" ;
      oxl:appliesTo <oxilite:schema> ; oxl:ontologyIri <https://oxilite.dev/ns#> ; oxl:version "1" .
}
```

- **Command line:** the `oxilite` shell and every subcommand install them when they create a new database. `--no-system-graphs` opts out.
- **Rust, Node and D1:** these libraries install them when asked with `StoreOptions { system_graphs: true, .. }`, `new Store({ systemGraphs: true })` or `D1Store.open(db, { systemGraphs: true })`. They are off by default, so a new store is empty, exactly as in Oxigraph.
- **D1 migration script:** `npx oxilite-d1 schema --system-graphs` (and `oxilite schema --system-graphs`) append them to the schema script.
- **Existing stores:** `install_system_graphs()`, `installSystemGraphs()` or `oxilite registry init` installs or refreshes them, and does nothing when they are already at the current vocabulary version. The SPARQL is `registry::system_graphs_update()` and runs on any store.

System graphs are part of the dataset, so they show up in `GRAPH ?g`. They are not registrations:
- `schema_graphs()` does not list them;
- they never narrow reasoning or the shape index;
- their axioms do not join the "every graph" fallback;
- `include_schema_graphs: false` hides them with the schema graphs.

A blank store gets them only once, when it is opened blank. On a versioned store they are written outside the change log.

## Vocabulary

Namespace `oxl:` = `https://oxilite.dev/ns#`. The vocabulary is published at [oxilitedb.com/ns](https://oxilitedb.com/ns/) (HTML) and [oxilitedb.com/ns/oxl.ttl](https://oxilitedb.com/ns/oxl.ttl) (Turtle). The source is [`crates/oxilite-core/vocab/oxl.ttl`](../crates/oxilite-core/vocab/oxl.ttl), exposed as `oxilite::schema::VOCABULARY` and installed in `<oxilite:vocabulary>`.

| Term | Kind | Meaning |
|---|---|---|
| `oxl:SchemaGraph` | class | A named graph holding schema rather than data |
| `oxl:SystemGraph` | class ⊑ `oxl:SchemaGraph` | A graph oxilite maintains itself: `<oxilite:schema>`, `<oxilite:vocabulary>` |
| `oxl:OntologyGraph` | class ⊑ `oxl:SchemaGraph` | RDFS / OWL axioms, used for query-time reasoning |
| `oxl:ShapesGraph` | class ⊑ `oxl:SchemaGraph` | SHACL shapes |
| `oxl:ShExGraph` | class ⊑ `oxl:SchemaGraph` | A ShEx schema (recorded and hidden, not compiled) |
| `oxl:appliesTo` | property | A graph the schema describes: a graph IRI, `oxl:DefaultGraph` or `oxl:AllGraphs` |
| `oxl:active` | `xsd:boolean` | `false` keeps the graph registered (and hidden) but stops it contributing. Absent means `true`. |
| `oxl:ontologyIri` | IRI | The `owl:Ontology` IRI of the content, when it differs from the graph name |
| `oxl:version` | string | A version pinned at registration |
| `oxl:sha256` | string | Hex SHA-256 of the source document, for drift detection |
| `oxl:loadedAt` | `xsd:dateTime` | When the graph was registered |
| `owl:imports` | IRI | Imports, recorded but not resolved |
| `oxl:DefaultGraph` | individual | Names the default graph, as a registered graph or as a target |
| `oxl:AllGraphs` | individual | As a target: every graph |

## Mapping schemas to graphs

`oxl:appliesTo` says which data a schema describes:

- **No `oxl:appliesTo`** (or `oxl:appliesTo oxl:AllGraphs`): the schema applies to every graph.
- **One or more graph IRIs, or `oxl:DefaultGraph`:** the schema applies to those graphs only.

Two datasets can therefore live side by side with conflicting ontologies:

```turtle
GRAPH <oxilite:schema> {
  <https://ex.org/onto/zoo>    a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/zoo> .
  <https://ex.org/onto/garden> a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/garden> .
  <https://ex.org/onto/common> a oxl:OntologyGraph .     # applies everywhere
}
```

With RDFS reasoning:
- a `ex:Dog` in the zoo graph is entailed only through the zoo and common ontologies;
- a `ex:Dog` in the garden graph only through the garden and common ones.

This holds even in a query over the union of graphs.

## What oxilite does with it

- **Reasoning** (`reasoning: rdfs | owl-ql`):
  - Only active ontology graphs contribute axioms. While none is registered, every graph does, which was the behaviour before the registry.
  - Each quad is entailed with the axioms that apply to its graph.
  - Internally, the TBox closure is computed per scope: one for all graphs, plus one per graph that has ontologies of its own. The rewriting picks the closure by the quad's graph.
- **Shape index:**
  - Compiled from the active shapes graphs, or from every graph while none is registered.
  - Cypher uses it to plan and to check writes.
  - The index is keyed by class, so it ignores shapes mappings. Validators can pick shapes per graph with `schema_graphs_for`.
- **Hiding:** `include_schema_graphs: false` removes `<oxilite:schema>` and every graph it registers, active or not, from pattern matching.
- **Change detection:** any write to `<oxilite:schema>` rebuilds the reasoning closure and the shape index in the same atomic request. That includes a plain SPARQL `INSERT DATA`.
- **Not affected:**
  - `materialize()` (OWL 2 RL) still reasons over the merged dataset.
  - Dumps and `named_graphs()` include the registry graph: the dataset is the dataset.

## SPARQL recipes (any store)

These are the updates and queries oxilite itself runs (see `oxilite_core::registry`). They work unchanged on Oxigraph or any SPARQL 1.1 store.

Register an ontology for two graphs, replacing any previous description:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
CREATE SILENT GRAPH <https://ex.org/onto/hr> ;
DELETE WHERE { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> ?p ?o } } ;
INSERT DATA { GRAPH <oxilite:schema> {
  <https://ex.org/onto/hr> a oxl:OntologyGraph ; oxl:active true ;
      oxl:appliesTo <https://ex.org/data/staff> , <https://ex.org/data/contractors> .
} }
```

Deactivate a registration:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
DELETE { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> oxl:active ?a } }
INSERT { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> oxl:active false } }
WHERE  { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> a ?role OPTIONAL { <https://ex.org/onto/hr> oxl:active ?a } } }
```

Unregister (the ontology's triples stay), or drop it with its triples:

```sparql
DELETE WHERE { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> ?p ?o } }
```

```sparql
DELETE WHERE { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> ?p ?o } } ;
DROP SILENT GRAPH <https://ex.org/onto/hr>
```

Which active ontologies apply to a data graph:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
SELECT ?onto WHERE {
  GRAPH <oxilite:schema> {
    ?onto a oxl:OntologyGraph .
    FILTER NOT EXISTS { ?onto oxl:active false }
    FILTER (NOT EXISTS { ?onto oxl:appliesTo ?any }
            || EXISTS { ?onto oxl:appliesTo <https://ex.org/data/staff> }
            || EXISTS { ?onto oxl:appliesTo oxl:AllGraphs })
  }
}
```

Query the data without the schema, on a store that cannot hide graphs:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
SELECT ?s ?p ?o WHERE {
  GRAPH ?g { ?s ?p ?o }
  FILTER (?g != <oxilite:schema>)
  FILTER NOT EXISTS { GRAPH <oxilite:schema> { ?g a ?role } }
}
```

## APIs

Every surface runs the recipes above.

| Surface | Register | List | Map | Other |
|---|---|---|---|---|
| Rust (`Store`, `AsyncStore`) | `register_schema_graph(graph, role, &Registration::new().applies_to([...]))` | `schema_graphs()`, `schema_graphs_for(graph, role)` | register again | `set_schema_graph_active`, `unregister_schema_graph`, `drop_schema_graph`, `shape_index` |
| `@oxilite/node`, `@oxilite/d1` | `registerSchemaGraph(graph, "ontology", { appliesTo: [...] })` | `schemaGraphs()` | register again | `setSchemaGraphActive`, `unregisterSchemaGraph`, `dropSchemaGraph`, `shapeIndex`; query option `include_schema_graphs` |
| `oxilite` CLI | `registry register GRAPH --role ontology [--file F] [--applies-to G]…` | `registry list [--json]` | `registry map GRAPH --to G…` | `activate`, `deactivate`, `unregister`, `drop`, `shapes`; `query --reasoning rdfs --no-schema-graphs` |
| Shell | `.register ontology GRAPH ?FILE?` | `.registry` | `.map GRAPH TARGET…` | `.activate`, `.deactivate`, `.unregister GRAPH ?--drop?`, `.shapes`, `.reasoning`, `.schemagraphs` |
| Studio (`oxilite.toml`) | `[[graph]] role = "ontology"` | | `applies_to = ["…"]` | |

GRAPH arguments take an IRI or `DEFAULT`; `ALL` as a target means every graph. In JSON, `appliesTo` is a list of IRIs, with `https://oxilite.dev/ns#DefaultGraph` for the default graph. An empty list means every graph.

## Upgrading from 0.4

0.4 kept registrations in a SQL table, `schema_graphs`. Opening a 0.4 store migrates it in one atomic request:
1. each row becomes triples of `<oxilite:schema>`;
2. the table is dropped;
3. `tbox_closure` is recreated with its scope column;
4. the schema version becomes 2.

On D1, the migration runs on `D1Store.open()`, not on `openExisting()`. The migration writes quads directly, so a versioned store's history does not record it. Graphs named by blank nodes cannot be registered and are skipped.

## Limits

- Graphs named by blank nodes cannot be registered, because another graph cannot name them.
- `owl:imports` is recorded, not followed: load and register imported ontologies yourself.
- Mappings drive query-time reasoning, not `materialize()`, and not the class-keyed shape index.
- ShEx graphs are recorded and hidden; nothing compiles them.
