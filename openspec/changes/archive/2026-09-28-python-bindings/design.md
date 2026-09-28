# Python bindings: design

## Context

`@oxilite/node` sets the pattern for bindings. A napi-rs class wraps `blocking::Store` on rusqlite or on a
dlopen'ed `libsqlite3`. Everything that carries RDF crosses the boundary as JSON: terms, quads, options and
results. The JSON forms are shared with the wasm engine (`oxilite_core::json` and the `json` modules of the
Cypher, Datalog and JSON-LD crates). A TypeScript wrapper then turns them into RDF/JS objects. This split
keeps the native code small, and `@oxilite/node` and `@oxilite/d1` return identical values.

Python has its own reference API, `pyoxigraph`, which is Oxigraph's Python package. It is the contract here,
as Oxigraph's JavaScript `Store` was for Node. Its tests (`python/tests/test_store.py`, `test_model.py` and
`test_io.py`) define the behaviour users rely on.

## Goals / Non-goals

**Goals**
- `import oxilite as pyoxigraph` works for code written against pyoxigraph's store, terms, formats, results
  and I/O functions.
- Every oxilite capability is reachable from Python with typed results. That includes Cypher, Datalog,
  reasoning, the schema registry, JSON-LD and credentials, versioning, text search, and a user-supplied
  SQLite.
- One wheel per platform covers every CPython from 3.9 up, and a release needs only a tag push.

**Non-goals**
- Python callbacks inside SPARQL evaluation (pyoxigraph's `custom_functions` and
  `custom_aggregate_functions`). Queries compile to SQL, and a Python function cannot run inside SQLite's
  statement without a per-row round trip across the GIL. These options raise `NotImplementedError`.
- An asyncio API. Calls release the GIL, so `asyncio.to_thread` or a thread pool gives concurrency.
- A D1 client in Python. D1 is reached from Workers (Rust or TypeScript).
- PyPy and free-threaded CPython wheels. The `abi3` stable ABI does not cover them. Users can build from the
  sdist.

## Decisions

### D1. PyO3 and maturin, stable ABI (`abi3-py39`)

PyO3 is the standard Rust–Python bridge, and maturin builds and publishes its wheels. With the `abi3-py39`
feature, the module links against CPython's stable ABI. One wheel per platform then serves 3.9 through 3.14
and later, so the release matrix is five platform wheels and not five platforms times six Pythons.

Alternatives: cffi or ctypes over a C ABI, which means writing the C ABI; or hand-written CPython
extensions. Neither is safer or smaller.

### D2. JSON across the boundary, pure-Python terms

The native module, `oxilite._native`, exposes one class, `NativeStore`, whose methods mirror
`@oxilite/node`'s `NativeStore`. Arguments and results that carry RDF are JSON text in the shared forms. A
few functions stand on their own: `parse`, `serialize`, `serialize_results`, `parse_query_results`,
`schema_sql`, and validation helpers. The terms (`NamedNode`, `Literal` and the rest) are plain Python
classes with `__slots__`. They are immutable, hashable, picklable, and have `__match_args__`.

Why:
- **One wire format.** A query returns the same values on Node, D1 and Python. New store features reach
  Python by adding a method that forwards JSON, with no new conversion code.
- **Speed.** Python's `json` module is C code. Decoding a row of terms costs about as much as building the
  objects from Rust, and the SQLite query dominates.
- **Plain objects.** Pure-Python terms can be pickled and copied, work with pattern matching, and type-check
  without PyO3 class machinery.

Cost: a result is materialized before Python sees it. `QuerySolutions` iterates over a decoded list, as
`@oxilite/node` returns an array. oxilite already evaluates each query as one SQL statement and collects its
rows, so the result is materialized either way.

### D3. The pyoxigraph surface, then Pythonic extensions

Names, positional and keyword arguments, result classes and exception types follow pyoxigraph:
- `Store(path)`, `store.query(q, use_default_graph_as_union=…)`
- `load(input, format, *, path, base_iri, to_graph)`
- `dump(output, format, *, from_graph)`
- `QuerySolution[0 | "s" | Variable("s")]`

oxilite's extensions use snake_case keyword arguments, with the same meanings as the Node option names:
- `reasoning`, `include_inferred`, `include_schema_graphs` and `as_of` on `query`
- `cypher(query, params, *, base, prefixes, …, as_of)`
- `datalog(program, *, use_default_graph_as_union, include_inferred, max_iterations, as_of)`
- `register_schema_graph(graph, role, *, applies_to, …)`

Results are frozen dataclasses: `CypherResult`, `CypherNode`, `DatalogResult`, `StoredDocument`,
`VersionStatus`, `CommitRecord`, `Change`, `SchemaGraphEntry` and `PropertyShapeEntry`. Epoch-second
timestamps become timezone-aware `datetime`s.

Commit metadata gets a context manager, `with store.commit(author=…, message=…):`, which is the Python form
of `withCommit`.

### D4. Store locations

`Store()` is in memory. `Store("data.sqlite")` opens or creates that file.

pyoxigraph's paths are directories (RocksDB). When the path is an existing directory, oxilite uses
`oxilite.sqlite` inside it, so pyoxigraph code that passes a directory keeps working. `Store.read_only(path)`
opens an existing store with SQLite's read-only flag. Writes then fail with `OSError`.

`library=` loads a SQLite shared library through `oxilite-dylib`, as `@oxilite/node` does.

### D5. The GIL is released during native work

Every `NativeStore` method converts its arguments, then runs the store operation inside `py.detach(...)`,
which releases the GIL. It converts the result after taking the GIL back. `Store<B>` is `Send + Sync`, and
the backend serializes access to its connection, so Python threads can share one store. SQLite's
single-writer rule still holds: writes queue behind one another.

### D6. Exceptions

| Cause | Python exception |
|---|---|
| SPARQL, RDF, query-results, Cypher or Datalog syntax | `SyntaxError`, with `filename`, `lineno`, `offset`, `end_lineno` and `end_offset` when the parser reports a location |
| Invalid IRI, blank node id, language tag or argument | `ValueError` |
| SQLite, I/O, read-only or corrupted store | `OSError` |
| JSON-LD or credentials | `oxilite.JsonLdError`, a subclass of `ValueError`, with `.code` |
| Unsupported by the compiler or the backend | `NotImplementedError` |
| Anything else (evaluation, hash collision) | `RuntimeError` |

The native module raises these exceptions itself: it matches on the Rust error enums, and for JSON-LD it
raises its own `JsonLdError` class with `code` set. The Python side never parses message text.

### D7. pyoxigraph options with no Oxigraph evaluator behind them

- **`prefixes`** is passed to `SparqlParser::with_prefix`.
- **`substitutions`** (`{Variable: term}`) joins a one-row `VALUES` into the query's algebra. The join goes
  below the solution modifiers, the projection, the grouping, and the `BIND`s and `FILTER`s that wrap the
  pattern. This matches pyoxigraph's "bind before evaluation" semantics: a substituted variable is fixed
  before any expression sees it. The compiler then treats it like any other `VALUES`, so the query is still
  one SQL statement.
- **`custom_functions` and `custom_aggregate_functions`** raise `NotImplementedError` (see Non-goals). The
  two pyoxigraph tests for them are allow-listed.

### D8. Terms are validated when built by the user, not when decoded

The constructors check their input through native helpers that call `oxrdf`: `NamedNode` checks the IRI,
`BlankNode` the id, and `Literal` the language tag. So a bad IRI raises `ValueError` where it is written, as
in pyoxigraph. Terms decoded from store results skip the checks through a private constructor, because the
store only returns valid terms.

`Literal(True)`, `Literal(1)` and `Literal(0.1)` build `xsd:boolean`, `xsd:integer` and `xsd:double`
literals. The double's lexical form is the XSD 1.1 canonical one (`1.0E-1`, `INF`, `NaN`), which is
what pyoxigraph's tests expect. It is computed in Rust.

### D9. I/O arguments

- **Inputs** to `load`, `bulk_load`, `parse` and `parse_query_results` can be `str`, `bytes`, a text or binary
  file object (read in full), or `path=`. A path is read by Rust and never copied through Python. The format
  can be inferred from the path's extension.
- **Outputs** of `dump`, `serialize` and `QuerySolutions.serialize` can be `None` (the bytes are returned), a
  binary file object (written), or a path (written by Python).
- **I/O errors** raised by the file object (`UnsupportedOperation` on a write-only file) propagate unchanged,
  as pyoxigraph's tests expect.

### D10. Packaging and release

- **Layout:** `bindings/python/` holds `Cargo.toml` (the crate `oxilite-python`, `crate-type = ["cdylib"]`,
  `publish = false`) and `pyproject.toml` (build backend maturin, `module-name = "oxilite._native"`,
  `python-source = "python"`). `python/oxilite/` holds the package, with `py.typed` and `_native.pyi`, and
  `tests/` holds the tests.
- **Version:** `dynamic = ["version"]`, so the version comes from the workspace's `Cargo.toml`. A release
  bump then covers PyPI too.
- **Wheels:** `.github/workflows/python-wheels.yml` builds with `PyO3/maturin-action` for:
  - `manylinux` x86_64 and aarch64
  - `musllinux` x86_64
  - macOS x86_64 and arm64
  - Windows x64
  - an sdist

  It smoke-tests each wheel, and on a `v*` tag publishes everything with `pypa/gh-action-pypi-publish` and
  PyPI trusted publishing, with no stored token.
- **Manual path:** `maturin publish` with a PyPI token, documented in `docs/python-publishing.md`.

### D11. Tests

- **Runner:** pytest in `bindings/python/tests`.
- **The port:** `test_pyoxigraph_store.py`, `test_pyoxigraph_model.py` and `test_pyoxigraph_io.py` are
  pyoxigraph's files, verbatim except for the import.
- **Divergences:** `conftest.py` reads the `py:` entries of `testsuite/allowlist.toml` and marks those tests
  `xfail(strict=True)`. A new divergence therefore fails, and a stale entry fails too, which is the rule the
  Node checker enforces.
- **oxilite's own tests** cover every extension, each tied to a `lat.md/tests.md` section by a `# @lat:`
  comment.

## Risks / Trade-offs

- **Materialized results use memory on huge SELECTs.** Mitigation: document `LIMIT` and `OFFSET` paging, as
  for Node.
- **pyoxigraph evolves.** The port pins the upstream commit it was copied from, as the JS port does.
- **`abi3` gives up some speed**, because some APIs are not available under the stable ABI. PyO3 falls back
  to slower calls there. JSON crossing dominates anyway.
- **manylinux builds compile a bundled SQLite and rustls.** Neither needs system libraries, so the wheels are
  self-contained.
