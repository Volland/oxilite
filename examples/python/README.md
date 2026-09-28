# oxilite Python examples

Short, self-contained scripts for the [`oxilite`](https://pypi.org/project/oxilite/) Python package. Each
one runs top to bottom, prints what it does and asserts its output. `bindings/python/tests/test_examples.py`
runs all of them in CI.

```bash
pip install oxilite
python examples/python/01_quickstart.py
```

| Script | Shows |
|---|---|
| [`01_quickstart.py`](01_quickstart.py) | A store in one SQLite file: load Turtle, add quads, `SELECT` / `ASK` / `CONSTRUCT`, results as CSV or dicts, dump, reopen |
| [`02_cypher_property_graph.py`](02_cypher_property_graph.py) | openCypher writes and reads, parameters, variable-length paths, aggregation, `DETACH DELETE`, and SPARQL over the same data |
| [`03_datalog.py`](03_datalog.py) | Recursive rules compiled to one SQL statement, stratified negation, aggregation, and materialized inferences |
| [`04_reasoning_and_schemas.py`](04_reasoning_and_schemas.py) | RDFS at query time, OWL 2 RL materialization, a registered ontology, SHACL shapes that refuse a Cypher write |
| [`05_jsonld_and_credentials.py`](05_jsonld_and_credentials.py) | JSON-LD documents kept byte for byte and queried as graphs; Verifiable Credentials found by issuer, subject and validity |
| [`06_time_travel.py`](06_time_travel.py) | A change log with commits, queries at `HEAD~n`, history, diffs, and purging a subject from every version |
| [`07_full_text_search.py`](07_full_text_search.py) | An FTS5 index over literals, used from SPARQL with `oxl:textMatch` |
| [`08_threads_and_asyncio.py`](08_threads_and_asyncio.py) | One store shared by a thread pool, `asyncio.to_thread`, and a read-only handle |

For everything in one script, see [`../python-tour/tour.py`](../python-tour/tour.py), the code of the
article [How to use oxilite with Python](https://oxilitedb.com/articles/oxilite-python). The full API is in
[docs/python.md](../../docs/python.md).
