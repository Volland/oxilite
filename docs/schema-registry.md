# Schema registry: reference

A store can say which of its named graphs hold schema rather than data: RDFS/OWL ontologies, SHACL shapes, ShEx schemas. It can also say which data graphs each schema describes.

oxilite uses the registry to:
- scope query-time reasoning to the right ontologies, per data graph;
- compile SHACL shapes into the index Cypher uses;
- hide schema from queries over the data.

The registry is plain RDF in a well-known named graph, so it travels with the dataset and works on any SPARQL 1.1 store, Oxigraph included. It ships with SHACL shapes of its own, so any SHACL processor can check it.

- [The registry graph](#the-registry-graph)
- [System graphs](#system-graphs)
- [Vocabulary](#vocabulary)
- [Mapping schemas to graphs](#mapping-schemas-to-graphs)
- [Imports](#imports)
- [What oxilite does with it](#what-oxilite-does-with-it)
- [Validating the registry](#validating-the-registry)
- [SPARQL recipes (any store)](#sparql-recipes-any-store)
- [Interoperability](#interoperability)
- [APIs](#apis)
- [Upgrading](#upgrading)
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

  <https://ex.org/shapes/hr> a oxl:ShapesGraph ;
      oxl:appliesTo oxl:AllGraphs ;
      oxl:active true .
}
```

The triples of `<https://ex.org/onto/hr>` itself stay in that graph. Registering never moves, copies or rewrites them.

oxilite always writes the canonical form: `oxl:active` with an `xsd:boolean`, and at least one `oxl:appliesTo`, `oxl:AllGraphs` for "every graph". It reads the short forms too (see [Vocabulary](#vocabulary)), so a registry written by hand needs neither.

A graph can hold more than one role. An ontology that carries its own SHACL shapes is typed with both classes. It is then listed once per role, contributes axioms and shapes, and has one description (targets, active flag, version) shared by both roles:

```turtle
GRAPH <oxilite:schema> {
  <https://ex.org/onto/hr> a oxl:OntologyGraph , oxl:ShapesGraph ; oxl:appliesTo oxl:AllGraphs .
}
```

The register API writes one role and replaces the whole description, roles included. To give a graph a second role, add the type with SPARQL. `map` changes only the targets, so it keeps every role.

System graphs use the `oxilite:` IRI scheme. `<oxilite:schema>` is an ordinary, stored named graph. `<oxilite:history>` (see [Versioning](versioning.md#history-as-data)) is a virtual graph over the change log.

## System graphs

oxilite maintains two system graphs of its own, both described in the registry as `oxl:SystemGraph`:

| Graph | Holds |
|---|---|
| `<oxilite:schema>` | The registry: one description per registered graph, plus its own |
| `<oxilite:vocabulary>` | The `oxl:` vocabulary and the registry shapes ([Turtle](https://oxilitedb.com/ns/oxl.ttl)), declared to apply to `<oxilite:schema>` |

A *blank* store can start with both installed: the vocabulary graph plus these registry triples.

```turtle
GRAPH <oxilite:schema> {
  <oxilite:schema>     a oxl:SystemGraph ; rdfs:label "schema registry" .
  <oxilite:vocabulary> a oxl:SystemGraph ; rdfs:label "oxilite vocabulary" ;
      oxl:appliesTo <oxilite:schema> ; oxl:ontologyIri <https://oxilite.dev/ns#> ; oxl:version "2" .
}
```

- **Command line:** the `oxilite` shell and every subcommand install them when they create a new database. `--no-system-graphs` opts out.
- **Rust, Node and D1:** these libraries install them when asked with `StoreOptions { system_graphs: true, .. }`, `new Store({ systemGraphs: true })` or `D1Store.open(db, { systemGraphs: true })`. They are off by default, so a new store is empty, exactly as in Oxigraph.
- **D1 migration script:** `npx oxilite-d1 schema --system-graphs` (and `oxilite schema --system-graphs`) append them to the schema script.
- **Existing stores:** `install_system_graphs()`, `installSystemGraphs()` or `oxilite registry init` installs or refreshes them, and does nothing when they are already at the current vocabulary version. The SPARQL is `registry::system_graphs_update()` and runs on any store.

System graphs are part of the dataset, so they show up in `GRAPH ?g`. They are not registrations:
- `schema_graphs()` does not list them;
- they never contribute axioms or shapes, whether or not anything is registered;
- `include_schema_graphs: false` hides them with the schema graphs.

The set of system graphs is fixed: `<oxilite:schema>` and `<oxilite:vocabulary>`. If you type another graph `oxl:SystemGraph`, it is hidden with the schema graphs, but that does not take it out of reasoning or the shape index.

A blank store gets them only once, when it is opened blank. On a versioned store they are written outside the change log.

## Vocabulary

Namespace `oxl:` = `https://oxilite.dev/ns#`, version 2. The vocabulary is published at [oxilitedb.com/ns](https://oxilitedb.com/ns/) (HTML) and [oxilitedb.com/ns/oxl.ttl](https://oxilitedb.com/ns/oxl.ttl) (Turtle). The source is [`crates/oxilite-core/vocab/oxl.ttl`](../crates/oxilite-core/vocab/oxl.ttl), exposed as `oxilite::schema::VOCABULARY` and installed in `<oxilite:vocabulary>`.

| Term | Kind | Meaning |
|---|---|---|
| `oxl:SchemaGraph` | class ⊑ `sd:Graph` | A named graph holding schema rather than data |
| `oxl:OntologyGraph` | class ⊑ `oxl:SchemaGraph` | RDFS / OWL axioms, used for query-time reasoning |
| `oxl:ShapesGraph` | class ⊑ `oxl:SchemaGraph` | SHACL shapes |
| `oxl:ShExGraph` | class ⊑ `oxl:SchemaGraph` | A ShEx schema (recorded and hidden, not compiled) |
| `oxl:SystemGraph` | class ⊑ `sd:Graph`, disjoint with `oxl:SchemaGraph` | A graph oxilite maintains itself: `<oxilite:schema>`, `<oxilite:vocabulary>` |
| `oxl:GraphTarget` | class | What `oxl:appliesTo` points at: a graph IRI, `oxl:DefaultGraph` or `oxl:AllGraphs` |
| `oxl:appliesTo` | property, range `oxl:GraphTarget` | A graph the schema describes. None at all is read as `oxl:AllGraphs` |
| `oxl:active` | functional, `xsd:boolean` | An `xsd:boolean` false (`false` or `"0"^^xsd:boolean`) keeps the graph registered and hidden but stops it contributing. Absent means `true`. A plain `"false"` is invalid and ignored |
| `oxl:ontologyIri` | functional, IRI | The `owl:Ontology` IRI of the content, when it differs from the graph name |
| `oxl:version` | functional, string | A version pinned at registration (see also `owl:versionInfo`) |
| `oxl:sha256` | functional, string | Lowercase hex SHA-256 of the source document, for drift detection |
| `oxl:loadedAt` | functional, `xsd:dateTime`, ⊑ `dcterms:date` | When the graph was registered |
| `owl:imports` | IRI | Imports; those naming another registered ontology are resolved (see [Imports](#imports)) |
| `oxl:DefaultGraph` | individual, `oxl:GraphTarget` | Names the default graph, as a registered graph or as a target |
| `oxl:AllGraphs` | individual, `oxl:GraphTarget` | As a target: every graph. A selector, never a registered graph |
| `oxl:RegistrationShape` | `sh:NodeShape` | The shape every registration conforms to (see [Validating the registry](#validating-the-registry)) |

The Rust reader, the SQL that scopes the caches and the SPARQL recipes below read these terms identically. A graph typed with two roles counts for both, and only the two lexical forms of an `xsd:boolean` false deactivate a graph.

## Mapping schemas to graphs

`oxl:appliesTo` says which data a schema describes:

- **`oxl:appliesTo oxl:AllGraphs`** (or no `oxl:appliesTo`): the schema applies to every graph.
- **One or more graph IRIs, or `oxl:DefaultGraph`:** the schema applies to those graphs only. `oxl:AllGraphs` among them wins.

Two datasets can therefore live side by side with conflicting ontologies:

```turtle
GRAPH <oxilite:schema> {
  <https://ex.org/onto/zoo>    a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/zoo> .
  <https://ex.org/onto/garden> a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/garden> .
  <https://ex.org/onto/common> a oxl:OntologyGraph ; oxl:appliesTo oxl:AllGraphs .
}
```

With RDFS reasoning:
- a `ex:Dog` in the zoo graph is entailed only through the zoo and common ontologies;
- a `ex:Dog` in the garden graph only through the garden and common ones.

This holds even in a query over the union of graphs: entailment is per graph, like a context.

## Imports

When an ontology imports another *registered, active* ontology, the imported ontology's axioms join the importer's reasoning scopes. This is transitive, and import cycles are safe.

- **Where imports are read:** `owl:imports` recorded in the registry (`--import`, `with_imports`), and `owl:imports` asserted in the ontology's own graph.
- **What an import matches:** a registered ontology's graph name or its `oxl:ontologyIri`.
- **What an import does not do:** fetch anything. An import that matches no registered ontology is recorded and ignored. Load and register the imported ontology yourself.

```turtle
GRAPH <oxilite:schema> {
  <https://ex.org/onto/zoo>  a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/zoo> .
  <https://ex.org/onto/core> a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/other> ;
      oxl:ontologyIri <https://ex.org/core#> .
}
GRAPH <https://ex.org/onto/zoo> { <https://ex.org/zoo#> owl:imports <https://ex.org/core#> . }
```

Here the core axioms also apply to the zoo graph, because the zoo ontology imports them. An inactive ontology is never imported.

## What oxilite does with it

- **Reasoning** (`reasoning: rdfs | owl-ql`):
  - Only active ontology graphs contribute axioms, together with the active ontologies they import. While no ontology is registered, every graph but the system graphs contributes, as before the registry existed.
  - Registered but inactive counts as registered. Deactivating the last ontology stops reasoning over axioms; it does not fall back to every graph.
  - Each quad is entailed with the axioms that apply to its graph.
  - Internally, the TBox closure is computed per scope: one for all graphs, plus one per graph that has ontologies of its own. The rewriting picks the closure by the quad's graph.
- **Shape index:**
  - Compiled from the active shapes graphs, or from every graph but the system graphs while none is registered.
  - Cypher uses it to plan and to check writes.
  - The index is keyed by class, so it ignores shapes mappings (see [Limits](#limits)). Validators can pick shapes per graph with `schema_graphs_for`.
- **Hiding:** `include_schema_graphs: false` removes the system graphs and every registered graph, active or not, from pattern matching.
- **Change detection:** a write that can change the registry rebuilds the reasoning closure and the shape index in the same atomic request. That includes a plain SPARQL `INSERT DATA`, and any `owl:imports` write.
  - Updates with a variable graph count only when a triple could be a registry triple: a variable or `oxl:` predicate, `owl:imports`, or `rdf:type` with a variable or `oxl:` class.
  - A bulk `INSERT { GRAPH ?g { ?s a ex:Person } } WHERE …` therefore rebuilds nothing.
- **Not affected:**
  - `materialize()` (OWL 2 RL) still reasons over the merged dataset.
  - Dumps and `named_graphs()` include the registry graph: the dataset is the dataset.

## Validating the registry

The vocabulary graph contains `oxl:RegistrationShape`, which checks each registration:
- the node is an IRI with at least one role class;
- `oxl:active` is at most one `xsd:boolean`;
- `oxl:appliesTo`, `oxl:ontologyIri` and `owl:imports` are IRIs;
- `oxl:version`, `oxl:sha256` and `oxl:loadedAt` each have at most one value, of the right datatype;
- `oxl:sha256` is 64 lowercase hex digits.

It targets every role class and every subject of a registration property, so a description without a role is caught as well.

- **Any SHACL processor:** use `oxl.ttl` as the shapes graph and `<oxilite:schema>` as the data graph.
- **oxilite:** `oxilite registry check` (or `Store::registry_problems()`) checks the same constraints without a SHACL engine. It prints one line per problem and exits non-zero when it finds any.

Readers are lenient: a value that breaks the shapes is ignored, never guessed. Run `check` after writing the registry by hand.

**Drift detection.** `registry register --file F` (and the shell's `.register`) records the file's SHA-256. `oxilite registry verify GRAPH --file F` compares the file you have now with the recorded digest, and exits non-zero on drift.

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

Change the targets only, keeping roles and everything else:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
DELETE { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> oxl:appliesTo ?t } }
INSERT { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> oxl:appliesTo oxl:AllGraphs } }
WHERE  { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> a ?role OPTIONAL { <https://ex.org/onto/hr> oxl:appliesTo ?t } } }
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

Which active ontologies apply directly to a data graph (imports not followed). `?a = false` compares values, so `"0"^^xsd:boolean` matches and a plain `"false"` does not, as in oxilite:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
SELECT ?onto WHERE {
  GRAPH <oxilite:schema> {
    ?onto a oxl:OntologyGraph .
    FILTER NOT EXISTS { ?onto oxl:active ?a FILTER (?a = false) }
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
  FILTER (?g NOT IN (<oxilite:schema>, <oxilite:vocabulary>))
  FILTER NOT EXISTS { GRAPH <oxilite:schema> { ?g a ?role } }
}
```

## Interoperability

The registry aligns with standard vocabularies instead of duplicating them:

- **SHACL.** `sh:shapesGraph` links a data graph to its shapes, the reverse direction of `oxl:appliesTo`. oxilite does not store both, because two copies of one fact drift apart. Derive it for a SHACL tool that expects it:

  ```sparql
  PREFIX oxl: <https://oxilite.dev/ns#>
  PREFIX sh:  <http://www.w3.org/ns/shacl#>
  CONSTRUCT { ?data sh:shapesGraph ?shapes }
  WHERE { GRAPH <oxilite:schema> {
    ?shapes a oxl:ShapesGraph ; oxl:appliesTo ?data .
    FILTER (?data != oxl:AllGraphs)
    FILTER NOT EXISTS { ?shapes oxl:active ?a FILTER (?a = false) }
  } }
  ```

- **SPARQL Service Description.** `oxl:SchemaGraph` and `oxl:SystemGraph` are subclasses of `sd:Graph`.
- **Dublin Core / PROV.** `oxl:loadedAt` is a sub-property of `dcterms:date`, and its description points to `prov:generatedAtTime`.
- **OWL and SPDX.** `oxl:version` points to `owl:versionInfo` / `owl:versionIRI`, which stay in the ontology's own graph. `oxl:sha256` points to `spdx:checksum`.
- **Vocabulary metadata.** `owl:versionIRI`, `dcterms:license`, `vann:preferredNamespacePrefix`, and `rdfs:isDefinedBy` on every term.

## APIs

Every surface runs the recipes above.

| Surface | Register | List | Map | Other |
|---|---|---|---|---|
| Rust (`Store`, `AsyncStore`) | `register_schema_graph(graph, role, &Registration::new().applies_to([...]))` | `schema_graphs()`, `schema_graphs_for(graph, role)` | `set_schema_graph_targets(graph, &[...])` | `set_schema_graph_active`, `unregister_schema_graph`, `drop_schema_graph`, `registry_problems`, `shape_index` |
| `@oxilite/node`, `@oxilite/d1` | `registerSchemaGraph(graph, "ontology", { appliesTo: [...] })` | `schemaGraphs()` | register again | `setSchemaGraphActive`, `unregisterSchemaGraph`, `dropSchemaGraph`, `shapeIndex`; query option `include_schema_graphs` |
| `oxilite` CLI | `registry register GRAPH --role ontology [--file F] [--applies-to G]…` | `registry list [--json]` | `registry map GRAPH --to G…` | `activate`, `deactivate`, `unregister`, `drop`, `shapes`, `check`, `verify GRAPH --file F`; `query --reasoning rdfs --no-schema-graphs` |
| Shell | `.register ontology GRAPH ?FILE?` | `.registry` | `.map GRAPH TARGET…` | `.activate`, `.deactivate`, `.unregister GRAPH ?--drop?`, `.shapes`, `.reasoning`, `.schemagraphs` |
| Studio (`oxilite.toml`) | `[[graph]] role = "ontology"` | | `applies_to = ["…"]` | |

GRAPH arguments take an IRI or `DEFAULT`; `ALL` as a target means every graph. In JSON, `appliesTo` is a list of IRIs, with `https://oxilite.dev/ns#DefaultGraph` for the default graph. An empty list means every graph.

## Upgrading

### From vocabulary 1 (0.5)

Existing registries keep working as they are:
- A registration without `oxl:appliesTo` still applies to every graph.
- New registrations are written with `oxl:appliesTo oxl:AllGraphs`.

Behaviour changes:
- **Deactivating every ontology** now silences reasoning over axioms. In 0.5 it fell back to every graph, including the deactivated one.
- **A graph typed with two roles** is listed once per role. In 0.5 one role was picked arbitrarily.
- **A plain `"false"`** for `oxl:active` no longer deactivates in the listing. It never did in reasoning.
- **`owl:imports`** between registered ontologies is now followed.
- **`oxl:SystemGraph`** is no longer a subclass of `oxl:SchemaGraph`.

`oxilite registry init` (or `install_system_graphs()`) replaces `<oxilite:vocabulary>` with version 2. Then run `oxilite registry check`.

### From 0.4

0.4 kept registrations in a SQL table, `schema_graphs`. Opening a 0.4 store migrates it in one atomic request:
1. each row becomes triples of `<oxilite:schema>`;
2. the table is dropped;
3. `tbox_closure` is recreated with its scope column;
4. the schema version becomes 2.

On D1, the migration runs on `D1Store.open()`, not on `openExisting()`. The migration writes quads directly, so a versioned store's history does not record it. Graphs named by blank nodes cannot be registered and are skipped.

## Limits

- Graphs named by blank nodes cannot be registered, because another graph cannot name them.
- `owl:imports` is resolved only against registered ontologies; nothing is fetched.
- Mappings drive query-time reasoning, not `materialize()`.
- **The shape index is keyed by class, not by graph.** A shapes graph mapped to one graph still constrains Cypher writes everywhere. Scoping the index per graph needs a graph column in `shapes_index` and is planned.
- A registration describes a graph, so the roles of one graph share one description (targets, active flag). Separate registration resources, one per role, are a candidate for vocabulary 3.
- ShEx graphs are recorded and hidden; nothing compiles them.
- The namespace `https://oxilite.dev/ns#` does not yet resolve: the vocabulary is served at `https://oxilitedb.com/ns/`. The `oxilite:` IRI scheme of the system graphs is not a registered URI scheme. Both are kept for compatibility until a 1.0 decision.
