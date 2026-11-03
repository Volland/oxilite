## 1. Crate `oxilite-elpi`

- [x] 1.1 Crate and workspace entry; `leanelpi` 0.4.1 optional behind `native`
- [x] 1.2 Prelude: term types, `ox.triple`, `ox.quad`, `ox.conclude`, `ox.violation`
- [x] 1.3 Term mapping with natives for integers and doubles
- [x] 1.4 Host: one pattern scan per call over `reasoning_input_job`, row budget
- [x] 1.5 Engine: program cache, declared host predicates, errors (program, run, fuel, budget, unsupported)
- [x] 1.6 `query`, `check`, `materialize` over any `SyncBackend`
- [x] 1.7 Annotate the code with `@tg: implements::` for each requirement of `elpi-programs`

## 2. Surfaces

- [x] 2.1 `oxilite` features `elpi` and `elpi-native`; `Store::elpi_query(_with)`, `elpi_check(_with)`, `elpi_materialize(_with)`
- [x] 2.2 `oxilite elpi` with `--goal`, `--check`, `--materialize`, `--producer`, `--graph`, `--inferred`, `--limit`, `--fuel`; CLI feature `elpi` with a run path to `LEANELPI_DIST`

## 3. Tests and docs

- [x] 3.1 Host unit test without the engine; store tests for every scenario with `elpi-native`
- [x] 3.2 lat.md: architecture section, decision D45, test specs
- [x] 3.3 Article on the site and README entry
- [x] 3.4 `tg check`, `tg ontology check`, `openspec validate --strict`
