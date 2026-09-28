<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite for Python

[![PyPI](https://img.shields.io/pypi/v/oxilite.svg)](https://pypi.org/project/oxilite/) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**An Oxigraph-compatible SPARQL 1.1 store for Python, on SQLite.** It has the API of
[pyoxigraph](https://pypi.org/project/pyoxigraph/) and keeps the whole dataset in one SQLite file. The same
data can also be queried with **openCypher** and **Datalog**, reasoned over with RDFS / OWL, described by
SHACL shapes, filled from JSON-LD documents and Verifiable Credentials, and read at any past version.

**[Website](https://oxilitedb.com)** · **[Python reference](https://github.com/Volland/oxilite/blob/main/docs/python.md)** · [Tutorial](https://oxilitedb.com/articles/oxilite-python) · [Source](https://github.com/Volland/oxilite) · [Issues](https://github.com/Volland/oxilite/issues)

```bash
pip install oxilite
```

## Quick start

```python
from oxilite import Store, NamedNode, Literal, Quad, RdfFormat

store = Store("data.sqlite")                     # Store() for an in-memory store
store.load("""
    @prefix ex: <http://example.com/> .
    ex:ada a ex:Person ; ex:name "Ada" ; ex:knows ex:alan .
""", RdfFormat.TURTLE)
store.add(Quad(NamedNode("http://example.com/alan"), NamedNode("http://example.com/name"), Literal("Alan")))

for solution in store.query("SELECT ?name WHERE { ?p <http://example.com/name> ?name }"):
    print(solution["name"].value)

store.update("DELETE WHERE { ?s <http://example.com/knows> ?o }")
print(len(store))
```

`query` returns what pyoxigraph returns: `QuerySolutions` for `SELECT`, `QueryBoolean` for `ASK`, and
`QueryTriples` for `CONSTRUCT` and `DESCRIBE`. Each can be serialized to JSON, XML, CSV or TSV (RDF for
triples).

## Coming from pyoxigraph

```python
import oxilite as pyoxigraph
```

Terms, formats, results, `parse`, `serialize`, `parse_query_results` and every `Store` method keep their
names, arguments and results. pyoxigraph's own test suite runs against this package on every build. Two
kinds of test fail, and they are allow-listed:
- custom Python functions inside SPARQL: a query is one SQL statement, so SQLite cannot call Python for
  each row
- `LOAD` of a remote URL: the core does no network I/O

`Store(path)` opens a SQLite file. Given an existing directory, as pyoxigraph's paths are, it uses
`oxilite.sqlite` inside it.

## Cypher and Datalog over the same data

```python
opts = {"base": "http://example.com/"}
store.cypher("CREATE (:Person {name: 'Grace'})-[:KNOWS {since: 1950}]->(:Person {name: 'Ada'})", **opts)
r = store.cypher("MATCH (a:Person {name: $name})-[:KNOWS]->(b) RETURN b.name AS friend", {"name": "Grace"}, **opts)
print(r.records)                          # [{'friend': 'Ada'}]

program = """
@prefix ex: <http://example.com/> .
reach(?x, ?y) :- ex:KNOWS(?x, ?y).
reach(?x, ?z) :- ex:KNOWS(?x, ?y), reach(?y, ?z).
?- reach(?a, ?b).
"""
print(store.datalog(program).records)     # [{'a': <NamedNode …>, 'b': <NamedNode …>}]
```

Nodes are IRIs, labels are `rdf:type`, properties are literal triples, and relationships are triples,
with an RDF 1.2 reifier when they carry properties. So SPARQL sees everything Cypher writes. A Datalog
program whose recursion is linear runs as one recursive SQL statement.

## Reasoning, schemas, documents, history

```python
from datetime import datetime, timezone
from oxilite import DefaultGraph, NamedNode, RdfFormat, Store

store = Store()
store.load("@prefix ex: <http://example.com/> . ex:rex a ex:Dog .", RdfFormat.TURTLE)
onto = NamedNode("http://example.com/onto")
store.load("@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
           "ex:Dog rdfs:subClassOf ex:Animal .", RdfFormat.TURTLE, to_graph=onto)
store.register_schema_graph(onto, "ontology", applies_to=[DefaultGraph()])   # the schema registry

assert store.query("ASK { ex:rex a ex:Animal }", reasoning="rdfs", prefixes={"ex": "http://example.com/"})
store.materialize(engine="reasonable")            # OWL 2 RL closure; query it with include_inferred=True

docs = store.jsonld()                             # JSON-LD stored byte for byte, RDF in a named graph
docs.put('{"@context": {"name": "http://schema.org/name"}, "@id": "urn:uuid:1", "name": "Ada"}')
vcs = store.credentials()                         # Verifiable Credentials 1.1 and 2.0, indexed by
vcs.find(issuer="did:example:academy", valid_at=datetime.now(timezone.utc))  # issuer, subject, validity

versioned = Store(versioning="log")               # an immutable change log
with versioned.commit(author="ada", message="import"):
    versioned.update('INSERT DATA { <urn:t1> <urn:status> "open" }')
with versioned.commit(author="grace", message="close"):
    versioned.update('DELETE DATA { <urn:t1> <urn:status> "open" } ; INSERT DATA { <urn:t1> <urn:status> "done" }')
print([s["s"].value for s in versioned.query("SELECT ?s { <urn:t1> <urn:status> ?s }", as_of="HEAD~1")])  # ['open']
```

## Examples

Each example is a short script that runs top to bottom, prints what it does and asserts its output. CI
runs all of them against every build.

| Example | Shows |
|---|---|
| [`01_quickstart.py`](https://github.com/Volland/oxilite/blob/main/examples/python/01_quickstart.py) | A store in one file: load, add, `SELECT` / `ASK` / `CONSTRUCT`, results as CSV or dicts, dump, reopen |
| [`02_cypher_property_graph.py`](https://github.com/Volland/oxilite/blob/main/examples/python/02_cypher_property_graph.py) | openCypher writes and reads, parameters, paths, aggregation, and SPARQL over the same data |
| [`03_datalog.py`](https://github.com/Volland/oxilite/blob/main/examples/python/03_datalog.py) | Recursive rules, negation, aggregation, and materialized inferences |
| [`04_reasoning_and_schemas.py`](https://github.com/Volland/oxilite/blob/main/examples/python/04_reasoning_and_schemas.py) | RDFS at query time, OWL 2 RL materialization, registered ontologies and SHACL shapes |
| [`05_jsonld_and_credentials.py`](https://github.com/Volland/oxilite/blob/main/examples/python/05_jsonld_and_credentials.py) | JSON-LD documents and Verifiable Credentials, found by issuer, subject and validity |
| [`06_time_travel.py`](https://github.com/Volland/oxilite/blob/main/examples/python/06_time_travel.py) | Commits, queries at past versions, history, diffs, and purging |
| [`07_full_text_search.py`](https://github.com/Volland/oxilite/blob/main/examples/python/07_full_text_search.py) | FTS5 full-text search from SPARQL |
| [`08_threads_and_asyncio.py`](https://github.com/Volland/oxilite/blob/main/examples/python/08_threads_and_asyncio.py) | Thread pools, `asyncio.to_thread`, and read-only handles |
| [`python-tour/tour.py`](https://github.com/Volland/oxilite/blob/main/examples/python-tour/tour.py) | Everything above in one script: the code of the [Python tutorial](https://oxilitedb.com/articles/oxilite-python) |

## API at a glance

| Method | Does |
|---|---|
| `Store(path=None, *, library=None, text_index=False, versioning="off", …)`, `Store.read_only(path)` | Open a file, a directory or memory; `library` loads a system `libsqlite3` |
| `query(sparql, *, base_iri, prefixes, use_default_graph_as_union, default_graph, named_graphs, substitutions, reasoning, include_inferred, include_schema_graphs, as_of)` | SPARQL 1.1 / 1.2 query |
| `update(sparql)` | SPARQL Update, atomically |
| `load`, `bulk_load`, `dump` | Turtle, N-Triples, N-Quads, TriG, N3, RDF/XML, JSON-LD, from `str`, `bytes`, files or paths |
| `add`, `extend`, `bulk_extend`, `remove`, `in`, `len`, iteration, `quads_for_pattern` | Quad-level access |
| `named_graphs`, `add_graph`, `clear_graph`, `remove_graph`, `clear`, `optimize`, `backup` | Graphs and maintenance |
| `explain`, `explain_update`, `explain_cypher`, `explain_datalog` | The SQL each language compiles to |
| `cypher(query, params, **options)` | openCypher read or write: `CypherResult(columns, rows, records, stats)` |
| `datalog`, `datalog_materialize` | Recursive rules, stratified negation, aggregation |
| `materialize(engine)`, `clear_inferences()` | OWL 2 RL materialization (`"sql"` or `"reasonable"`) |
| `register_schema_graph`, `schema_graphs`, `set_schema_graph_active`, `unregister_schema_graph`, `drop_schema_graph`, `shape_index`, `install_system_graphs` | The schema registry |
| `jsonld(**options)`, `credentials(**options)` | JSON-LD documents and Verifiable Credentials |
| `versioning`, `set_versioning`, `commit`, `set_commit_info`, `history`, `changes`, `diff`, `purge` | History and time travel |
| `parse`, `serialize`, `parse_query_results` | RDF and results I/O without a store |

Every result is typed: the package ships `py.typed` and passes `mypy --strict`. Store calls release the
GIL. The full reference is [docs/python.md](https://github.com/Volland/oxilite/blob/main/docs/python.md).

## Platforms

Wheels use the stable ABI, so each one serves CPython 3.9 and every later version:
- Linux: glibc x86_64 and aarch64, musl x86_64
- macOS: x86_64 and arm64
- Windows: x64

On other platforms, `pip` builds the package from the source distribution, which needs a Rust toolchain.
To build it yourself, see
[building and publishing the package](https://github.com/Volland/oxilite/blob/main/docs/python-publishing.md).

## The oxilite family

oxilite is an Oxigraph-compatible RDF database and SPARQL 1.1 engine that stores its data in SQLite. It
runs anywhere SQLite runs: in-process, on a system or vendor `libsqlite3`, and on Cloudflare D1 and in
Durable Objects.

| Package | What it is for |
|---|---|
| [`oxilite`](https://crates.io/crates/oxilite) (Rust) | The store: a drop-in for `oxigraph::store::Store`, plus `AsyncStore` for D1 |
| [`oxilite`](https://pypi.org/project/oxilite/) (Python) | This package: the API of pyoxigraph |
| [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node) | Node.js bindings, with the API of Oxigraph's JS package |
| [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1) | Cloudflare D1 and Durable Objects from TypeScript |
| [`oxilite-cli`](https://crates.io/crates/oxilite-cli) | The `oxilite` command and a SPARQL endpoint like `oxigraph serve` |
| [`oxilite-cypher`](https://crates.io/crates/oxilite-cypher), [`oxilite-datalog`](https://crates.io/crates/oxilite-datalog) | openCypher and Datalog over the same data |
| [`oxilite-jsonld`](https://crates.io/crates/oxilite-jsonld), [`oxilite-vc`](https://crates.io/crates/oxilite-vc) | JSON-LD documents and Verifiable Credentials |

## License

Dual-licensed under [MIT](https://github.com/Volland/oxilite/blob/main/LICENSE-MIT) or
[Apache-2.0](https://github.com/Volland/oxilite/blob/main/LICENSE-APACHE), at your option, like Oxigraph.
