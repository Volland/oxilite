---
lat:
  require-code-mention: true
---
# Tests

Test specifications that are implemented in code; each leaf is referenced by exactly one `@lat:` comment next to its test. Planned tests live in [[test-plan]].

## Encoding

Unit tests of the tagged 64-bit term encoding described in [[architecture#Term encoding]].

### Inline integers sort by value

Canonical integers in ±2^58 encode inline, ids are positive and ordered like the values, decode back exactly, and out-of-range values are not inlined.

### Non-canonical literals are hashed

`"012"^^xsd:integer` is a different term from `"12"^^xsd:integer`: it is hashed (Typed tag) with its numeric value 12 in the side columns, preserving term identity.

### Simple literal equals xsd string

A simple literal and the same lexical form typed `xsd:string` get the same id (RDF 1.1), while a language-tagged literal with that form gets a different one.

### Tags partition the id space

An IRI and a blank node with the same text get different ids, and each id falls in the range reserved for its tag, so kind checks are integer range checks.

## Write path

Unit tests of statement generation for inserts, see [[architecture#Write path]].

### Statements respect the size limit

Inserting 500 quads under a 2 000-byte statement limit yields several statements, none longer than the limit and none using bound parameters.

## Planner

Unit tests of the greedy join-order planner, see [[architecture#Query planner]].

### Rare predicate first

With statistics showing `ex:rare` has 10 triples and `rdf:type ex:Common` 1 000, the `ex:rare` pattern is ordered first.

### Connected patterns are preferred

In a chain of patterns, every pattern after the first shares a variable with an earlier one, so no Cartesian product is introduced.

### Frequent values are not selective

A (predicate, object) pair recorded in `stats_po` as far more frequent than its predicate's average makes the planner start from a rarer pattern instead.

### BSBM plans use indexes

Every BSBM explore and business-intelligence query template, on a small BSBM-shaped dataset, compiles fully to SQL under D1's 90 KB, runs, and its `EXPLAIN QUERY PLAN` scans no quad table or index.

### Heuristics without statistics

Without statistics, a pattern with a constant subject is ordered before one with only a constant predicate.

## Backends

Tests of the native backends, see [[architecture#Backends]].

### Dylib loads a system SQLite

A system `libsqlite3` found at a well-known path is loaded at runtime; values of every storage class round-trip and a failing atomic request rolls back.

### Invalid library is reported

Opening the dylib backend with a nonexistent library path fails with an error naming the library.

## Store

Integration tests of the blocking store against the M1 specifications.

### Queries are single statements

A five-pattern BGP with a filter uses at most two backend requests (the query plus one term-resolution round-trip), and `explain()` reports it fully compiled.

### Collision aborts the batch

A forged dictionary row that collides with a term being inserted makes `extend` fail with a collision error, and none of the batch's quads are stored.

### Reopen keeps data

Quads and named graphs written to a database file are still present after reopening it, and `validate()` passes.

### Graph index is optional

A store created with `graph_index: false` has only the `posg` and `ospg` secondary indexes on `quads`.

### Insert and remove report changes

Inserting an existing quad or removing an absent one reports `false`; the non-canonical literal `"012"^^xsd:integer` round-trips exactly.

### Pattern scans match a naive filter

For all 16 combinations of bound and unbound positions, `quads_for_pattern` returns exactly the quads a naive filter selects.

### Planner uses statistics

After `optimize()`, the generated SQL scans the rare predicate before the `rdf:type` pattern, and results are unchanged.

### Union default graph deduplicates

With the union-default-graph option a triple present in two named graphs matches once, while `GRAPH ?g` still returns both graphs.

### Dylib store works end to end

A store opened on a system `libsqlite3` through `Store::open_with_library` loads Turtle and answers an aggregate query with a filter.

## Oxigraph compatibility

Suites of the compatibility harness, see [[test-plan#Oxigraph compatibility harness]].

### Ported Oxigraph store API tests

Oxigraph 0.5.11 `lib/oxigraph/tests/store.rs`, copied with only `oxigraph::` replaced by `oxilite::`, passes against `oxilite::store::Store`; RocksDB-only tests are compiled out by their feature gates.

### Differential corpus matches Oxigraph

About 120 query families over a seeded dataset give the same results on Oxigraph and on every oxilite variant, for two seeds; divergences must be allow-listed.

The dataset covers every literal kind, blank nodes, named graphs with cross-graph duplicates, a class hierarchy, cycles and triple terms.

### Optimizer regression query

Oxigraph's OPTIONAL-on-foreign-key regression (20 persons × 20 orders) returns Oxigraph's results, and the OPTIONAL's BGP is planned starting from the foreign-key lookup on the bound `?c`.

### Update corpus matches Oxigraph

Every SPARQL UPDATE in the differential update corpus, applied to the seeded dataset, leaves oxilite and Oxigraph with the same quads; divergences must be allow-listed.

## D1

`@oxilite/d1` tests running the wasm core against a Miniflare D1 database, see [[architecture#Backends#Cloudflare D1]].

### Store API on Miniflare D1

`add`, `has`, `delete`, `size` and `match` on `D1Store` behave like Oxigraph's JavaScript store API against a real D1 binding.

### Ids above 2^53 survive D1

Fifty hashed terms (60-bit ids) round-trip through D1 and JavaScript intact, proving ids travel as TEXT and no precision is lost above 2^53.

### Failed update leaves D1 unchanged

An update whose later operation fails (`CREATE GRAPH` on an existing graph) aborts its whole batch, so the earlier `INSERT DATA` is not visible.

### Large loads respect D1 limits

A 6000-triple bulk load is split into batches under D1's statement limits, all triples arrive, and statistics are refreshed for `explain()`.

### Cypher runs on D1

Through the wasm core on Miniflare D1, `cypher()` creates a chain per row, finds its shortest path by breadth-first search, counts with `OPTIONAL MATCH`, detaches a node, and rejects deleting a connected node.

### Reasoning on D1

RDFS subclass chains, an OWL transitive property and SQL-rule materialization (one D1 batch per round) work on a Miniflare D1 database.

### SQL values survive arbitrary-precision JSON

`SqlValue` deserializes numbers both as plain JSON and in the map form serde_json uses when `arbitrary_precision` is enabled (as the `ssi` crates do), so D1 responses decode in builds with `vc`.

### JSON-LD documents on D1

`@oxilite/d1` stores, replaces and removes JSON-LD documents on Miniflare D1, loads persisted contexts, reports JSON-LD error codes, and includes the JSON-LD tables in the migration schema.

### Credentials on D1

`@oxilite/d1` stores credentials with the bundled W3C contexts, stores a presentation's embedded credentials, finds them by issuer and validity, and rejects invalid credentials.

### System graphs on D1

`installSystemGraphs()` installs the vocabulary and registry description on D1 once, and the schema script with `systemGraphs` carries their inserts.

### Schema registry on D1

On Miniflare D1 the registry's portable SPARQL registers, maps, lists, deactivates and drops schema graphs; a mapping narrows entailments and hiding excludes the registered graphs.

## Reasoning

Query-time RDFS / OWL QL rewriting and OWL 2 RL materialization, each test run on the bundled SQLite and on the system `libsqlite3`, see [[architecture#Reasoning]].

### Default has no inference

A subclass axiom and an instance give no entailed type without reasoning, and the entailed type with `Reasoning::Rdfs`.

### Transitive subclass chain

A ⊑ B ⊑ C types instances of A as C; SPARQL updates that add or delete schema triples change query answers immediately, and `rdfs:subClassOf` patterns are answered from the closure.

### Reasoning keeps single statements

A reasoned BGP compiles fully to one SQL statement that reads `tbox_closure`.

### Materialize then query

`materialize()` derives facts through `owl:sameAs`, subclasses and inverse properties; they are visible only with `include_inferred`, re-running replaces them, and `clear_inferences()` removes them.

### Agreement with reasonable

On five sample ontologies (RDFS, property axioms, equality, class expressions with lists and chains, schema cycles) the SQL rules and the `reasonable` crate produce exactly the same closure.

## Schema registry

Ontology and shapes graphs declared in the registry graph `<oxilite:schema>`, their mapping to data graphs, the scoping that follows, and the compiled shape index, see [[architecture#Schema registry]].

### Registration round-trip

A graph is registered with role, IRI, version and imports, listed back from `<oxilite:schema>`, and unregistered; registering and unregistering leave its triples untouched and queryable.

### Reasoning scoped to registered ontologies

With nothing registered every graph contributes axioms; once two ontology graphs are registered, deactivating one withdraws exactly its entailments and reactivating restores them, with no reload of data.

### Axioms outside registered ontologies

A subClassOf axiom in a graph that is not registered as an ontology entails nothing once another ontology graph has been registered.

### Hiding schema graphs

`include_schema_graphs: false` removes the registered graphs' triples from pattern matching and changes nothing while no graph is registered; hiding applies to an inactive registration too.

### Dropping a schema graph

Dropping a registered graph removes its registration and its quads in one request, leaves the rest of the dataset intact, and withdraws its entailments.

### Compiled shape index

Shapes written into the store are compiled immediately, with no `optimize()` in between: datatype, `sh:minCount`, `sh:maxCount`, `sh:pattern`, `sh:in` values and relationship-valued shapes all come back from the index.

### Shape index follows deletions

Registering a shapes graph narrows the index to it, and deleting the shape triples empties the index.

### Shapes merged across shapes

Two shapes targeting the same class and path, one declaring a cardinality and the other a datatype, merge into one entry carrying both.

### Registry survives reopening

A store reopened from disk still lists its registrations and still reasons under them.

### Ontologies apply to the graphs they are mapped to

Two conflicting ontologies mapped to two data graphs entail only for their own graph's quads; a global ontology adds to both, remapping to every graph widens the entailments, and `schema_graphs_for` lists what applies to a graph.

### The registry is RDF in the schema graph

A plain SPARQL `INSERT DATA` into `<oxilite:schema>` registers a graph as the API does; the API's registration, mapping and deactivation read back with SPARQL, and hiding schema graphs hides the registry graph too.

### A version 1 registry is migrated

A store with the old `schema_graphs` table and unscoped `tbox_closure` opens with its registration as registry triples, reasoning scoped by it, the table gone and schema version 2 recorded.

### Registry SPARQL runs on Oxigraph

The core's registry SPARQL runs unchanged on Oxigraph, reads back the same registration, and leaves the same `<oxilite:schema>` graph as oxilite.

The system-graph install on Oxigraph leaves the same quads as a bootstrapped blank oxilite store, and the vocabulary parses as Turtle.

### A blank store starts with the system graphs

With `system_graphs`, a new store holds `<oxilite:schema>` and `<oxilite:vocabulary>`; they are current, unlisted, do not narrow reasoning and hide with the schema, and a store with data gets them only on request.

### Imports bring registered ontologies into scope

An ontology importing another registered one — recorded in the registry by graph name, or asserted in its graph by `oxl:ontologyIri` — gets its axioms; cycles end, and inactive ontologies are not imported.

Deactivating every ontology then silences reasoning instead of falling back to every graph.

### The active flag reads alike everywhere

A plain `"false"` leaves a graph active for the listing and for reasoning and is reported as a problem; `"0"^^xsd:boolean` deactivates it for both and is valid.

### Remapping keeps the description

A graph typed with two roles is listed once per role; setting its targets keeps both roles and its version, and no targets are written as `oxl:AllGraphs`.

### Readers agree on edge cases

Unit test of the reader: two role classes give two entries, only `xsd:boolean` false deactivates, and the SQL reads both lexical forms of false.

### Registry problems

Unit test of `problems`: a non-boolean flag, a duplicate flag, a literal target, a malformed digest and a description with no role class are each reported once.

### Registry writes are detected narrowly

Bulk typing and plain data updates through `GRAPH ?g` do not count as registry writes; `oxl:` classes, variable classes, `owl:imports` and anything in `<oxilite:schema>` do.

## Text search

FTS5 full-text search and `oxl:textMatch`, see [[architecture#Text search]].

### Word match

With the text index, word, multi-word and prefix queries (including across diacritics) find the right literals on bundled and system SQLite, compile to an FTS5 `MATCH`, and survive `clear()` and re-insertion.

### Same answers without the index

Without the text index the same queries give the same answers through the fallback evaluator.

### Disabled by default

A default store has no full-text table and its schema script creates none.

### Text search on D1

A D1 store opened with `textIndex: true` answers `textMatch` through FTS5 on Miniflare.

## Validation

SHACL and ShEx with rudof over oxilite stores (`oxilite-validate`), see [[architecture#Validation]].

### SHACL violation reported

A `sh:minCount 1` on `ex:name`, a nested `sh:node` and a `sh:targetObjectsOf` target report the failing focus nodes, in native and SPARQL modes.

### SHACL suite matches rudof in memory

Every W3C SHACL core test (226 reports, 178 with violations) gives the same report over an oxilite store as over rudof's in-memory graph, on bundled SQLite and the system `libsqlite3`, in both validation modes.

### ShEx conforming node

A node with the required properties conforms to its ShEx shape through a shape map; a node that points to a non-conforming node does not.

### ShEx suite matches rudof in memory

The 1161 shexTest validation tests that rudof can parse give the same statuses over an oxilite store as in memory.

### Shapes read from the store

Validating against a registered shapes graph gives the same report as passing the same shapes as text, and naming the graph explicitly gives that report too.

### Stored shapes need an unambiguous graph

With no shapes graph registered, and with more than one registered and none named, validation fails with an error naming the problem instead of guessing.

### Bounded prefetch matches full validation

On a D1-like async backend (and the Miniflare D1 sidecar with `OXILITE_D1_URL`), prefetch-based SHACL and ShEx validation match full validation, and a too-small limit fails with `TooLarge` stating the limit and size found.


### The registry shapes check a registry

rudof validates a registry against the shapes in the bundled vocabulary: a canonical registry with a two-role graph conforms, and a plain `"false"`, a short digest and a roleless description are the three focus nodes reported.
## JSON-LD documents

Specification scenarios of `oxilite-jsonld`, run on bundled SQLite, the system `libsqlite3`, the D1 code path and (with `OXILITE_D1_URL`) Miniflare D1. See [[architecture#JSON-LD documents]].

### Documents round-trip verbatim

A stored document comes back byte for byte (key order, whitespace, escapes), with its SHA-256, its graph and a store time; an unknown key reads as absent.

### Key strategies

The default key is the top-level id; a JSON Pointer or explicit key can replace it; a missing key is rejected by default, and the content-hash fallback is stable, so storing twice keeps one document.

### Graph strategies

A template graph substitutes the percent-encoded key; a key that is not an IRI is refused as a graph name; the default-graph strategy records the default graph as the target.

### Invalid JSON-LD is rejected atomically

Invalid JSON and invalid JSON-LD fail with distinct errors (the latter with its JSON-LD error code) and write neither the document nor any quad.

### Graph containers are owned by the document

A property declared `@container: @graph` puts its triples in a blank-node graph listed among the document's graphs; removing the document removes both graphs and their names.

### Blank nodes are document-scoped

Two documents with the same anonymous node yield two distinct blank nodes, and re-storing a document leaves the quad count unchanged.

### Replace is atomic

Replacing a document drops its old triples; a replace that fails (its graph is owned by another document) leaves the previous version, row and triples, untouched.

### Remove clears owned graphs only

Removing a document deletes its row and owned graphs and reports that it existed; other graphs and default-graph data stay; removing again reports false.

### Shared graphs delete exact triples

With the fixed-graph and default-graph strategies, replacing or removing a document deletes exactly its previous triples and nothing of the other documents in the graph.

### Contexts load offline

A document with an unknown remote context fails naming the IRI; a context registered on the handle or persisted with `put_context` makes it convert without network access.

### Nested contexts load in rounds

A persisted context that imports another persisted context is resolved in successive reads.

### SPARQL finds documents

A document's graph answers `GRAPH <key>` queries, and graphs bound by SPARQL map back to their documents (or to none for plain graphs).

### Check and rebuild repair drift

After a SPARQL UPDATE edits a document's graph, `check_documents` reports the missing and extra quads, and `rebuild_graph` restores the conversion of the stored JSON.

### Metadata lookup

Documents are found by issuer, validity instant, one element of their types and keyset paging, from the metadata columns.

### Tables are created on demand

A store gets the three JSON-LD tables and the default metadata indexes only when a JSON-LD handle is opened, with its schema version and data unchanged.

### Large documents stay atomic

A document far above the SQL-length limit is written in chunks and reads back identical; a write that needs more statements than one batch allows fails with `DocumentTooLarge`.

### W3C toRdf suite

The W3C JSON-LD 1.1 `toRdf` tests run through `put_document`, with remote contexts served from the local checkout; outputs must be isomorphic to the expected N-Quads.

Allow-listed `json-ld` 0.21 limitations are tolerated, and the test fails when a listed entry starts passing.

## Verifiable Credentials

Specification scenarios of `oxilite-vc` on the same engines as the JSON-LD tests, with W3C VCDM example credentials as fixtures. See [[architecture#JSON-LD documents#Verifiable Credentials]].

### Credential id is key and graph

A VCDM 2.0 credential is stored under its `id`, read back verbatim, and its triples (type, issuer) are in the named graph of the same IRI.

### Key and graph are configurable

A JSON Pointer key and a template graph override the defaults for credentials.

### Credentials without id

A credential without `id` gets a stable content-hash key; with the reject policy it fails, and an explicit key stores it.

### Structure is checked

Credentials whose first context is not a W3C credentials context, without an issuer, or without the `VerifiableCredential` type are rejected with the violated rule and write nothing; VCDM 1.1 and 2.0 are recorded as `vc1` and `vc2`.

### Proofs live in owned graphs

A Data Integrity proof is in its own graph, linked from the credential graph, which holds no `proofValue`; removing the credential removes everything.

### Presentations store embedded credentials

A presentation is stored verbatim with profile `vp2`, and each embedded credential is stored under its own id and found by issuer; they outlive the presentation, and embedding can be switched off.

### Metadata across data model versions

Issuer, subject, types and validity are extracted from VCDM 1.1 (`issuanceDate`, `expirationDate`) and 2.0 (`validFrom`, `validUntil`) as epoch seconds.

### Metadata indexes are configurable

With no metadata indexes, no JSON-LD index exists but the metadata columns are still filled.

### Find valid credentials

Filtering by issuer and a validity instant returns only unexpired credentials; type filters and limits apply.

### SPARQL selects credentials

A SPARQL query over all graphs finds the credential holding a degree claim, and its graph maps back to the stored credential.

### Blank-node labels are stable

The blank-node labels and quad count of a reference credential are pinned, so a relabelling change in `json-ld` is noticed.

## Node

`@oxilite/node` tests over the napi-rs addon, next to the verbatim port of Oxigraph's `js/test/store.test.ts`, see [[architecture#Bindings]]. The port's failures must match `js:` entries of `testsuite/allowlist.toml`.

### File store persists across processes

A child Node process writes a quad to a SQLite file; a store reopened on that file in the test process sees it.

### Reasoning options

`reasoning: "rdfs"` entails types per query, and `materialize()` with both engines (`sql`, `reasonable`) makes `owl:sameAs` facts visible with `include_inferred`.

### Explain returns SQL

`explain()` returns the generated SQL for a SELECT, and `explainUpdate()` describes a compiled update.

### Cypher reads and writes the same dataset

`store.cypher()` creates nodes and a relationship with a property that SPARQL then finds, reads data inserted by SPARQL UPDATE, returns typed node and relationship objects, and `explainCypher()` shows the SQL.

### JSON-LD documents round-trip and query

`@oxilite/node` stores a document verbatim in its own graph, queryable with SPARQL and mapped back from its graph; options select pointer keys and template graphs; removal reports existence.

### Credentials are stored and found

`@oxilite/node` stores credentials under their id with validity as `Date`s, stores a presentation's credentials, finds them by issuer, validity and type, and raises `JsonLdError` codes for invalid credentials.

### Schema registry

The Node store registers ontology and shapes graphs by term or IRI, scopes RDFS reasoning to them, maps an ontology to other graphs or the default graph, exposes the shape index and rejects unknown roles.

### System graphs

`new Store({ systemGraphs: true })` starts with the vocabulary graph while a default store stays empty, and `installSystemGraphs()` adds it to an existing store once.

## Cypher

openCypher over the RDF store ([[architecture#Property graph frontend]]). Unless noted, each test runs on the bundled SQLite, the system `libsqlite3` (dylib), a D1-capability async store, and Miniflare D1 when `OXILITE_D1_URL` points at `testsuite/d1-sidecar`.

### Cypher nodes are visible to SPARQL

A node created with `CREATE (:Person {name: 'Ada'})` is found by a SPARQL `ASK` over `rdf:type` and the `name` property under the base IRI.

### RDF data is visible to Cypher

Turtle loaded with SPARQL-side APIs is matched by a Cypher `MATCH` over labels, a relationship and properties.

### A plain relationship is one triple

Creating a relationship without properties between existing nodes adds exactly one quad and no reifier.

### Relationship properties live on a reifier

A relationship property round-trips through Cypher, and SPARQL finds it on an RDF 1.2 reifier of the asserted triple.

### Parallel relationships stay distinct

Two relationships of one type between the same nodes are counted separately and keep their own properties; adding a third, plain one keeps the existing reifiers.

### Plain relationship becomes parallel

Adding a relationship with properties next to an existing plain one yields two relationships, and deleting the plain one keeps the other.

### Multi-valued properties read as lists

A property with several RDF values reads as a list in term order under the default policy, and equality in `WHERE` matches any of the values.

### List properties round-trip as rdf:JSON

List and map property values are stored as `rdf:JSON` literals and decode back, including map access and `size()`.

### WITH pipelines aggregation

`WITH` seals an aggregation that a later `WHERE` filters; `ORDER BY`, `SKIP` and `LIMIT` follow Cypher null ordering.

### OPTIONAL MATCH yields null

A person without a matching `OPTIONAL MATCH` row keeps its row, with null for the optional part.

### Reads are one SQL statement

A three-hop `MATCH` over plain relationships needs at most three backend requests: the query, term resolution and node materialisation.

### Relationships are not reused in one MATCH

Over a single relationship, a two-hop undirected pattern returns no rows, while a one-hop undirected pattern matches it in both directions.

### Variable-length paths follow trails

On a three-node cycle, `*1..5` stops when a relationship would repeat, and path values expose their length and nodes.

### Shortest paths run on D1

`shortestPath` and `allShortestPaths` return the shortest length on both backends, the D1 one included, as a breadth-first step machine.

### Nodes, relationships and paths are values

Returned nodes carry labels and properties and relationships carry type and properties, alongside `labels()`, `type()`, `keys()` and map projections.

### Explain shows the SQL

`explain_cypher` on a two-pattern `MATCH` reports the join order and the lowered SPARQL form.

### Read clauses and functions

A table of read queries covers string predicates, `IN`, null checks, pattern predicates, `EXISTS` subqueries, `collect` and other clauses, each with its expected rows.

### Parameters

`$` parameters, including a list of maps fed through `UNWIND`, drive both writes and reads.

### MERGE is idempotent

Running `MERGE` twice leaves one node with `ON CREATE` and `ON MATCH` effects applied, and later rows in one statement match nodes that earlier rows created.

### SET and REMOVE

`SET` updates properties and labels, `+=` merges maps with null removing a key, and `REMOVE` drops properties and labels.

### DETACH DELETE removes relationships

Deleting a node with incoming and outgoing relationships removes those relationships and their reifiers, and deleting everything restores the empty store.

### Deleting a connected node fails atomically

`SET` followed by `DELETE` of a node that still has relationships fails with a constraint error, and the `SET` is not applied.

### Per-row creation on D1

`UNWIND range(1, 100)` with `CREATE` makes 100 distinct nodes in one statement on both backends.

### Labels follow the class hierarchy

With RDFS reasoning, a label matches instances of its subclasses; without reasoning it does not.

### Inverse relationship types

With OWL QL reasoning, a relationship type also matches pairs stored only through its `owl:inverseOf` property.

### Required properties join without OPTIONAL

With a registered SHACL schema, a property with `sh:minCount 1` compiles to an inner join, while an optional property keeps a `LEFT JOIN`.

### SHACL guards abort invalid writes

Writes that break shape datatype, required-property or `sh:in` constraints fail with a shape violation and leave the store unchanged.

### Schema procedures

`db.labels()`, `db.relationshipTypes()` and `db.schema.nodeTypeProperties()` report labels, types and mandatory properties from the shapes and the data.

### Temporal values are stored as XSD literals

Dates, zoned datetimes and durations created by Cypher are `xsd:date`/`xsd:duration` literals for SPARQL, and read back with arithmetic, components, week dates and `duration.between`.

### Union default graph reads every graph

With `union_default_graph`, patterns, labels and properties come from every named graph, so Cypher reads JSON-LD documents and credentials; a triple stored in two graphs counts once.

### Ordering by an aggregate

`RETURN p.name, count(f) AS n ORDER BY n DESC` sorts the grouped rows: the compiler seals the grouped block before sorting by its aggregates, which SQLite cannot sort in place.

### Pattern comprehensions

`[(p)-[:KNOWS]->(f) WHERE … | f.name]` gives each row the list of its own matches (empty when none), with path variables and `size()` over the list.

### Schema datatypes type comparisons

With the SHACL schema loaded, a property with `sh:datatype xsd:integer` compiles `p.age > 30` to a single numeric comparison instead of one per possible type, with the same answer.

### SPARQL and Cypher agree

Over one dataset (labels, multi-valued and temporal properties, reified and parallel relationships, an ontology), 21 questions asked in SPARQL and in Cypher give the same rows; Cypher is checked on the native store and on the D1 code path.

### openCypher TCK

The openCypher TCK (`testsuite/openCypher`, 3880 scenarios) runs through a Gherkin runner with result tables, expected errors and side effects diffed from graph snapshots; failures must match `tck-allowlist.txt`.

The test fails when an unlisted scenario fails or a listed one passes, so the list only shrinks. `OXILITE_TCK_REPORT=1` prints each failure with its query.

The runner uses its own 16 MiB thread: path-heavy scenarios (Match6/7/9, Pattern2) compile to algebra about a hundred joins deep, which overflows the 2 MiB default test stack in debug builds.

### Shape index agrees with the shapes query

The compiled shape index and the shapes SPARQL query describe the same constraints over one dataset.

The shapes use datatypes, cardinalities, patterns, `sh:in` lists (with inline integer and boolean values), relationship-valued shapes, and two shapes targeting the same class and path.

## Datalog

The Datalog dialect: parsing, compilation and evaluation against a real store, see [[architecture#Datalog frontend]].

### Derived relation from a triple pattern

A rule whose body is one IRI predicate defines a relation over that triple pattern, and the goal returns its variables in the order the goal names them.

### Join of two atoms

Two body atoms sharing a variable join, so a grandparent rule returns exactly the pairs two `ex:parent` steps apart.

### Transitive closure agrees with a property path

A linear recursive rule returns exactly what the SPARQL property path `ex:parent+` returns over the same data, which is the oracle for the recursion.

### Numeric comparison

A constraint comparing a variable with a number filters the rule, so an age threshold selects only the people above it.

### Arithmetic in a constraint

Arithmetic on a bound variable is evaluated inside the constraint, so `?a * 2 < 40` selects on the computed value.

### Negation over a derived relation

`not` over a recursively derived relation is stratified: a rule for people with no ancestor returns only the root of the parent chain.

### Negation compiles to NOT EXISTS

A negated body atom becomes `NOT EXISTS` in the generated SQL rather than an anti-join built in Rust.

### Count grouped by key

`COUNT(?y)` in a rule head groups by the head's other arguments, so each subject gets the number of its derived matches as an `xsd:integer`.

### Unstratified negation is rejected

A rule whose body negates the predicate it defines is rejected before compilation, with an error saying the program is not stratified and naming the cycle.

### Unbound head variable

A head variable that no positive body atom binds is unsafe: the program is rejected with that variable named.

### Unbound negated variable

A variable occurring only inside a negated atom is unsafe and is rejected with that variable named.

### Quantifying over the predicate

The built-in `triple/3` atom binds the predicate position, so a rule can range over the predicates a subject uses.

### Cycle terminates

Recursion over cyclic data reaches a fixpoint instead of looping: on a three-node cycle every node reaches every node, including itself.

### Goal with a constant

A goal that fixes one argument to a constant projects only the remaining variable.

### Request count

A non-recursive program compiles to a single SQL statement, with no statement separator in the generated SQL.

### Options scope graphs

A rule matches the default graph by default; `union_default_graph` widens it to every graph, which makes a triple in a named graph visible.

### Even and odd

Two predicates defined in terms of each other over a successor chain return the even and the odd positions respectively, which is the smallest real mutual recursion.

### Mutual recursion uses one tagged member

A mutually recursive component compiles to a single `WITH RECURSIVE` member carrying a discriminant column, and `explain()` says so.

### Non-linear recursion agrees with the linear form

A rule with two recursive body atoms is iterated in the work table, and returns exactly what the linear formulation of the same closure returns.

### Doubling reaches the fixpoint faster

A non-linear rule composes the relation with itself, so a four-hop chain closes in a handful of rounds rather than one per hop.

### Iterated component is reported

`explain()` names the iteration strategy and says it costs one request per round, so the price is never a surprise.

### Iteration bound

A component that has not converged within `max_iterations` fails naming the bound instead of running unbounded.

### Iteration leaves no rows behind

Running the same iterated program twice gives the same answer, so an evaluation's work rows do not outlive it.

### Linear rewrite of a non-linear closure

Written in linear form, the same closure compiles to one statement and returns every pair along the chain.

### Materializing an iterated component

A program whose recursion is non-linear can still be materialized: the iteration runs first, then the conclusions are written in one atomic request.

### Iterated materialization cleans up

Materializing an iterated program twice gives the same count, so neither the work rows nor the previous inferences leak into the second run.

### Derived facts become queryable

Materializing a program stores its conclusions as inferences: the derived predicate is absent from a plain query and present with `include_inferred`.

### Asserted data is untouched

Materializing leaves the asserted quad count unchanged, and clearing the inferences leaves it unchanged again.

### Re-running replaces

Materializing a second, smaller program replaces the previous conclusions instead of adding to them.

### Producers keep their own conclusions

OWL 2 RL and a named rule program materialize side by side: each conclusion is attributed to its producer, re-running one keeps the other's, and clearing one producer removes only its own.

### Non-triple head is rejected

A rule head with no RDF form cannot be materialized: the call fails and nothing is written.

### Unary head materializes as rdf:type

A one-argument head is stored as an `rdf:type` triple, so the derived class answers `?p a ex:Adult` with inferences included.

### A unary atom is a class

A one-argument IRI atom reads as `rdf:type`, so `ex:Person(?x)` returns every subject typed with that class.

### An aggregate with no inline form is rejected

`AVG` and `GROUP_CONCAT` produce values that the encoding cannot turn into a term id inside SQL, so a head using them is refused with that reason.

### Sum and min over a group

`SUM` aggregates the numeric values behind the ids and returns the total as an `xsd:integer` term.

### String constraint

A string function compares the lexical form behind an id against a constant written in the program, which the store need never have seen.

### Backend without compound recursive CTEs

On a backend that lacks compound recursive common table expressions, a mutually recursive program is refused with an error naming the SQLite version that would support it.

### The documented program runs

The program printed in the README and on the website is executed end to end, so the documentation cannot drift away from what the dialect accepts.

### D1 statement length

No statement a program compiles to — goal, seed or step — exceeds D1's 90 KB limit, checked without a D1 binding.

### D1 compound limit

No compound SELECT exceeds the five terms D1 allows, so a predicate with five rules still compiles.

### D1 never gets a compound recursive term

Mutual recursion is refused on a backend that has not declared compound recursive terms, rather than emitting SQL that would fail there.

### D1 never needs user-defined functions

`REGEX` is refused on a backend without the oxilite functions, rather than emitted and failing at execution.

### D1 materialization stays within the statement budget

A materialization plan fits D1's limit on statements per request, and each of its statements fits the length limit.

### Bound parameters are never needed

Generated statements carry their own constants: no placeholder appears in any of them.

## Planner benchmark

`cargo run --release -p oxilite --example planner_bench` loads 350 010 quads (50 000 people) and compares the oxilite planner with SQLite's planner on three join-heavy queries.

Measured on an Apple Silicon laptop (M1 milestone):

| query | oxilite planner | SQLite planner |
|---|---|---|
| rare-badge-star | 75 µs | 7.2 ms |
| friends-of-badged | 96 µs | 87 µs |
| city-age-filter | 478 µs | 7.8 ms |

## Shell

The interactive shell of [[architecture#Command line and HTTP endpoint#Interactive shell]], driven through a session with captured output and through the binary with a script on standard input.

### Statements run when complete

Brackets inside strings, IRIs and comments do not count; one line runs when it parses, several lines wait for `;` or an empty line and then run once, and a broken statement reports its error.

### Exit stops at once

`.exit`, `.quit` and `.exit N` end the session with status 0 or `N`, even with leading spaces.

### Session prefixes

Well-known prefixes are declared for statements that use them, a statement's own declaration wins, and a `PREFIX` line joins the session so later statements and results use it.

### Tables fit the terminal

Column widths shrink the widest column first to fit the terminal, no table line is wider than the terminal, and clipped cells end with `…`.

### Completion knows the store

Predicates and classes of the store complete after their prefix in the right position, keywords complete from their start, variables from earlier lines of the statement complete, and dot-commands and their fixed arguments complete.

### Save copies the store

`.save FILE` copies default-graph and named-graph quads into a new SQLite file and refuses a file that exists.

### A new file gets the schema

`oxilite FILE` creates the file with the schema, and what the shell inserted is seen by `oxilite query -l FILE` and by `oxilite -l FILE`.

### In memory by default

`oxilite` without a file keeps inserts for the session and creates no file; subcommands behave as before.

### Scripts report failures

A script prints what succeeded, reports errors with their line on standard error and exits with 1; `.exit` stops before later lines run.

### Schema registry commands

`.register` (loading a file), `.map`, `.registry`, `.shapes`, `.activate`, `.deactivate` and `.unregister --drop` manage the registry from the shell; unknown roles and graphs report errors.

### Session query options

`.reasoning`, `.inferred` and `.schemagraphs` apply to the user's queries and `.explain`, not to `.dump`; `.materialize` and `.materialize clear` drive OWL 2 RL inferences.

## Command line

The `oxilite` subcommands beyond the shell, see [[architecture#Command line and HTTP endpoint]].

### Registry and reasoning flags

A new CLI store starts with the system graphs, `registry` manages registrations, and the query flags change what queries match.

The system graphs can be skipped (`--no-system-graphs`) and installed later (`registry init`); `registry register --file` loads and records a digest, `list --json`, `map`, `activate`, `drop` and `shapes` work on a file store, and `query` / `explain` honour `--reasoning`, `--inferred` and `--no-schema-graphs`.

## Studio server

The language server behind oxilite studio, driven over an in-memory LSP connection against a temporary workspace, see [[architecture#Studio server]].

### Files load on initialize

Initialize loads the RDF files under the root, including subdirectories, and skips hidden directories, `node_modules` and non-RDF files.

### Query joins across files

A SPARQL query joins triples from two different files, because the default graph is the union of the per-file graphs.

### Row limit truncates

A query with `limit` returns at most that many rows and sets `truncated`.

### Syntax error is a diagnostic

A Turtle file with a syntax error loads no triples and publishes one diagnostic starting on the error's line.

### Reload picks up changes

After files are fixed and added on disk, `oxilite/reload` loads them and clears the earlier diagnostic.

### Query errors are request failures

A query that does not parse answers with a `RequestFailed` error instead of crashing the server.

### Scanner roles and prefixes

The lenient scanner assigns subject, predicate and object roles through `;`, `,` and nested blank nodes, resolves prefixes and relative IRIs, skips expressions, and scans unfinished text with UTF-16 columns.

### Index finds definitions

Definitions are subject occurrences across files, references count every occurrence, a subject's statement with a given predicate is located, and removing a file forgets its occurrences.

### Completion follows the triple role

Predicate position offers predicates in store frequency order, after `a` only classes, an empty position prefixes, `a` and keywords, and a variable the other variables.

### Completion declares missing prefixes

A prefix known only from a workspace file completes with an edit adding its declaration to the query.

### Syntax diagnostics as you type

SPARQL errors land on the reported line, updates are accepted, Turtle reports every broken statement, and an unused predicate gets a warning.

### Attaching without activating

`oxilite/attach` with `activate: false` adds the connection but leaves the active one alone; requests naming it reach it, and unnamed requests keep using the active one.

### Hover and definition across files

Hovering an IRI in a query shows its label and where it is defined; definition jumps to the subject line in another file, and references find every mention.

### Initialize announces its capabilities

The `initialize` result carries the capabilities at the top level; nested one level deeper, a client would see none and never sync documents.

### Completion follows the document's connection

A document named for an attached connection completes from that store's vocabulary, and falls back to the active one when the name is cleared.

### Completion over LSP uses the store

An opened query document gets store-driven completion and its own diagnostics.

### Attached stores and confirmed updates

Attaching a store makes it active; an update fails with the confirmation code until confirmed, Project store updates are ephemeral, and detaching returns to the Project store.

### Explain returns the SQL

`oxilite/explain` returns the compiled SQL for a query on the active connection.

### Manifest assigns graphs and roles

Globs assign files to graphs and roles, the profile is read, an unmatched file is not loaded, and a malformed manifest is rejected.

### Conventions sniff roles

Without a manifest, SHACL shape classes make a shapes file, `owl:Ontology` an ontology, `.dl` rules, other RDF data, and unknown extensions are ignored.

### SHACL results land on the data line

Without reasoning the data conforms; with RDFS an employee without a name violates the person shape, the diagnostic sits on its statement's line with the shape linked, and shapes are not queryable data.

### Manifest graphs and per-graph reload

Manifest graphs load under their IRIs, OWL 2 RL and a rule file materialize side by side, editing one file reloads its graph and reasoning follows, and the resource view names each inference's producer.

### Datalog runs and reports errors

A program's goal returns its solutions, explain describes it, an unsafe rule is a diagnostic on its line, and rule bodies complete the store's predicates.

### Cypher runs over the data's own names

`:Person` and `:knows` resolve to the data's namespace, paths come back as path values, labels and relationship types complete by name, and a syntax error is a diagnostic.

### Import and export round-trip

Importing a file into an attached store needs confirmation and can target a graph; exporting writes N-Quads with that graph, and a dataset refuses a single-graph format.

### Why explains inferences down to asserted lines

A type inferred through two subclass steps is explained by the subclass template with an inferred premise; a rule conclusion names its rule, producer and located premises.

Asserted triples are leaves, and triples that do not hold are reported absent.

### Why picks the rule whose premises hold

When several rules can derive a triple, the explanation uses one whose premises hold: a report two levels down is explained by the recursive rule through the middle manager, not by the direct rule.

### Manifest tests run and snapshot

Tests are listed with their lines; a query test fails without an expected file until a snapshot is written, and a stale expectation reports what is missing.

A fixture violates exactly the named shape, the project conforms, and entailments hold.

### Check reports everything and fails

`oxilite check` fails on failing tests, renders them for a terminal, and reports a SHACL violation with the data file and line.

### D1 over HTTP with billing

Against a stand-in for D1's `/raw` endpoint: attaching makes D1 active, and an update asks first with its billed-row estimate.

64-bit ids survive the JSON round trip, and each payload and the connection report their cost.

### MCP tools answer agents

The MCP server initializes, lists its tools, answers a query as a table, summarizes the schema with prefixes, validates, explains a rule conclusion, and reports a bad query as a tool error.

### ShEx results are diagnostics

A ShEx schema and its shape map validate the data: the nonconforming node is a result located on its statement, with the shape located in the `.shex` file.

### Full-text search with the manifest index

With `text_index = true`, `oxl:textMatch` finds the literal and the plan uses the full-text index.

### Ontology diagram data

The ontology request returns declared and used classes, the subclass link, an object property with its range, and a datatype property flagged as such.

### Datalog debugger counts per rule

Each rule reports its body matches and its head's facts, and a rule over a predicate nobody uses matches nothing, at its line.

### Explorer shows the class hierarchy

The explorer lists its folders, roots the class tree at the superclass with its inferred instances counted, lists subclasses with asserted counts, and gives files their roles.

### Ontology graphs apply where the manifest maps them

A manifest ontology graph with `applies_to` makes employees of the mapped graph persons, not those of another graph, and the mapping is visible in `<oxilite:schema>`.

## Versioning

The store clock, the immutable change log and time travel ([[architecture#Versioning]]), on bundled SQLite, the system SQLite, the D1 code path and Miniflare D1.

### Off store is unchanged

A store without versioning sends no statement touching `ticks` or `quad_log`, inserts quads with today's statement, reports level `off`, and refuses an as-of query as keeping no history.

### Stamps record the adding tick

At `stamped`, each write opens a tick and each new quad records it: changes since a tick list exactly the later additions in tick order, re-adding keeps the first tick, removals leave no trace, and as-of is refused.

### As-of equals snapshots

A deterministic random walk of inserts, removals and DELETE/INSERT updates; after every step the store is snapshotted, and a query as of each step's tick returns exactly that snapshot.

### Only effective changes are logged

Re-adding a present quad and removing an absent one add no commit, and a quad removed and re-added within one update leaves no change in the log.

### History is immutable

Updating or deleting rows of `quad_log` or `ticks` directly fails with "history is immutable".

### Commits carry author and message

Writes inside `with_commit` record its author and message on their commit, with their counts of added quads; later writes record none; the genesis appears in the history.

### Versions compare within one query

`SERVICE <oxilite:version/HEAD~1>` joins the previous version with the current one, returning each changed status with its old and new value; the diff of the two versions has one addition and one removal.

A property path (`next+`) reaches one node fewer as of the version before the last edge was added.

### Opening never changes the level

Opening a store holding data with a higher level fails and names the explicit change; after `set_versioning`, reopening with default options keeps the level and history keeps recording.

### Upgrade records the store as genesis

Raising a plain store to `log` records its quads as the genesis commit: as-of genesis returns them even after later removals, and a tick before genesis is outside the history.

### Downgrades freeze and resume

Lowering to `stamped` freezes the history, which still answers at the freeze; raising it again records the gap on a resume tick, and as-of inside the gap fails.

Writes in the gap are not logged, the history stays exact after the resume, and lowering to `off` with `allow_loss` drops the log, the ticks and the stamp column but no quad.

### Updates do not read the past

An update whose `WHERE` reads `SERVICE <oxilite:version/HEAD~1>` fails with an error saying versions are read in queries only, and writes nothing.

### Off store SQL matches the pre-versioning snapshot

Every W3C SPARQL 1.0, 1.1 and 1.2 query and update, and the batch writer's statements, compile for an unversioned store to the SQL and planner notes recorded from 0.3.1, before versioning.

The snapshot covers native and D1 capabilities; renderings that embed fresh random blank nodes are recorded as varying, and the seven files with `\u` escapes are left out, since how they parse depends on a parser feature workspace builds enable.

### History graph answers who changed what

`GRAPH <oxilite:history>` returns the author, message, time and previous commit of the commit that removed a triple, and every added triple with its commit's author.

The commit's number is its tick (`#n`); a store without a log refuses changes but still lists its commits.

### Datalog compares versions per atom

`at "HEAD~1"` on one atom compares a status with its previous value, `at ?c` bound by `commit` gives the value at every commit, and an `at` variable nothing binds is unsafe.

### Datalog reads the history relations

`removed` joined with `commit` names who removed each value, ancestry recurses over `commit`, and `branch("main", ?c)` has one row.

The default graph reads as unbound, a program's own `commit` relation wins, and an unversioned store refuses the built-ins.

### Cypher reads a past version

`asOf: "HEAD~1"` returns the statuses before the last `SET`, node materialization included, on the native store and the D1 code path; a writing statement with a version is refused.

### D1 driver queries the history

`@oxilite/d1` on Miniflare reads who removed a triple from the history graph and runs Cypher at `HEAD~1`, refusing a versioned write.

### Node binding queries the history

`@oxilite/node` answers Cypher at `HEAD~1`, lists the authors in the history graph and reads `removed` in Datalog.

### Purge removes from history

A purge by subject removes the quads from the store, from every as-of state and from the change feed, and records a purge commit carrying the reason and no removed content.

### D1 batches keep a statement for the tick

With D1's limits, a versioned store's capabilities reserve two of the 50 statements (the tick's terms and the tick), stamped inserts stay under the SQL length limit, the prepared batch fits D1, and unversioned inserts never mention `ticks`.

### Every engine keeps the same history

The same scenario — commits, as-of by `HEAD~n`, commit counts per change, freeze, a write in the gap, resume — gives the same answers on bundled SQLite, the system SQLite, the D1 code path and, with `OXILITE_D1_URL`, Miniflare D1.

### Datalog reads a past version

An `@version "HEAD~1"` directive and `Options::as_of` run a recursive program on the previous version; materialization with a version and a version on an unversioned store are refused.

### D1 driver keeps history

`@oxilite/d1` on Miniflare opens a store at level `log`, records a commit's author and message, answers `as_of` queries, diffs and `SERVICE` comparisons, and applies a level change as a migration run with `db.exec`.

### Node binding keeps history

`@oxilite/node` records commits with `withCommit`, answers `as_of` queries and Datalog `asOf`, diffs versions and freezes the history by lowering the level.

