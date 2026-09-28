## Why

Python is where most RDF and knowledge-graph work happens: data engineering, notebooks, ML pipelines and
agent frameworks. oxilite is reachable today from Rust, Node.js and Cloudflare Workers, but not from Python.
Oxigraph users there run `pyoxigraph`, a RocksDB store in a directory. Many would take a single SQLite file:
it is easy to copy, inspect, back up and ship. They should be able to switch by changing an import, as Node
users could switch from Oxigraph's JavaScript package.

## What Changes

- **A new package, `oxilite` on PyPI**, built with PyO3 and maturin from `bindings/python`. It is a native
  module (`oxilite._native`) wrapped by a typed, pure-Python API.
- **The pyoxigraph API, unchanged:**
  - terms (`NamedNode`, `BlankNode`, `Literal`, `DefaultGraph`, `Triple`, `Quad`, `Variable`,
    `BaseDirection`)
  - formats (`RdfFormat`, `QueryResultsFormat`)
  - results (`QuerySolutions`, `QuerySolution`, `QueryBoolean`, `QueryTriples`)
  - `parse`, `serialize` and `parse_query_results`
  - `Store`: `add`, `extend`, `bulk_extend`, `remove`, `in`, `len`, iteration, `quads_for_pattern`, `query`,
    `update`, `load`, `bulk_load`, `dump`, named-graph methods, `clear`, `flush`, `optimize`, `backup`,
    `Store.read_only`
- **Every oxilite feature beyond Oxigraph**, with Pythonic names and typed results:
  - `explain` and `explain_update`
  - openCypher: `cypher`, `explain_cypher`
  - Datalog: `datalog`, `datalog_materialize`, `explain_datalog`
  - reasoning: query-time `reasoning`, `materialize`, `clear_inferences`, `include_inferred`
  - the schema registry and the system graphs
  - JSON-LD documents and Verifiable Credentials
  - versioning: levels, commits, `as_of`, `history`, `changes`, `diff`, `purge`
  - FTS5 text search
  - a user-supplied `libsqlite3`
  - `schema_sql`
- **The pyoxigraph test suite:** `test_store.py`, `test_model.py` and `test_io.py` are ported with only the
  import changed. Any failure must be an allow-listed divergence (`py:` entries in
  `testsuite/allowlist.toml`).
- **Packaging and release:**
  - one `abi3` wheel per platform, for CPython 3.9 and later
  - a GitHub Actions workflow that builds wheels for Linux, macOS and Windows plus an sdist, and publishes to
    PyPI through trusted publishing
  - the package version comes from the Cargo workspace
- **Docs:**
  - a package README
  - a reference, `docs/python.md`
  - a setup-and-publishing guide, `docs/python-publishing.md`
  - README sections
  - a website section and an article: "How to use oxilite with Python"

## Capabilities

### New Capabilities
- `python-bindings`: oxilite for Python, with pyoxigraph's API plus every oxilite extension.

### Modified Capabilities
<!-- none -->

## Impact

- **Code:**
  - New `bindings/python`: a Rust crate `oxilite-python` (a workspace member, not published to crates.io),
    the Python package, and tests.
  - No change to the core or to other crates. The bindings reuse the JSON forms of `oxilite_core::json`,
    `oxilite_cypher::json`, `oxilite_datalog::json` and `oxilite_jsonld::json`, as `@oxilite/node` does.
- **CI:**
  - a `python` job: build with maturin, then run pytest and the pyoxigraph port
  - `python-wheels.yml` for releases
- **Release process:** PyPI joins crates.io and npm. The wheel workflow runs on version tags, so a release
  needs no local Python step.
- **Docs:** `lat.md` gains Python sections in the architecture, the tests and the milestones, and decision
  D37. The website gains a Python section and an article.
