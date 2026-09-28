# Building and publishing the Python package

This guide takes you from a fresh checkout to a published `oxilite` release on PyPI. The package lives in
[`bindings/python`](../bindings/python). For its API, see [python.md](python.md).

It covers five things:
1. Setting up a development environment.
2. Building and testing the package.
3. Building release wheels locally.
4. The one-time PyPI setup.
5. Releasing, either automatically from a tag or by hand.

## How the package is put together

| Piece | Where | Role |
|---|---|---|
| Native module `oxilite._native` | `bindings/python/src/lib.rs` (Rust crate `oxilite-python`, PyO3) | Wraps `oxilite::blocking::Store`, exchanging JSON with Python |
| Python package `oxilite` | `bindings/python/python/oxilite/` | pyoxigraph's API: terms, formats, results, `Store`, typed extension results |
| Type stubs | `python/oxilite/_native.pyi`, `py.typed` | Strict type checking for users |
| Build configuration | `bindings/python/pyproject.toml` | maturin build backend; the version comes from the Cargo workspace |
| Tests | `bindings/python/tests/` | pyoxigraph's test suite (ported verbatim) and oxilite's own tests |
| CI | `.github/workflows/ci.yml`, job `python` | Builds, tests and type-checks on Linux and macOS for every push |
| Release | `.github/workflows/python-wheels.yml` | Builds every wheel and the sdist; publishes on `v*` tags |

The wheels use CPython's stable ABI (`abi3`, Python 3.9 and later), so each platform needs one wheel,
whatever the Python version. SQLite is compiled into the wheel, and the HTTP client used for JSON-LD
contexts uses rustls, so a wheel needs no system library.

## 1. Set up a development environment

You need three things:
- **Rust**: the stable toolchain from [rustup](https://rustup.rs). The workspace needs rustc 1.88 or
  later.
- **Python**: 3.9 or later. Use a recent one for development, such as 3.12.
- **A C compiler**, for the bundled SQLite: Xcode command-line tools on macOS, `build-essential` on
  Debian or Ubuntu, or the Visual Studio Build Tools on Windows.

```bash
git clone https://github.com/Volland/oxilite.git
cd oxilite/bindings/python
python3 -m venv .venv
source .venv/bin/activate            # Windows: .venv\Scripts\activate
pip install maturin pytest mypy
```

> **Two `rustc`s on one machine.** If another `rustc` is also installed (for example Homebrew's), put
> rustup's first on your `PATH` with `export PATH="$HOME/.cargo/bin:$PATH"`. Otherwise the build can fail
> with `rustc … is not supported`.

## 2. Build and test

1. Build the package into the virtual environment in editable mode:

   ```bash
   maturin develop            # debug build: fast to compile
   maturin develop --release  # optimized build, for benchmarks
   ```

   After changing Python files, you need not rebuild. After changing `src/lib.rs`, run `maturin develop`
   again.

2. Run the tests:

   ```bash
   python -m pytest
   ```

   Expect about 130 passing tests and three expected failures (`xfail`). Those three are the pyoxigraph
   tests listed as `py:` entries in `testsuite/allowlist.toml`. The allow-list is strict in both
   directions:
   - an unlisted failure fails the run
   - a listed test that starts passing also fails the run, until you remove its entry

   To run the system-SQLite test on macOS, point it at the library:

   ```bash
   OXILITE_SQLITE_LIBRARY=/usr/lib/libsqlite3.dylib python -m pytest
   ```

   The run includes `tests/test_examples.py`, which runs every script in `examples/python/`, the
   tutorial's `examples/python-tour/tour.py`, and the code blocks of the package README (the page PyPI
   shows), so an API change that breaks an example fails the tests.

3. Type-check:

   ```bash
   mypy --strict python/oxilite
   ```

4. Lint the Rust side from the repository root:

   ```bash
   cargo fmt -p oxilite-python
   cargo clippy -p oxilite-python --all-targets
   ```

CI runs the same steps in the `python` job of `.github/workflows/ci.yml`.

## 3. Build release artifacts locally

```bash
cd bindings/python
maturin build --release --out dist     # the wheel for this machine: dist/oxilite-X.Y.Z-cp39-abi3-<platform>.whl
maturin sdist --out dist               # the source distribution: dist/oxilite-X.Y.Z.tar.gz
```

Check the wheel in a clean environment, preferably on the oldest supported Python:

```bash
python3.9 -m venv /tmp/oxilite-check
/tmp/oxilite-check/bin/pip install dist/oxilite-*.whl
/tmp/oxilite-check/bin/python -c "import oxilite; s = oxilite.Store(); s.update('INSERT DATA { <urn:a> <urn:b> <urn:c> }'); print(len(s), oxilite.__version__)"
```

Wheels for other platforms are built by the release workflow. maturin can cross-compile (`--target`,
`--zig`), but the workflow is the tested path.

## 4. One-time PyPI setup

The release workflow publishes with PyPI **trusted publishing**: PyPI trusts this repository's workflow,
and no API token is stored anywhere.

1. **Create the accounts.** You need an account on [pypi.org](https://pypi.org/account/register/) with
   two-factor authentication. For rehearsals, create one on
   [test.pypi.org](https://test.pypi.org/account/register/) too.
2. **Register the trusted publisher.** The project does not exist yet, so add a *pending* publisher. On
   PyPI, go to *Your account → Publishing → Add a new pending publisher* and fill in:

   | Field | Value |
   |---|---|
   | PyPI Project Name | `oxilite` |
   | Owner | `Volland` |
   | Repository name | `oxilite` |
   | Workflow name | `python-wheels.yml` |
   | Environment name | `pypi` |

3. **Create the environment.** In the GitHub repository, go to *Settings → Environments → New
   environment* and create `pypi`. Under *Deployment branches and tags*, choose *Selected branches and
   tags* and allow the tag pattern `v*` and the branch `main`, so only release tags and manual runs on
   `main` can publish. As an extra safety catch, add yourself as a required reviewer: each publish then
   waits for your approval in the Actions tab.

   The same with the GitHub CLI:

   ```bash
   gh api -X PUT repos/Volland/oxilite/environments/pypi \
     --input - <<< '{"deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}'
   gh api -X POST repos/Volland/oxilite/environments/pypi/deployment-branch-policies -f name='v*' -f type=tag
   gh api -X POST repos/Volland/oxilite/environments/pypi/deployment-branch-policies -f name=main -f type=branch
   ```

After the first successful upload, the pending publisher becomes a normal publisher of the `oxilite`
project.

## 5. Release

### Automatically, from the release tag

A release is tagged in the usual way:
1. Bump the workspace version in the root `Cargo.toml`, along with the internal dependency versions and
   the npm packages.
2. Commit "Release X.Y.Z".
3. Create an annotated tag `vX.Y.Z` and push it.

The package version is `dynamic` in `pyproject.toml`: maturin reads it from the Cargo workspace, so there
is no Python version to bump.

Pushing the tag starts `python-wheels.yml`:
1. **`wheels`** builds six wheels:
   - `manylinux` x86_64 and aarch64
   - `musllinux` x86_64
   - macOS x86_64 and arm64
   - Windows x64

   It installs the Linux x86_64, macOS arm64 and Windows wheels and runs a smoke test with each.
2. **`sdist`** builds the source distribution.
3. **`publish`** downloads every artifact and uploads it to PyPI through trusted publishing. If you added
   a required reviewer, approve the deployment in the Actions tab.

Check the result at <https://pypi.org/project/oxilite/>, then run `pip install oxilite==X.Y.Z` in a clean
environment.

### When the tag run fails

A tag cannot be rebuilt with a fix, and moving a pushed tag is bad practice. If a wheel fails on the tag
(so nothing was published), fix it on `main`, then run the workflow by hand from `main` (*Actions → Python
wheels → Run workflow*, branch `main`) with **Publish** checked, or:

```bash
gh workflow run python-wheels.yml --ref main -f publish=true
```

The version still comes from the Cargo workspace, so this publishes the tagged version with the fix. Do it
only while `main` has not moved on to the next version's changes in the Python package or its crates.

### Rehearse on TestPyPI

Run the workflow by hand (*Actions → Python wheels → Run workflow*) with **Publish** unchecked. It builds
and uploads every wheel as a workflow artifact without publishing. To rehearse the upload itself:
1. Download the artifacts.
2. Upload them to TestPyPI with twine and a TestPyPI token:

   ```bash
   pip install twine
   twine upload --repository testpypi dist/*
   pip install --index-url https://test.pypi.org/simple/ oxilite
   ```

### By hand, without the workflow

This path is for emergencies, or for a platform the workflow does not build.
1. Create an API token under *Account settings → API tokens* on PyPI, scoped to the `oxilite` project.
2. Run:

   ```bash
   cd bindings/python
   export MATURIN_PYPI_TOKEN=pypi-…        # never commit it
   maturin publish                          # builds this platform's wheel and the sdist, then uploads both
   ```

   Or upload wheels you built elsewhere:

   ```bash
   twine upload dist/*
   ```

PyPI refuses a second upload of the same file name. A version cannot be replaced, only yanked, so a fix
means a new version.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `rustc 1.87.0 is not supported` | An older `rustc` is first on `PATH`. Use rustup's (`export PATH="$HOME/.cargo/bin:$PATH"`) |
| `maturin develop` says it needs a virtualenv | Activate the venv first, or use `maturin build` and `pip install` the wheel |
| `ImportError: … _native` after pulling | The native module is stale. Run `maturin develop` again |
| A pyoxigraph test fails in the port | Fix the behaviour, or, if the difference is intended, add a `py:` entry with a reason and a decision to `testsuite/allowlist.toml` |
| `readme path … does not exist` during `maturin sdist` | Every workspace crate needs the README its manifest names. The sdist includes the workspace |
| The `publish` job waits, then fails with `Branch "…" is not allowed to deploy to pypi` | The run is not on a `v*` tag or `main`. Run it from `main` |
| The `publish` job fails with `invalid-publisher` | The trusted publisher on PyPI does not match. Check the owner, repository, workflow file name and environment (`pypi`) |
| `File already exists` on upload | That version is already on PyPI. Bump the version |
| `ARM assembler must define __ARM_ARCH` (ring) in the aarch64 wheel | manylinux2014's aarch64 cross compiler is too old for ring. Build that wheel with `manylinux: "2_28"` (glibc 2.28 or later) |
