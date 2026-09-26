## MODIFIED Requirements

### Requirement: Schema graphs are registered by role
The system SHALL record registrations as RDF in the named graph `<oxilite:schema>`, using the
`oxl:` vocabulary (`https://oxilite.dev/ns#`).
- Each registered graph SHALL be a resource of that graph, typed `oxl:OntologyGraph`,
  `oxl:ShapesGraph` or `oxl:ShExGraph`, described by `oxl:active`, `oxl:appliesTo`,
  `oxl:ontologyIri`, `oxl:version`, `oxl:sha256`, `oxl:loadedAt` and `owl:imports`.
- The default graph SHALL be named `oxl:DefaultGraph`.
- The system SHALL list, activate, deactivate, unregister and drop registrations.
- Registering MUST NOT move, copy or rewrite any triple of the registered graph.
- No relational table SHALL hold registrations.

#### Scenario: Register and list
- **WHEN** a graph holding an ontology is registered with role `ontology` and version `v1`
- **THEN** listing the registry returns that graph with role `ontology`, version `v1` and active
- **AND** `ASK { GRAPH <oxilite:schema> { <that graph> a oxl:OntologyGraph ; oxl:version "v1" } }` is true

#### Scenario: Triples stay queryable
- **WHEN** a graph is registered as a shapes graph
- **THEN** `SELECT ?s ?p ?o WHERE { GRAPH <that graph> { ?s ?p ?o } }` still returns its triples

#### Scenario: Unregister keeps the data
- **WHEN** a registered graph is unregistered
- **THEN** the registry no longer lists it, `<oxilite:schema>` holds no triple about it, and its triples are still in the store

#### Scenario: Dropping a schema graph
- **WHEN** a registered graph is dropped through the registry
- **THEN** both its registration and every one of its quads are gone

#### Scenario: Registering by SPARQL
- **WHEN** `INSERT DATA { GRAPH <oxilite:schema> { <g> a oxl:OntologyGraph } }` runs as a plain update
- **THEN** the registry lists `<g>` as an ontology and reasoning is scoped to it, as if the API had registered it

## ADDED Requirements

### Requirement: Schema graphs map to the graphs they describe
A registration SHALL name the graphs it applies to with `oxl:appliesTo`: named graphs,
`oxl:DefaultGraph` or `oxl:AllGraphs`. A registration without `oxl:appliesTo` SHALL apply to
all graphs. Query-time reasoning SHALL entail a triple from a quad of graph G only through the
active ontologies that apply to G. `Store::schema_graphs_for(G, role)` SHALL return the active
schema graphs of a role that apply to G.

#### Scenario: Ontology mapped to one graph
- **WHEN** ontology O1 (`ex:Dog rdfs:subClassOf ex:Animal`) applies to graph A, ontology O2 (`ex:Dog rdfs:subClassOf ex:Plant`) applies to graph B, `ex:rex a ex:Dog` is in A and `ex:fido a ex:Dog` is in B, and a query over the union asks for animals and plants with RDFS reasoning
- **THEN** `ex:rex` is an animal and not a plant, and `ex:fido` is a plant and not an animal

#### Scenario: Global ontologies apply everywhere
- **WHEN** a global ontology declares `ex:Animal rdfs:subClassOf ex:Being` and O1 above applies to A
- **THEN** `ex:rex` is also an `ex:Being`

### Requirement: The registry is portable SPARQL
The system SHALL generate every registry operation as SPARQL 1.1 text over `<oxilite:schema>`,
and SHALL read the registry with a SPARQL query. The same text SHALL run on Oxigraph and
produce the same registry graph. The vocabulary SHALL be published as Turtle.

#### Scenario: Round trip through Oxigraph
- **WHEN** the registration update for a graph runs on an Oxigraph store and the registry query's results are parsed
- **THEN** the parsed entry equals the registration

### Requirement: A blank store can start with the system graphs
The system SHALL offer a store option that, when a store is opened holding no quad, writes the
`oxl:` vocabulary into `<oxilite:vocabulary>` and describes `<oxilite:schema>` and
`<oxilite:vocabulary>` as `oxl:SystemGraph` in the registry (with the vocabulary's version).
The option SHALL be off by default in the libraries and on for stores the command line creates.
An existing store SHALL get them on request, idempotently, through portable SPARQL. System graphs
MUST NOT be listed as registrations, narrow reasoning or the shape index, and SHALL be hidden with
the schema graphs.

#### Scenario: New store from the command line
- **WHEN** `oxilite update -l new.sqlite -u 'INSERT DATA { … }'` creates a store
- **THEN** `ASK { GRAPH <oxilite:vocabulary> { oxl:appliesTo ?p ?o } }` is true and `registry list` shows no registration

#### Scenario: Library default is empty
- **WHEN** a store is created with default options
- **THEN** it holds no quad

### Requirement: Registry of released stores is migrated
Opening a store created with schema version 1 SHALL convert its `schema_graphs` rows into
registry triples, drop the table, rebuild `tbox_closure` with its scope column, and record
schema version 2.

#### Scenario: Old store opens
- **WHEN** a version 1 store with one registered ontology is opened
- **THEN** the registry lists that ontology and the table `schema_graphs` no longer exists

### Requirement: Registry from the command line
The `oxilite` binary SHALL manage the registry:
- `registry list` (`--json`);
- `registry register GRAPH --role ontology|shacl|shex`, with `--file`, `--iri`, `--version`,
  `--import`, `--applies-to` and `--inactive`;
- `registry map GRAPH --to TARGET…`;
- `registry activate|deactivate|unregister|drop GRAPH`;
- `registry shapes`.

`query`, `explain` and `serve` SHALL accept `--reasoning`, `--inferred` and
`--no-schema-graphs`, and `oxilite materialize` SHALL run (or, with `--clear`, remove) OWL 2 RL
inferences.

#### Scenario: Register an ontology from a file
- **WHEN** `oxilite registry register http://ex.org/onto --role ontology --file onto.ttl` runs
- **THEN** `registry list --json` shows the graph with role `ontology` and the file's SHA-256
- **AND** the graph holds the file's triples
