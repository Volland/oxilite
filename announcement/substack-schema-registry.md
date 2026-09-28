# Graphs that describe graphs: schema management that travels with your data

*Every triple store can hold an ontology. Very few can tell you which ontology is meant for which data, and none of the big ones keep that answer inside the dataset. This is how oxilite does it: a schema registry written in plain RDF, managed with plain SPARQL 1.1, that works on any RDF store you already run.*

> oxilite 0.7.0 · `oxl:` vocabulary 2.1 · [github.com/Volland/oxilite](https://github.com/Volland/oxilite) · [oxilitedb.com/ns](https://oxilitedb.com/ns/) · MIT or Apache-2.0

---

## The ontology that disappears

Load an ontology into a triple store and it disappears into the data.

It becomes more triples, stored in the same tables as your data and indexed the same way. The store has no idea that `ex:Dog rdfs:subClassOf ex:Animal` is a statement *about* your data and not a piece *of* it. From the inside, a schema is just a very well-connected corner of the graph.

Nothing seems wrong at first. The problems show up later, and they come as questions the store cannot answer:

- **Which ontologies are loaded right now?** You can guess by searching for `owl:Ontology`, but that finds declarations, not intent.
- **Which version?** Somebody loaded `hr.ttl` in March. Is it the same file as the one in git today?
- **Which data does this ontology describe?** Your staff graph and your supplier graph both have a `Person` class, and they don't mean the same thing.
- **Can I switch one off?** A new ontology is making every rock in the dataset an `Animal`. You want to drop its entailments without deleting it.
- **Can I query my data without fifty thousand axioms in the results?** `SELECT ?s ?p ?o` shouldn't return your TBox.

SHACL makes this worse. Shapes are RDF too, so they end up in the store next to everything else. Meanwhile the validator often reads a *different* copy from a file on disk, so you now have two sources of truth for the same constraints.

All of these problems come from the same missing concept: **a graph whose role is schema, not data, and a place to say what that schema applies to.**

## How stores solve it today, and why that isn't enough

Every serious store has some answer. Almost all of them keep it in configuration:

- **Stardog** has *reasoning schemas*: a named set of graphs, chosen per query and defined in database configuration.
- **Virtuoso** builds *inference rule sets* from a graph. They live in SQL and are selected per query with `input:inference`.
- **GraphDB and RDF4J** attach a ruleset to a repository and keep SHACL shapes in a reserved graph. It is all repository configuration.
- **Apache Jena** does it in code, with an `ont-policy` file for imports. That file is RDF, but it sits outside the data.
- **Neo4j neosemantics** stores graph configuration as node properties.

Each of these works well within its own store. They all have the same weakness: **the knowledge "this ontology describes that data" is not part of your dataset.** Dump the data, load it somewhere else, and the mapping stays behind. You can't query it with SPARQL, version it with the data, or validate it. And you can't hand it to a colleague who uses a different store.

The mapping is metadata about the data, and in RDF we already know where metadata about data belongs: in the data.

## The idea: the registry is RDF

oxilite keeps schema registrations as ordinary triples in a well-known named graph, `<oxilite:schema>`. Each registered graph is described there, with the graph's own IRI as the subject:

```turtle
PREFIX oxl: <https://oxilite.dev/ns#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

GRAPH <oxilite:schema> {
  <https://ex.org/onto/hr> a oxl:OntologyGraph ;
      oxl:appliesTo <https://ex.org/data/staff> , <https://ex.org/data/contractors> ;
      oxl:active true ;
      oxl:version "2.1" ;
      oxl:ontologyIri <https://ex.org/hr#> ;
      oxl:imports <http://xmlns.com/foaf/0.1/> ;
      oxl:sha256 "9f2c…" ;
      oxl:loadedAt "2026-09-26T10:00:00Z"^^xsd:dateTime .

  <https://ex.org/shapes/hr> a oxl:ShapesGraph ;
      oxl:appliesTo oxl:AllGraphs ;
      oxl:active true .
}
```

Read it out loud and it answers every question from the first section:

- **What it is.** `<https://ex.org/onto/hr>` is an ontology graph (`oxl:OntologyGraph`). Other roles are `oxl:ShapesGraph` for SHACL and `oxl:ShExGraph` for ShEx.
- **What it describes.** The staff and contractors graphs, and nothing else.
- **Whether it counts.** `oxl:active true`. Set it to `false` and the graph stays registered but contributes nothing.
- **Which version, and which file.** A pinned version string and the SHA-256 of the source document, for drift detection.
- **When.** `oxl:loadedAt`.
- **What it builds on.** Imports of other registered ontologies.

Two properties of this design matter more than any single field.

**Registering moves nothing.** The ontology's triples stay in its own graph, `<https://ex.org/onto/hr>`, and remain ordinary RDF that SPARQL can read and update. The registry only *labels* the graph. We considered separate `ontology_quads` and `shape_quads` tables and rejected them. Shapes are RDF that people query, a second table would force a `UNION` into every pattern scan, and named graphs already give you the separation for free.

**The registry is part of the dataset.** It shows up in `GRAPH ?g`. It's in your dumps and your backups, and in your history if the store is versioned. When you move the data, the schema mapping comes along.

## It works on any RDF store

This is the part I care about most. The registry is not an oxilite feature that happens to be written in RDF. It is **RDF that oxilite happens to act on.**

Every operation oxilite performs on the registry is a plain SPARQL 1.1 update or query. These are exactly the statements it runs internally, and they run unchanged on Oxigraph, Jena Fuseki, GraphDB, Virtuoso, Stardog, or any other store that speaks SPARQL 1.1.

**Register an ontology for two data graphs**, replacing any previous description:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
CREATE SILENT GRAPH <https://ex.org/onto/hr> ;
DELETE WHERE { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> ?p ?o } } ;
INSERT DATA { GRAPH <oxilite:schema> {
  <https://ex.org/onto/hr> a oxl:OntologyGraph ; oxl:active true ;
      oxl:appliesTo <https://ex.org/data/staff> , <https://ex.org/data/contractors> .
} }
```

**Re-map it** to every graph, keeping its roles and everything else:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
DELETE { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> oxl:appliesTo ?t } }
INSERT { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> oxl:appliesTo oxl:AllGraphs } }
WHERE  { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> a ?role OPTIONAL { <https://ex.org/onto/hr> oxl:appliesTo ?t } } }
```

**Switch it off** without forgetting it:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
DELETE { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> oxl:active ?a } }
INSERT { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> oxl:active false } }
WHERE  { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> a ?role OPTIONAL { <https://ex.org/onto/hr> oxl:active ?a } } }
```

**Unregister it** (its triples stay), or drop it together with its triples:

```sparql
DELETE WHERE { GRAPH <oxilite:schema> { <https://ex.org/onto/hr> ?p ?o } } ;
DROP SILENT GRAPH <https://ex.org/onto/hr>
```

**Ask which active ontologies apply to a data graph.** This is the query every reasoner needs:

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

**Query your data without the schema**, on a store that has no way to hide graphs:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
SELECT ?s ?p ?o WHERE {
  GRAPH ?g { ?s ?p ?o }
  FILTER (?g NOT IN (<oxilite:schema>, <oxilite:vocabulary>))
  FILTER NOT EXISTS { GRAPH <oxilite:schema> { ?g a ?role } }
}
```

So a team can:

- keep a registry in a GraphDB repository for years and never run oxilite;
- use the recipes in a Jena pipeline to pick which ontologies to feed its reasoner;
- dump a dataset from oxilite, load it into Oxigraph, and still know exactly which schema describes which data.

To be precise about the limits: on another store, the registry is *data plus recipes*. The store won't scope its own reasoning by it unless you wire that up yourself. What carries over is the description and every operation on it, so nothing you record is tied to oxilite.

## One store, conflicting ontologies

The `oxl:appliesTo` property makes something possible that most stores can't do: **two datasets with conflicting ontologies in the same store.**

```turtle
GRAPH <oxilite:schema> {
  <https://ex.org/onto/zoo>    a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/zoo> .
  <https://ex.org/onto/garden> a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/garden> .
  <https://ex.org/onto/common> a oxl:OntologyGraph ; oxl:appliesTo oxl:AllGraphs .
}
```

With RDFS reasoning on in oxilite:

- an `ex:Dog` in the zoo graph is reasoned over with the zoo ontology and the common one;
- an `ex:Dog` in the garden graph is reasoned over with the garden ontology and the common one.

This holds **even in a single query over the union of both graphs.** Entailment is per graph, like a context. The zoo's idea of what a `Dog` is never leaks into the garden.

The targets are simple:

- `oxl:AllGraphs`, or no `oxl:appliesTo` at all: the schema applies everywhere.
- One or more graph IRIs, or `oxl:DefaultGraph`: only those graphs. If `oxl:AllGraphs` appears among them, it wins.

Internally, oxilite computes one TBox closure for "all graphs" plus one for each graph that has ontologies of its own. The query rewriting picks the closure by the quad's graph. No extra round trips are needed. On Cloudflare D1, where every round trip costs milliseconds, that matters.

## Imports, without the internet

OWL has `owl:imports`, and every OWL user has seen a tool freeze because it tried to fetch half the web.

oxilite resolves imports **only against ontologies you have registered**:

- When an active ontology imports another registered, active ontology, the imported axioms join the importer's reasoning scopes. This is transitive, and cycles are safe.
- An import can name the other ontology's graph or its `oxl:ontologyIri`.
- An import that matches nothing registered is recorded and ignored. **Nothing is ever fetched.**

```turtle
GRAPH <oxilite:schema> {
  <https://ex.org/onto/zoo>  a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/zoo> .
  <https://ex.org/onto/core> a oxl:OntologyGraph ; oxl:appliesTo <https://ex.org/data/other> ;
      oxl:ontologyIri <https://ex.org/core#> .
}
GRAPH <https://ex.org/onto/zoo> { <https://ex.org/zoo#> owl:imports <https://ex.org/core#> . }
```

The core axioms now also apply to the zoo data, because the zoo ontology imports them.

Why doesn't the registry itself use `owl:imports`? Its domain is `owl:Ontology`. If you wrote it on registrations, an OWL reasoner would conclude that every registered graph, shapes graphs included, is an ontology. And any OWL tool loading `<oxilite:schema>` would try to fetch every import. So the registry records `oxl:imports`, while `owl:imports` written *inside* the ontology's own graph is still honoured, as OWL intends.

## A registry that validates itself

A configuration you can't check is a configuration that silently rots. Because the registry is RDF, it can be checked the way RDF is checked: **with SHACL.**

The `oxl:` vocabulary ships with `oxl:RegistrationShape`. It requires that:

- every registration is an IRI with at least one role class;
- `oxl:active` has at most one value, and it is an `xsd:boolean`;
- `oxl:appliesTo`, `oxl:ontologyIri` and the imports are IRIs;
- `oxl:version`, `oxl:sha256` and `oxl:loadedAt` each have at most one value of the right datatype;
- `oxl:sha256` is exactly 64 lowercase hex digits.

The shape targets every role class *and* every subject of a registration property. A description that forgot its role is caught too.

You can check a registry with **any SHACL processor**: use [`oxl.ttl`](https://oxilitedb.com/ns/oxl.ttl) as the shapes graph and `<oxilite:schema>` as the data graph. oxilite also has a built-in check that needs no SHACL engine:

```bash
oxilite registry check -l hr.db
```

It prints one line per problem and exits non-zero when it finds any, so it fits in CI.

**Readers are lenient.** A value that breaks the shapes is ignored, never guessed. A plain string `"false"` for `oxl:active` doesn't deactivate anything, because it isn't a boolean. Only the two lexical forms of an `xsd:boolean` false (`false` and `"0"^^xsd:boolean`) do. The Rust reader, the SQL that scopes the caches and the portable SPARQL recipes all follow the same rule, and tests keep them in agreement.

**Drift detection** closes the loop between the store and your repository:

```bash
oxilite registry register https://ex.org/onto/hr --role ontology \
    --file ontologies/hr.ttl --version 2.1 \
    --applies-to https://ex.org/data/staff --applies-to https://ex.org/data/contractors \
    -l hr.db

# later, in CI
oxilite registry verify https://ex.org/onto/hr --file ontologies/hr.ttl -l hr.db
```

`register --file` loads the file into the graph and records its SHA-256. `verify` compares the file you have now against the recorded digest and exits non-zero on drift. "Is production running the ontology in main?" becomes a one-line check.

## Standards, not a private dialect

A new vocabulary is only worth having if it plays well with the ones you already use. `oxl:` is small on purpose and links to the standard vocabularies instead of duplicating them:

- **SHACL.** `sh:shapesGraph` links a data graph to its shapes, the reverse direction of `oxl:appliesTo`. We don't store both, because two copies of one fact drift apart. A SHACL tool that wants `sh:shapesGraph` can get it with one `CONSTRUCT`:

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

- **DCAT / Dublin Core.** `?data dcterms:conformsTo ?schema` is also the reverse of `oxl:appliesTo`, and it is derived the same way. A registry can therefore feed a DCAT catalogue directly.
- **SPARQL Service Description.** `oxl:RegisteredGraph` is a subclass of `sd:Graph`. The registration's subject is the graph itself, not the name–graph pair.
- **PROV and Dublin Core.** `oxl:loadedAt` is a sub-property of `dcterms:date` and points to `prov:generatedAtTime`.
- **OWL and SPDX.** `oxl:version` points to `owl:versionInfo` and `owl:versionIRI`, which stay in the ontology's own graph. `oxl:sha256` points to `spdx:checksum`.

The vocabulary itself has `owl:versionIRI`, `owl:priorVersion`, `owl:backwardCompatibleWith`, a licence, a preferred prefix, and `rdfs:isDefinedBy` on every term. It is **consistent under OWL**, together with the descriptions of the system graphs, and a test keeps it that way. We found and fixed exactly such an inconsistency on the way to 2.1: the system graphs were described with properties whose domain was `oxl:SchemaGraph`, which contradicted their disjointness. Now the registration properties have the domain `oxl:RegisteredGraph`, the disjoint union of schema graphs and system graphs.

## What oxilite does with the registry

Everything above is portable. This is what you get when the store *also* acts on the registry.

**Scoped reasoning.** Query-time RDFS and OWL-QL reasoning uses only active ontology graphs and the active ontologies they import, each applied to the graphs it targets. Deactivate an ontology and its entailments disappear immediately, with no data reloaded. Reactivate it and they come back.

**A compiled shape index.** The SHACL property shapes of the active shapes graphs are compiled into an index covering target class, path, datatype, cardinalities, patterns and `sh:in` values. Cypher plans with it and checks writes against it, in one indexed request instead of a SPARQL evaluation per write. The validator can use the shapes held in the store, so there is one source of truth.

**Hiding.** Pass `include_schema_graphs: false` and every registered graph and system graph drops out of pattern matching. That covers the default graph, `GRAPH ?g`, property paths and `OPTIONAL`. Your data queries return data.

**Change detection in the same atomic request.** A write that can change the registry rebuilds the reasoning closure and the shape index *inside the same atomic request*. That includes a hand-written `INSERT DATA` into `<oxilite:schema>`. There is no rebuild step and no window where the store is stale. Bulk writes like `INSERT { GRAPH ?g { ?s a ex:Person } } WHERE …` are recognised as unable to touch the registry and rebuild nothing.

**A safe default.** While nothing is registered, every graph counts, exactly as before the registry existed. Registering your first ontology is what narrows the scope. Upgrading never changes your results until you opt in.

The same operations are available on every surface:

| Surface | Register | Other |
|---|---|---|
| Rust | `register_schema_graph(graph, role, &Registration::new().applies_to([...]))` | `set_schema_graph_targets`, `set_schema_graph_active`, `drop_schema_graph`, `registry_problems` |
| Node, Cloudflare D1 | `registerSchemaGraph(graph, "ontology", { appliesTo: [...] })` | `setSchemaGraphActive`, `dropSchemaGraph`, `shapeIndex` |
| CLI | `oxilite registry register GRAPH --role ontology --applies-to G` | `map`, `activate`, `deactivate`, `drop`, `check`, `verify` |
| Shell | `.register ontology GRAPH FILE` | `.map`, `.deactivate`, `.registry` |
| Studio | `[[graph]] role = "ontology"` in `oxilite.toml` | `applies_to = [...]` |

Every one of them runs the same SPARQL recipes shown earlier.

## System graphs: the registry describes itself

A registry should be self-describing, so oxilite maintains two **system graphs**:

- `<oxilite:schema>`: the registry.
- `<oxilite:vocabulary>`: the `oxl:` vocabulary and its shapes, registered as applying to `<oxilite:schema>`.

A new store can start with both installed. The registry then contains its own description: the vocabulary graph is an ontology whose shapes apply to the registry graph. Load a dataset anywhere and the rules for reading its registry are right there in the data.

These are opt-in in the libraries (`systemGraphs: true`), so a new store is empty by default, exactly as in Oxigraph. The CLI and shell install them when they create a database. For an existing store, `oxilite registry init` installs or refreshes them. The SPARQL behind it, `registry::system_graphs_update()`, runs on any store too.

## How it compares

| | Configuration kept as | Scope per data graph | Portable to another store | Validates its own configuration |
|---|---|---|---|---|
| **oxilite (`oxl:` 2.1)** | RDF in the dataset | yes, for reasoning | yes, plain SPARQL 1.1 | yes, SHACL shapes |
| Stardog reasoning schemas | database configuration | no | no | no |
| Virtuoso inference rule sets | SQL | no | no | no |
| GraphDB / RDF4J | repository configuration | no | partly | partly |
| Apache Jena | code, plus an RDF file outside the data | in code | partly | no |
| Neo4j neosemantics | node properties | no | no | partly |
| Nanopublications | RDF in the dataset | not applicable | yes | through shapes |

Other stores have some features oxilite doesn't. Stardog and Virtuoso let you pick the schema **per query**, and oxilite doesn't do that yet. Nanopublications have the strongest provenance model of any of these. The difference oxilite is betting on is the first column: **where the configuration lives.**

## Honest limits

- **The shape index is keyed by class, not by graph.** A shapes graph mapped to one graph still constrains Cypher writes everywhere. Validators can already pick shapes per graph, and a per-graph index is planned.
- **Mappings drive query-time reasoning, not `materialize()`.** OWL 2 RL materialization still reasons over the merged dataset.
- **One description per graph.** A graph with two roles (an ontology that carries its own shapes) shares one set of targets and one active flag. Separate registrations per role are a candidate for vocabulary 3.
- **ShEx graphs are recorded and hidden, not compiled.**
- **Graphs named by blank nodes can't be registered**, because no other graph can refer to them.
- **The namespace doesn't resolve yet.** `https://oxilite.dev/ns#` is the term namespace, while the vocabulary is served at [oxilitedb.com/ns](https://oxilitedb.com/ns/). The `oxilite:` scheme of the system graphs isn't a registered URI scheme. Both are kept for compatibility until a 1.0 decision.

## Try it

```bash
cargo install oxilite-cli

oxilite registry register https://ex.org/onto/zoo --role ontology \
    --file zoo.ttl --applies-to https://ex.org/data/zoo -l zoo.db
oxilite registry list -l zoo.db
oxilite registry check -l zoo.db
oxilite query -l zoo.db --reasoning rdfs --no-schema-graphs \
    'SELECT ?x WHERE { GRAPH ?g { ?x a <https://ex.org/zoo#Animal> } }'
```

Or skip oxilite completely: open your existing store, paste the registration recipe into its SPARQL console, and start describing your graphs. The vocabulary is at [oxilitedb.com/ns](https://oxilitedb.com/ns/), and the full reference, with every recipe, is [`docs/schema-registry.md`](https://github.com/Volland/oxilite/blob/main/docs/schema-registry.md).

---

**The takeaway.** An ontology shouldn't disappear into the data it describes, and the fact that it describes that data shouldn't be locked in one vendor's configuration. Put that fact where RDF puts every other fact: in a graph, in a published vocabulary, checked by SHACL and managed with SPARQL. Then it travels with your dataset to any store that speaks SPARQL 1.1. oxilite reads the same triples, scopes reasoning per graph, compiles your shapes and hides your schema from data queries.

Issues, disagreements, and registries written for other stores are all welcome at [github.com/Volland/oxilite](https://github.com/Volland/oxilite).

---

*Substack tags:* RDF · Semantic Web · Knowledge Graphs · SPARQL · Ontology · SHACL · OWL · Linked Data · Data Engineering · Open Source

*For sharing:* #RDF #SemanticWeb #KnowledgeGraph #SPARQL #OWL #SHACL #Ontology #LinkedData #DataGovernance #Oxigraph #SQLite #OpenSource
