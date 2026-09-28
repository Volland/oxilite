Status: implemented (2026-09-28). Publishing to PyPI waits for the one-time trusted-publisher setup in
`docs/python-publishing.md`; archive the change after the first release.

## 1. Native module

- [x] 1.1 `bindings/python` crate `oxilite-python` (PyO3, `abi3-py39`, cdylib), workspace member, not published
- [x] 1.2 `NativeStore` over `blocking::Store` (bundled SQLite and dylib; directory paths; read-only)
- [x] 1.3 JSON methods mirroring `@oxilite/node`: query (with `prefixes` and `substitutions`), update, load, dump, add, remove, contains, match, len, named graphs, explain, Cypher, Datalog, reasoning, schema registry, JSON-LD and credentials, versioning, backup
- [x] 1.4 Module functions: `parse`, `serialize`, `serialize_results`, `parse_query_results`, `schema_sql`, term validation, canonical doubles
- [x] 1.5 Error kinds mapped to Python exceptions; syntax errors carry locations
- [x] 1.6 GIL released during store work

## 2. Python package

- [x] 2.1 Terms and formats with pyoxigraph's API (immutable, hashable, picklable, `__match_args__`)
- [x] 2.2 `Store` and result classes (`QuerySolutions`, `QuerySolution`, `QueryBoolean`, `QueryTriples`)
- [x] 2.3 `parse`, `serialize`, `parse_query_results` with str / bytes / file / path inputs
- [x] 2.4 Extensions with typed dataclasses: Cypher, Datalog, reasoning, schema registry, JSON-LD and credentials, versioning (`commit` context manager)
- [x] 2.5 `py.typed`, `_native.pyi`; `mypy --strict` clean

## 3. Tests

- [x] 3.1 Port pyoxigraph `test_store.py`, `test_model.py`, `test_io.py` (import changed only); `py:` allow-list with strict xfail (112 passed, 3 allow-listed)
- [x] 3.2 oxilite suites: persistence, directory and read-only stores, options, reasoning, explain, Cypher, Datalog, registry, system graphs, JSON-LD, credentials, versioning, threads, text search, dylib
- [x] 3.3 CI job `python`: maturin build, pytest, mypy, the tutorial script

## 4. Packaging and release

- [x] 4.1 `pyproject.toml` (maturin backend, dynamic version, metadata, classifiers)
- [x] 4.2 `.github/workflows/python-wheels.yml`: wheels for manylinux x86_64/aarch64, musllinux x86_64, macOS x86_64/arm64, Windows x64, sdist; smoke tests; trusted publishing on tags
- [x] 4.3 Build and install a wheel locally (macOS arm64 wheel on CPython 3.9); build the sdist and install it from source

## 5. Docs and site

- [x] 5.1 `bindings/python/README.md` (PyPI page)
- [x] 5.2 `docs/python.md` reference; `docs/python-publishing.md` setup and publishing guide
- [x] 5.3 Main README: Python install, quick start, package table
- [x] 5.4 Website: landing-page Python section and Get started tab, article "How to use oxilite with Python" (asserted by `examples/python-tour/tour.py`), articles index, sitemap
- [x] 5.5 `lat.md`: architecture, decision D37, tests, milestone; `lat check`; `openspec validate --strict`
