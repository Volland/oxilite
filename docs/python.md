# oxilite for Python: reference

This page is the reference for the `oxilite` Python package: every class, method, option and result type,
with the exceptions each can raise.
- For a guided introduction, read [How to use oxilite with Python](https://oxilitedb.com/articles/oxilite-python).
- To build the package or publish it to PyPI, read [python-publishing.md](python-publishing.md).

```bash
pip install oxilite
```

The package needs CPython 3.9 or later. Wheels are published for:
- Linux (glibc x86_64 and aarch64, musl x86_64)
- macOS (x86_64 and arm64)
- Windows (x64)

Elsewhere, `pip` builds the package from the source distribution, which needs a Rust toolchain.

## Contents

- [The model in one paragraph](#the-model-in-one-paragraph)
- [Coming from pyoxigraph](#coming-from-pyoxigraph)
- [Terms](#terms)
- [Formats](#formats)
- [Store](#store)
- [SPARQL queries and updates](#sparql-queries-and-updates)
- [Loading and dumping](#loading-and-dumping)
- [Parsing and serializing without a store](#parsing-and-serializing-without-a-store)
- [Explain](#explain)
- [openCypher](#opencypher)
- [Datalog](#datalog)
- [Reasoning](#reasoning)
- [Schema registry and system graphs](#schema-registry-and-system-graphs)
- [JSON-LD documents](#json-ld-documents)
- [Verifiable Credentials](#verifiable-credentials)
- [Versioning and time travel](#versioning-and-time-travel)
- [Full-text search](#full-text-search)
- [Your own SQLite library](#your-own-sqlite-library)
- [Threads and the GIL](#threads-and-the-gil)
- [Exceptions](#exceptions)
- [Differences from pyoxigraph](#differences-from-pyoxigraph)

## The model in one paragraph

A `Store` is one SQLite database: a file, or memory. It holds an RDF dataset (quads) and answers SPARQL 1.1
and SPARQL 1.2 (RDF-star) queries by compiling each one to a single SQL statement. The same quads can be
read and written with openCypher (nodes are IRIs, labels are `rdf:type`, properties are literal triples)
and queried with Datalog. They can be reasoned over with RDFS and OWL, described by registered ontologies
and SHACL shapes, filled from JSON-LD documents and Verifiable Credentials, and versioned with an immutable
change log. Every one of these is a method on `Store`.

## Coming from pyoxigraph

Change the import:

```python
import oxilite as pyoxigraph            # or: from oxilite import Store, NamedNode, …
```

What stays the same:
- the terms, the formats, the result classes, `parse`, `serialize` and `parse_query_results`
- every `Store` method pyoxigraph has, with the same arguments and results

What changes:
- **The store location.** pyoxigraph stores data in a RocksDB directory. oxilite stores it in one SQLite
  file. `Store("data.sqlite")` opens a file. `Store("data/")`, given an existing directory, uses
  `data/oxilite.sqlite`, so pyoxigraph code that passes a directory keeps working.
- **Three unsupported features.** `custom_functions`, `custom_aggregate_functions` and remote SPARQL `LOAD`
  are not supported (see [Differences from pyoxigraph](#differences-from-pyoxigraph)).

## Terms

Terms are immutable values. They can be hashed, compared by value, pickled and copied, and used in
`match` statements. Constructors validate their input and raise `ValueError` for an invalid IRI, blank-node
id or language tag.

| Class | Constructor | Attributes | `str()` |
|---|---|---|---|
| `NamedNode` | `NamedNode("http://ex/a")` | `value` | `<http://ex/a>` |
| `BlankNode` | `BlankNode()` (fresh id) or `BlankNode("b1")` | `value` | `_:b1` |
| `Literal` | `Literal("chat", language="fr")`, `Literal("5", datatype=XSD_INTEGER)`, `Literal(5)`, `Literal(True)`, `Literal(0.5)` | `value`, `datatype`, `language`, `direction` | `"chat"@fr` |
| `BaseDirection` | `BaseDirection.LTR`, `BaseDirection("rtl")` | `value` | `rtl` |
| `DefaultGraph` | `DefaultGraph()` | `value` (`""`) | `DEFAULT` |
| `Variable` | `Variable("s")` | `value` | `?s` |
| `Triple` | `Triple(s, p, o)` | `subject`, `predicate`, `object`; indexable, unpackable | `<s> <p> <o>` |
| `Quad` | `Quad(s, p, o, graph_name=None)` | `subject`, `predicate`, `object`, `graph_name`, `triple` | `<s> <p> <o> <g>` |

Python values make typed literals:
- `Literal(True)` is `"true"^^xsd:boolean`
- `Literal(42)` is `"42"^^xsd:integer`
- `Literal(0.1)` is `"1.0E-1"^^xsd:double`

A literal with a language tag has the datatype `rdf:langString`. With a direction too (`direction=`), it
has `rdf:dirLangString` (RDF 1.2). A triple can be the object of another triple, which is RDF 1.2's triple
term:

```python
from oxilite import Triple, NamedNode, Literal, Quad

ex = lambda n: NamedNode(f"http://example.org/{n}")
claim = Triple(ex("bob"), ex("age"), Literal(23))
quad = Quad(ex("alice"), ex("claims"), claim)
s, p, o, g = quad
match quad:
    case Quad(NamedNode(who), _, Triple(_, _, Literal(age)), _):
        print(who, "claims", age)
```

## Formats

`RdfFormat` names a serialization of triples or quads:
- `TURTLE`, `N_TRIPLES`, `N_QUADS`, `TRIG`, `N3`, `RDF_XML`, `JSON_LD`
- attributes: `name`, `iri`, `media_type`, `file_extension`, `supports_datasets`
- `RdfFormat.from_media_type(...)` and `RdfFormat.from_extension(...)` return a format or `None`

`QueryResultsFormat` names a SPARQL results serialization:
- `JSON`, `XML`, `CSV`, `TSV`
- the same attributes and constructors

Wherever a format is expected, you can also pass a media type or an extension as a string (`"text/turtle"`,
`"ttl"`). When a `path` is given and the format is not, the format is inferred from the path's extension.

## Store

```python
Store(
    path=None,              # a SQLite file, an existing directory (uses oxilite.sqlite), or None: in memory
    *,
    library=None,           # a SQLite shared library to load instead of the bundled SQLite
    graph_index=True,       # the optional graph-first index (fast GRAPH <g> {…}, CLEAR GRAPH)
    text_index=False,       # the FTS5 index behind oxl:textMatch
    versioning="off",       # "off", "stamped" or "log"
    as_of_index=False,      # with "log": faster as-of queries
    stamp_index=False,      # with "stamped" or "log": index the tick that added each quad
    system_graphs=False,    # install <oxilite:vocabulary> and the registry's description in a blank store
)
Store.read_only(path)       # an existing store, read-only: writes raise OSError
```

The options after `library` apply when the database is created. An existing store keeps its schema and its
versioning level. Change the level with `set_versioning`.

### Quad access

| Method | Does |
|---|---|
| `add(quad)` | Adds a quad (a `Triple` goes to the default graph) |
| `extend(quads)` | Adds quads atomically |
| `bulk_extend(quads)` | Adds quads in chunks, which is not atomic, then refreshes planner statistics |
| `remove(quad)` | Removes a quad |
| `quad in store` | Membership |
| `len(store)` | Number of quads |
| `iter(store)` | Every quad |
| `quads_for_pattern(s, p, o, g=None)` | Quads matching a pattern: `None` matches anything, a `None` graph matches every graph, and `DefaultGraph()` matches only the default graph |

### Graphs and maintenance

| Method | Does |
|---|---|
| `named_graphs()` | Iterator of named graphs |
| `contains_named_graph(g)` | Whether a named graph exists (also when empty) |
| `add_graph(g)` | Declares an empty named graph |
| `clear_graph(g)` | Removes a graph's quads, and keeps the graph |
| `remove_graph(g)` | Removes a named graph and its quads |
| `clear()` | Empties the store |
| `flush()` | Does nothing: SQLite commits are durable. Kept for pyoxigraph |
| `optimize()` | Refreshes planner statistics. Run it after large imports |
| `backup(path)` | Writes a consistent copy with `VACUUM INTO` (to `oxilite.sqlite` when `path` is a directory) |
| `Store.schema_sql(graph_index=True, text_index=False, versioning="off")` | The schema as a SQL script, for example a Cloudflare D1 migration |

## SPARQL queries and updates

```python
store.query(query, *,
    base_iri=None, prefixes=None,                       # {"ex": "http://example.org/"}
    use_default_graph_as_union=False,
    default_graph=None, named_graphs=None,              # a graph or a list of graphs
    substitutions=None,                                 # {Variable("s"): term}: pre-bound variables
    reasoning=None,                                     # "rdfs" or "owl-ql": query-time entailment
    include_inferred=False,                             # also match materialized inferences
    include_schema_graphs=None,                         # False: hide registered ontology and shapes graphs
    as_of=None,                                         # "HEAD~1", "#42", "@2026-09-01T00:00:00Z"
)
store.update(update, *, base_iri=None, prefixes=None)  # atomic
```

`query` returns one of three result types:

| Query form | Result | How to read it |
|---|---|---|
| `SELECT` | `QuerySolutions` | An iterator of `QuerySolution`. `variables` is the list of `Variable`s. A solution is read by index (`s[0]`), by name (`s["x"]`), by `Variable`, by unpacking, or with `s.get("x", default)`. An unbound value is `None` |
| `ASK` | `QueryBoolean` | Use it as a `bool` |
| `CONSTRUCT`, `DESCRIBE` | `QueryTriples` | An iterator of `Triple` |

Each result has `serialize(output=None, format=…)`, which writes JSON, XML, CSV or TSV (an RDF format for
triples). It returns bytes when `output` is `None`:

```python
from oxilite import QueryResultsFormat

data = store.query("SELECT ?s WHERE { ?s ?p ?o }").serialize(format=QueryResultsFormat.JSON)
```

Results are decoded when the query returns. For very large SELECTs, page with `LIMIT` and `OFFSET`.

## Loading and dumping

```python
store.load(input=None, format=None, *, path=None, base_iri=None, to_graph=None)        # atomic
store.bulk_load(input=None, format=None, *, path=None, base_iri=None, to_graph=None)   # chunked, then statistics
store.dump(output=None, format=None, *, from_graph=None)                              # bytes when output is None
```

- **`input`** is a `str`, `bytes`, or a text or binary file object. Or pass `path=`, which Rust reads
  directly.
- **`output`** is a binary file object or a path.
- **Dataset formats** (N-Quads, TriG, JSON-LD) dump the whole store. A graph format (Turtle, N-Triples,
  RDF/XML) needs `from_graph`.
- **Syntax errors** raise `SyntaxError` with `filename`, `lineno` and `offset`.

```python
from oxilite import Store, RdfFormat, NamedNode

store = Store()
store.load(path="dump.nq")                                  # format from the extension
store.load("<a> <b> <c> .", RdfFormat.TURTLE, base_iri="http://ex/", to_graph=NamedNode("http://ex/g"))
store.dump("copy.trig")                                     # TriG, from the extension
turtle = store.dump(format=RdfFormat.TURTLE, from_graph=NamedNode("http://ex/g"))
```

## Parsing and serializing without a store

```python
from oxilite import parse, serialize, parse_query_results, RdfFormat

quads = list(parse("<http://ex/a> <http://ex/b> 1 .", RdfFormat.TURTLE))
data = serialize(quads, format=RdfFormat.N_QUADS)           # bytes
results = parse_query_results(path="results.srj")           # QuerySolutions or QueryBoolean
```

`parse` accepts `base_iri`, `without_named_graphs`, `rename_blank_nodes` and `lenient` (which accepts
relative IRIs and invalid language tags).

## Explain

Every language has an explain method. Each returns text for a person to read.
- **`store.explain(sparql)`**: the SQL, the join order and the planner's estimates. When the compiler hands
  part of a query to the fallback evaluator, the text says why.
- **`store.explain_update(update)`**: the SQL of each operation of an update.
- **`store.explain_cypher(query, params=None, **options)`**: the SPARQL a Cypher statement lowers to, the
  SQL, and the steps that run in Rust.
- **`store.explain_datalog(program)`**: the strata, the strategy for each recursive component, and the SQL.

## openCypher

```python
result = store.cypher(query, params=None, *,
    base=None,                   # namespace of labels, types and keys without a prefix (default urn:oxilite:pg:)
    prefixes=None, names=None,   # {"schema": "http://schema.org/"}, {"knows": "http://xmlns.com/foaf/0.1/knows"}
    multi_value=None,            # "list" (default), "first" or "error"
    var_length_cap=None, shortest_path_cap=None,
    shapes=None,                 # check writes against the SHACL shapes (default True)
    as_of=None,                  # read a past version (writes are refused)
    node_marker=None,            # give created nodes rdf:type rdfs:Resource (default True)
    reasoning=None,              # "rdfs" / "owl-ql": labels follow class hierarchies
    use_default_graph_as_union=None,
)
```

`CypherResult` has four fields:
- `columns`
- `rows`: lists of values
- `records`: dicts keyed by column
- `stats`: a `CypherStats` with `nodes_created`, `nodes_deleted`, `relationships_created`,
  `relationships_deleted`, `properties_set`, `labels_added` and `labels_removed`

Values are Python values: `None`, `bool`, `int`, `float`, `str`, lists and dicts. Graph values are
dataclasses:
- `CypherNode(id, labels, properties)`, where `id` is the node's IRI
- `CypherRelationship(id, rel_type, start, end, properties)`
- `CypherPath(nodes, relationships)`
- `CypherTemporal(type, value, kind)`, a date, time, datetime or duration in ISO 8601 form

Parameters are JSON values. `datetime`, `date` and `time` objects are passed as ISO strings, and terms as
their value.

```python
opts = {"base": "http://example.org/"}
store.cypher("CREATE (:Person {name: 'Grace'})-[:KNOWS {since: 1950}]->(:Person {name: 'Ada'})", **opts)
r = store.cypher("MATCH (a:Person {name: $name})-[:KNOWS]->(b) RETURN b.name AS friend", {"name": "Grace"}, **opts)
assert r.records == [{"friend": "Ada"}]
```

A writing statement is atomic. Whatever Cypher writes is RDF, so SPARQL sees it, and Cypher sees data
written with SPARQL.

## Datalog

```python
result = store.datalog(program, *, use_default_graph_as_union=None, include_inferred=None,
                       max_iterations=None, as_of=None)
stats = store.datalog_materialize(program, *, use_default_graph_as_union=None,
                                  include_inferred=None, max_iterations=None)
```

- **Results.** `DatalogResult` has `columns` (the goal's variables), `rows` (terms, or `None` for an
  unbound column), `records` (dicts), and `rounds`: how many rounds each iterated component took. `rounds`
  is empty when every recursion ran as a single recursive CTE.
- **Materialization.** `datalog_materialize` stores the conclusions as inferences, and returns
  `DatalogMaterializeResult(inferred, relations)`. SPARQL and Cypher see them with `include_inferred=True`.
  It shares the inference set with `materialize()`: running either one replaces the set.

```python
program = """
@prefix ex: <http://example.org/> .
anc(?x, ?y) :- ex:parent(?x, ?y).
anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).
?- anc(ex:ada, ?who).
"""
for record in store.datalog(program).records:
    print(record["who"])
```

An unsafe or unstratified program raises `ValueError`, and a syntax error raises `SyntaxError`.

## Reasoning

There are two ways to reason:
- **At query time.** `query(..., reasoning="rdfs")` or `reasoning="owl-ql"` rewrites the query over the
  class and property hierarchies. Nothing is stored.
- **By materialization.** `materialize(engine="sql")` computes the OWL 2 RL closure with SQL rules;
  `engine="reasonable"` computes it in memory, faster, with the same result. Either one returns the number
  of inferred triples, which are stored apart from the data. Queries see them with `include_inferred=True`.
  `clear_inferences()` removes them.

Once an ontology graph is registered (see below), reasoning uses only the registered ontologies, for the
graphs they apply to.

## Schema registry and system graphs

The registry records which graphs hold ontologies, SHACL shapes or ShEx schemas, and which graphs each
applies to. It is itself RDF, stored in the system graph `<oxilite:schema>`.

```python
store.register_schema_graph(graph, role, *,          # role: "ontology", "shacl" or "shex"
    iri=None, version=None, sha256=None, imports=None,
    applies_to=None,                                 # graphs (terms, IRIs, DefaultGraph()); empty: every graph
    active=True)
store.schema_graphs()                                # list[SchemaGraphEntry]
store.set_schema_graph_active(graph, active)         # bool: was it registered?
store.unregister_schema_graph(graph)                 # keeps the triples
store.drop_schema_graph(graph)                       # removes the triples; returns how many
store.shape_index()                                  # list[PropertyShapeEntry]: the compiled SHACL constraints
store.install_system_graphs()                        # adds <oxilite:vocabulary> and the registry's description
```

- **Graph arguments** are terms or IRI strings. The constant `DEFAULT_GRAPH_IRI` names the default graph in
  `applies_to`.
- **`SchemaGraphEntry`** has `graph`, `role`, `iri`, `version`, `sha256`, `imports`, `applies_to`, `active`
  and `loaded_at`.
- **`PropertyShapeEntry`** has `target`, `path`, `datatype`, `min_count`, `max_count`, `pattern`,
  `values_in` and `relationship`.
- **`query(..., include_schema_graphs=False)`** answers over the data alone, without the registered
  graphs.

## JSON-LD documents

```python
docs = store.jsonld(*,
    key=None,             # "id" (default), "contentHash", "explicit" or {"pointer": "/a/b"}
    on_missing_key=None,  # "reject" (default) or "contentHash"
    graph=None,           # "key" (default), "default", {"template": "https://ex/g/{key}"} or {"fixed": iri}
    base_iri=None, rdf_direction=None, processing_mode=None,
    contexts=None,        # {iri: context}, available in memory
    network=False,        # download unknown remote contexts (otherwise: JsonLdError)
    cache_fetched=False,  # persist downloaded contexts
    indexes=None)         # {"issuer": bool, "subject": bool, "valid_until": bool}
```

Each document is stored byte for byte under a key. Its RDF goes in a named graph (by default the key),
which SPARQL queries like any other graph.

| Method | Does |
|---|---|
| `put(document, key=None)` | Stores a document (JSON text, bytes, or a dict) and returns its key. Replaces a document with the same key |
| `put_all(documents)` | Stores several documents in one atomic write. Items are documents or `(document, key)` pairs |
| `get(key)` | The `StoredDocument`, or `None` |
| `remove(key)` | Removes the document and the graphs it owns. Returns `False` when it was not stored |
| `list(after=None, limit=100)` | Documents ordered by key, with keyset paging |
| `find(issuer=, subject=, type=, valid_at=, profile=, after=, limit=)` | Documents by metadata (indexed) |
| `graphs(key)` | The graphs a document owns |
| `document_for_graph(graph)` | The document behind a graph, such as a `?g` bound by SPARQL |
| `put_context(iri, context)`, `remove_context(iri)`, `contexts()` | Contexts persisted in the store, for offline loading |
| `check()` | `Drift(key, missing, extra)` for each document whose graphs no longer match its JSON |
| `rebuild(key)` | Regenerates a document's graphs from its JSON |

`StoredDocument` fields:
- `key`, `graph`
- `json`: the exact stored text; `document` is the same, parsed
- `sha256`, `profile`
- `issuer`, `subject`, `types`, `refs`
- `valid_from`, `valid_until`, `stored_at`: timezone-aware datetimes

## Verifiable Credentials

```python
vcs = store.credentials()                        # the jsonld options, plus embed_credentials=True
key = vcs.put(credential_json)                   # checks the VCDM structure; the key is the credential id
keys = vcs.put_presentation(presentation_json)   # PresentationKeys(key, credentials)
vcs.find(issuer="did:example:issuer", valid_at=datetime.now(timezone.utc))
vcs.get(key); vcs.remove(key); vcs.documents     # the JSON-LD operations above
```

Credentials of VCDM 1.1 and 2.0 are stored under their `id`, with their RDF in the graph of the same IRI
and each proof in a graph of its own. The W3C contexts are bundled, so no network access is needed. Proofs
are not verified: check them on the stored JSON with a verifier library.

## Versioning and time travel

```python
store = Store("data.sqlite", versioning="log")   # an immutable change log
with store.commit(author="ada", message="import"):
    store.load(path="people.ttl")
store.query("SELECT …", as_of="HEAD~1")         # also Cypher and Datalog: as_of=…
store.history(limit=20)                         # list[CommitRecord], newest first
store.changes(after=0, until=None)              # list[Change(tick, added, quad)]
store.diff("HEAD~1", "HEAD")                    # the net change between two versions
store.purge(subject=NamedNode("http://ex/p"), reason="erasure request")   # from the present and the past
store.versioning()                              # VersionStatus
store.set_versioning("stamped", allow_loss=False)
store.set_commit_info(author="bot")             # until changed; commit() sets it for a block
```

- **Levels.**
  - `off`: the default.
  - `stamped`: a store clock; each quad records the tick that added it.
  - `log`: every change recorded, with time travel.
- **Version references.**
  - `HEAD`
  - `HEAD~n`
  - `#tick`
  - `@2026-09-01T12:00:00Z`: the version current at that instant
- **The history is also RDF,** in the graph `<oxilite:history>`, which SPARQL can query.

## Full-text search

Create the store with `text_index=True`, then match literals with FTS5:

```python
store = Store(text_index=True)
store.query('''PREFIX oxl: <https://oxilite.dev/ns#>
SELECT ?doc WHERE { ?doc <http://ex/text> ?t FILTER(oxl:textMatch(?t, "graph data*")) }''')
```

Without the index, the same query still answers, through the fallback evaluator.

## Your own SQLite library

```python
store = Store("data.sqlite", library="/usr/lib/x86_64-linux-gnu/libsqlite3.so.0")
```

oxilite loads the library at runtime and uses only the stable C API. Any SQLite 3.37 or later works: a
system build, a vendor build, or one with encryption. The database file is the same either way.

## Threads and the GIL

Store methods release the GIL while SQLite works, so other Python threads keep running. A `Store` can be
shared between threads. Writes queue behind one another, as SQLite allows only one writer. For asyncio,
wrap calls in `asyncio.to_thread`.

## Exceptions

| Cause | Exception |
|---|---|
| SPARQL, RDF, results, Cypher or Datalog syntax | `SyntaxError` (with `filename`, `lineno` and `offset` for RDF and results files) |
| Invalid IRI, language tag, argument, option, or program | `ValueError` |
| SQLite, file, read-only or corrupted store | `OSError` |
| JSON-LD or credentials | `oxilite.JsonLdError`, a subclass of `ValueError`. `code` is the JSON-LD error code (`"loading remote context failed"`, `"invalid local context"`…) or one of `"missing-key"`, `"invalid-graph-name"`, `"graph-owned"`, `"document-too-large"`, `"invalid"`, `"json"` |
| Something the compiler or backend cannot do | `NotImplementedError` |
| Evaluation errors and hash collisions | `RuntimeError` |

## Differences from pyoxigraph

The pyoxigraph test suite (`test_store.py`, `test_model.py`, `test_io.py`) runs against oxilite on every
build. It fails in three places, which are allow-listed in `testsuite/allowlist.toml`:

| pyoxigraph feature | oxilite |
|---|---|
| `custom_functions`, `custom_aggregate_functions` | `NotImplementedError`. A query is one SQL statement run by SQLite, which cannot call Python for each row or group |
| `update("LOAD <https://…>")` | `NotImplementedError`. The core does no network I/O. Download the document and `load` it |

Other differences are by design:
- `Store(path)` is a SQLite file, not a RocksDB directory (a directory holds `oxilite.sqlite`).
- `flush` does nothing.
- `backup` writes a single file.
- `default_graph=[…]` builds the RDF merge of the listed graphs with set semantics, as `FROM` does: a triple
  in two of them matches once.
