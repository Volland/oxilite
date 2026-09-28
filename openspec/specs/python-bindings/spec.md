# python-bindings Specification

## Purpose
Provides oxilite to Python applications with the API of pyoxigraph (Oxigraph's Python package), backed by
SQLite files, plus every oxilite extension with typed results.

## Requirements

### Requirement: Store lifecycle
The package SHALL open:
- an in-memory store
- a store in a SQLite file
- a store in `oxilite.sqlite` inside a given existing directory
- an existing store read-only
- a store through a SQLite shared library given at runtime

Store options SHALL be keyword arguments: `graph_index`, `text_index`, `versioning`, `as_of_index`,
`stamp_index` and `system_graphs`.

#### Scenario: File store persists across processes
- **WHEN** a child Python process adds a quad to a store opened on a file, and the test process then opens
  the same file
- **THEN** the quad is in the store

#### Scenario: Directory path
- **WHEN** `Store(dir)` is given an existing directory
- **THEN** the data is stored in `dir/oxilite.sqlite`, and `Store.read_only(dir)` reads it back

#### Scenario: Read-only store refuses writes
- **WHEN** a quad is added to a store opened with `Store.read_only`
- **THEN** the call raises `OSError`

### Requirement: pyoxigraph-compatible API
The package SHALL export the pyoxigraph names, arguments and result types listed below, and SHALL behave as
pyoxigraph does:
- terms: `NamedNode`, `BlankNode`, `Literal`, `DefaultGraph`, `Triple`, `Quad`, `Variable`, `BaseDirection`
- formats: `RdfFormat`, `QueryResultsFormat`
- results: `QuerySolutions`, `QuerySolution`, `QueryBoolean`, `QueryTriples`
- functions: `parse`, `serialize`, `parse_query_results`
- `Store`: `add`, `extend`, `bulk_extend`, `remove`, `__contains__`, `__len__`, `__iter__`,
  `quads_for_pattern`, `query`, `update`, `load`, `bulk_load`, `dump`, `named_graphs`,
  `contains_named_graph`, `add_graph`, `clear_graph`, `remove_graph`, `clear`, `flush`, `optimize`, `backup`
  and `read_only`

#### Scenario: SELECT result shape
- **WHEN** `store.query("SELECT ?s ?o WHERE { ?s ?p ?o }")` runs on a store with one triple
- **THEN** it returns a `QuerySolutions` whose `variables` are `[Variable("s"), Variable("o")]`, and its
  solution can be read by index, by name, by `Variable`, and by unpacking

#### Scenario: ASK and CONSTRUCT
- **WHEN** an ASK query and a CONSTRUCT query run
- **THEN** the ASK returns a `QueryBoolean` usable as a `bool`, and the CONSTRUCT returns a `QueryTriples`
  that iterates over `Triple`s

#### Scenario: pyoxigraph tests
- **WHEN** pyoxigraph's `test_store.py`, `test_model.py` and `test_io.py` run with only the import changed
- **THEN** every test passes, except those allow-listed as `py:` divergences in `testsuite/allowlist.toml`
- **AND** each allow-listed test fails, so the allow-list has no stale entries

### Requirement: Terms behave as values
Terms SHALL be immutable, hashable and comparable by value. They SHALL support `pickle`, `copy`, structural
pattern matching (`match`) and N-Triples-style `str()`. Constructors SHALL reject invalid IRIs, blank-node
ids and language tags with `ValueError`.

#### Scenario: Literal from Python values
- **WHEN** `Literal(True)`, `Literal(1)` and `Literal(0.1)` are built
- **THEN** they equal `Literal("true", datatype=xsd:boolean)`, `Literal("1", datatype=xsd:integer)` and
  `Literal("1.0E-1", datatype=xsd:double)`

#### Scenario: Invalid IRI
- **WHEN** `NamedNode("not an iri")` is built
- **THEN** it raises `ValueError`

### Requirement: Query options
`Store.query` SHALL accept pyoxigraph's options: `base_iri`, `prefixes`, `use_default_graph_as_union`,
`default_graph`, `named_graphs` and `substitutions`. It SHALL also accept oxilite's: `reasoning`
(`"none"`, `"rdfs"` or `"owl-ql"`), `include_inferred`, `include_schema_graphs` and `as_of`.
`custom_functions` and `custom_aggregate_functions` SHALL raise `NotImplementedError`.

#### Scenario: Substitutions
- **WHEN** a query `SELECT ?s ?p ?o WHERE { ?s ?p ?o }` runs with `substitutions={Variable("s"): foo}` on a
  store where two subjects have triples
- **THEN** only the solutions with `?s = foo` are returned

#### Scenario: Query-time reasoning
- **WHEN** a store holds `ex:a rdf:type ex:Dog` and `ex:Dog rdfs:subClassOf ex:Animal`, and
  `ASK { ex:a a ex:Animal }` runs with `reasoning="rdfs"`
- **THEN** it returns true, and false without the option

### Requirement: I/O
`load`, `bulk_load`, `parse` and `parse_query_results` SHALL accept `str`, `bytes`, a file object, or
`path=`. When a path is given without a format, the format SHALL be inferred from the extension. `dump`,
`serialize` and a result's `serialize` SHALL write to a binary file object or a path, or return `bytes` when
no output is given. Syntax errors SHALL raise `SyntaxError` with the parser's location.

#### Scenario: Round trip through a file
- **WHEN** a store is dumped to `data.nq`, and `data.nq` is loaded into a new store with `path=` and no format
- **THEN** both stores hold the same quads

#### Scenario: Syntax error location
- **WHEN** a Turtle file whose second line is invalid is parsed
- **THEN** `SyntaxError` is raised with `filename` set and `lineno` equal to 2

### Requirement: Explain
The package SHALL expose `explain(query)` and `explain_update(update)`, which return the generated SQL and the
planner's notes.

#### Scenario: Explain a SELECT
- **WHEN** `explain` is called with a SELECT query
- **THEN** the returned string contains `SELECT`

### Requirement: Cypher from Python
The package SHALL expose `cypher(query, params=None, **options)` and `explain_cypher`. The result SHALL be a
`CypherResult` with `columns`, `rows`, `records` (dicts keyed by column) and `stats`. Nodes, relationships,
paths and temporal values SHALL be the dataclasses `CypherNode`, `CypherRelationship`, `CypherPath` and
`CypherTemporal`.

#### Scenario: Cypher writes are visible to SPARQL
- **WHEN** `cypher("CREATE (:Person {name: 'Ada'})", base="http://ex/")` runs
- **THEN** a SPARQL query finds `?p <http://ex/name> "Ada"`, and the result's `stats.nodes_created` is 1

#### Scenario: Parameters and records
- **WHEN** `cypher("MATCH (p:Person {name: $name}) RETURN p.name AS n", {"name": "Ada"}, base=…)` runs
- **THEN** `records` is `[{"n": "Ada"}]`

### Requirement: Datalog from Python
The package SHALL expose `datalog`, `datalog_materialize` and `explain_datalog`. Solutions SHALL be oxilite
terms in `rows`, with `None` for an unbound column, and `records` SHALL be keyed by the goal's variables.
`rounds` SHALL report how many rounds each iterated component took.

#### Scenario: Recursion
- **WHEN** a transitive-closure program runs with a constant in its goal
- **THEN** every reachable node is returned as a `NamedNode`

#### Scenario: Materialization feeds SPARQL
- **WHEN** `datalog_materialize` runs, and a SPARQL query then runs with `include_inferred=True`
- **THEN** the query sees the derived triples, and a query without the option does not

#### Scenario: A rejected program raises
- **WHEN** an unstratified program is submitted
- **THEN** an exception is raised whose message says the program is not stratified

### Requirement: Reasoning
The package SHALL expose `materialize(engine="sql" | "reasonable")`, which returns the number of inferred
triples, and `clear_inferences()`.

#### Scenario: Both engines agree
- **WHEN** `materialize` runs with each engine on a store with an `owl:sameAs` fact
- **THEN** both return the same count, and `include_inferred=True` queries see the entailed triples

### Requirement: Schema registry
The package SHALL expose:
- `register_schema_graph(graph, role, *, iri, version, sha256, imports, applies_to, active)`
- `schema_graphs()`, `set_schema_graph_active`, `unregister_schema_graph`, `drop_schema_graph`
- `shape_index()` and `install_system_graphs()`

A graph argument SHALL be a term or an IRI string.

#### Scenario: Registration round trip
- **WHEN** an ontology graph is registered with `applies_to=[DefaultGraph()]` and `version="1.0"`
- **THEN** `schema_graphs()` returns an entry with role `"ontology"`, version `"1.0"`, and the default graph's
  IRI in `applies_to`

#### Scenario: System graphs
- **WHEN** a store is opened with `system_graphs=True`
- **THEN** the graph `<oxilite:vocabulary>` holds triples, and a default store has none

### Requirement: JSON-LD documents and credentials
The package SHALL expose `jsonld(**options)` and `credentials(**options)`, which return handles with `put`,
`put_all`, `get`, `remove`, `list`, `find`, `graphs`, `document_for_graph`, `put_context`, `remove_context`,
`contexts`, `rebuild` and `check`. The credentials handle SHALL also have `put_presentation`. Errors SHALL
raise `JsonLdError` with the JSON-LD error code in `code`.

#### Scenario: Document round trip
- **WHEN** a JSON-LD document is stored with `put`
- **THEN** `get` returns its exact text, and SPARQL finds its triples in the graph named after its `@id`

#### Scenario: Credentials by validity
- **WHEN** a credential is stored and `find(issuer=…, valid_at=datetime)` runs
- **THEN** the credential is returned, with `valid_from` as a timezone-aware `datetime`

#### Scenario: Error code
- **WHEN** a credential without `type` is stored
- **THEN** `JsonLdError` is raised with a non-empty `code`

### Requirement: Versioning
The package SHALL expose:
- `versioning()` and `set_versioning(level, *, as_of_index, stamp_index, allow_loss, author, message)`
- `set_commit_info` and the context manager `commit(author=…, message=…)`
- `history(limit)`, `changes(after, until)`, `diff(from_, to)` and `purge(…, reason=…)`
- the query option `as_of`

#### Scenario: Time travel
- **WHEN** a store opened with `versioning="log"` receives two commits, and a query runs with
  `as_of="HEAD~1"`
- **THEN** it sees the store as it was after the first commit

#### Scenario: Commit metadata
- **WHEN** a write runs inside `with store.commit(author="ada", message="fix")`
- **THEN** `history()[0]` has that author and message, and later writes do not

### Requirement: Concurrency
Store methods SHALL release the GIL while the store works, and a store SHALL be safe to share between Python
threads.

#### Scenario: Threads share a store
- **WHEN** four threads each insert 100 distinct quads into one store
- **THEN** the store holds 400 quads

### Requirement: Typed package
The package SHALL ship `py.typed` and type annotations for every public name, including a stub for the native
module.

#### Scenario: Strict type check
- **WHEN** a program using the package is checked with `mypy --strict`
- **THEN** it reports no errors from the package's types

### Requirement: Distribution
The package SHALL be installable with `pip install oxilite` from `abi3` wheels for Linux (glibc x86_64 and
aarch64, musl x86_64), macOS (x86_64 and arm64) and Windows (x64), on CPython 3.9 or later. An sdist SHALL
build it from source where no wheel fits. The package version SHALL equal the Cargo workspace version.

#### Scenario: Wheel smoke test
- **WHEN** a built wheel is installed into a fresh environment
- **THEN** `python -c "import oxilite; oxilite.Store().update('INSERT DATA { <urn:a> <urn:b> <urn:c> }')"`
  succeeds
