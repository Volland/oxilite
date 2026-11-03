## Context

`schema_graphs` (released in 0.4.0) labels graphs by role in a SQL table. Scoping predicates
read it through SQL subqueries, and every surface except Rust lacks access to it. This change
replaces the table with RDF and keeps SQL only for derived caches.

## The registry graph

- **System graph:** `<oxilite:schema>`, following `<oxilite:history>`. The IRI scheme
  `oxilite:` marks system graphs. The vocabulary lives in the existing `oxl:` namespace
  `https://oxilite.dev/ns#`.
- **Subjects:** each registered graph is described with its own IRI as subject. The default
  graph is `oxl:DefaultGraph`. Graphs named by blank nodes cannot be registered, because a
  blank node cannot be named from another graph.

```turtle
GRAPH <oxilite:schema> {
  <https://ex.org/onto/hr> a oxl:OntologyGraph ;
      oxl:appliesTo <https://ex.org/data/staff> , <https://ex.org/data/contractors> ;
      oxl:active true ;
      oxl:version "2.1" ;
      oxl:ontologyIri <https://ex.org/hr#> ;
      owl:imports <http://xmlns.com/foaf/0.1/> ;
      oxl:sha256 "9f2c…" ;
      oxl:loadedAt "2026-09-26T10:00:00Z"^^xsd:dateTime .
  <https://ex.org/shapes/hr> a oxl:ShapesGraph .           # applies to all graphs
}
```

| Term | Kind | Meaning |
|---|---|---|
| `oxl:SchemaGraph` | class | A named graph holding schema rather than data |
| `oxl:OntologyGraph`, `oxl:ShapesGraph`, `oxl:ShExGraph` | classes | Roles, subclasses of `oxl:SchemaGraph` |
| `oxl:appliesTo` | property | A target graph, `oxl:DefaultGraph` or `oxl:AllGraphs`. Absent means all graphs. |
| `oxl:active` | `xsd:boolean` | Absent means true. An inactive graph stays registered and hidden, but contributes nothing. |
| `oxl:ontologyIri` | IRI | The `owl:Ontology` IRI, when it differs from the graph name |
| `oxl:version`, `oxl:sha256` | strings | Pinning and drift detection |
| `oxl:loadedAt` | `xsd:dateTime` | Set at registration |
| `owl:imports` | IRI | Recorded, not resolved |
| `oxl:DefaultGraph`, `oxl:AllGraphs` | individuals | Names for the default graph and for every graph |

## Writes are SPARQL

The core builds each registry operation as SPARQL text
(`registry::register_update`, `unregister_update`, `set_active_update`, `drop_update`,
`entries_query`):

- **Register:** `CREATE SILENT GRAPH`, then delete the graph's old description, then
  `INSERT DATA` of the new one.
- **Unregister:** `DELETE WHERE` of the description.
- **Drop:** the same, plus `DROP SILENT GRAPH` (or `CLEAR DEFAULT`).

Stores run these through their normal update path, so on every backend they:
- are atomic;
- are versioned when versioning is on;
- refresh the caches.

A caller using Oxigraph runs the same text. The operations that report something (whether a
registration existed, how many quads were dropped) first run a query, then the update.

The update planner and the quad writers treat anything touching the registry as a schema
change:
- a quad in `<oxilite:schema>`;
- a pattern on that graph;
- an `oxl:appliesTo` or `oxl:active` predicate;
- `rdf:type` with a role class.

Such changes rebuild both caches in the same request.

## Scoped closure

`tbox_closure(kind, scope, sub, sup)`. A scope is a graph id, or the id of `oxl:AllGraphs`
(written ALL below) for the closure that applies everywhere.

- **The axioms of scope ALL** come from active ontology graphs without a specific mapping.
  With no active ontology registered, they come from every graph, as before.
- **The axioms of a graph G** that some active ontology maps to specifically are those of the
  global ontologies plus those mapped to G. Closures do not compose across ontologies, so each
  scope gets a full closure of its own.
- **Computation:** each closure statement reads one derived table, `(s, p, o, scope)`, that
  joins the ontology quads with the mapping read from `<oxilite:schema>`. Recursion joins on
  equal scope.
- **Specific scopes** (`SELECT DISTINCT scope … <> ALL`) are loaded with the statistics.
- **Rewriting:** each closure join gets `c.scope = <scope of x.g>`.
  - With no specific scopes, that is the constant ALL. The common case compiles to exactly one
    extra equality on an indexed column.
  - Otherwise it is `CASE WHEN x.g IN (…) THEN x.g ELSE ALL END`.
- **Transitive properties** are walked only inside graphs whose scope declares them transitive.
- **Patterns answered by the closure itself** (`?a rdfs:subClassOf ?b`) read every scope,
  distinct.

## Hiding and shape scope

- **Hiding** (`include_schema_graphs: false`) excludes `<oxilite:schema>` and every graph it
  types with a role, active or not.
- **The shape index** reads the active shapes graphs, or every graph when none is registered,
  as before. Mappings of shapes graphs are recorded, and `Store::schema_graphs_for(graph,
  role)` returns what applies to a data graph, but the class-keyed index ignores them.

## Migration

- `SCHEMA_VERSION` becomes 2.
- `open` reads `sqlite_master` first:
  - an old `tbox_closure` (without `scope`) is dropped before the schema DDL recreates it;
  - if `schema_graphs` exists, its rows are read with their IRIs joined from `terms`, turned
    into registry quads, and written. The table is then dropped and the caches rebuilt, all in
    one atomic request.
- Graphs named by blank nodes are skipped.
- The migration writes quads directly, so versioned stores do not log it.

## Surfaces

- **Rust:** `Registration { iri, version, sha256, imports, applies_to, active }`, where
  `applies_to: Vec<GraphName>` is empty for all graphs. `RegisteredGraph` carries it back with
  `loaded_at: Option<String>`.
- **JSON** (bindings): the registration takes `appliesTo: string[]` of IRIs
  (`https://oxilite.dev/ns#DefaultGraph` for the default graph). Entries return it, with
  `loadedAt` as an ISO string.
- **WASM:** it exposes the SPARQL builders and the parser of listing results. `D1Store` runs
  them with its own `query` / `update`, so the D1 path is exactly the portable SPARQL path.
- **Shell and CLI:**
  - `.map GRAPH TARGET…` and `registry map GRAPH --to TARGET…` re-register with new targets;
    `ALL` resets.
  - `register --applies-to` sets targets at registration.
- **Studio:** `[[graph]] role = "ontology"` accepts `applies_to = [...]`. Shapes stay out of
  the project store, so the "shapes are not data" rule is unchanged.

## Risks

- **Closure size:** it grows with the number of specific scopes, since each gets a copy of the
  global closure. Closures are small (TBox only), so this is acceptable.
- **Registry graph visibility:** the registry graph appears in `GRAPH ?g` results. That is
  consistent with "the dataset is the dataset", and `include_schema_graphs: false` hides it.
