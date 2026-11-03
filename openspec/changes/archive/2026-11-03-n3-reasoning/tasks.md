## 1. Core

- [x] 1.1 `ops::reasoning_input_job`: asserted quads, optionally other producers' inferences, by subject, object, predicates and graphs, system graphs excluded
- [x] 1.2 `reason::inference_replace`: one atomic request replacing a producer's inferences

## 2. Crate `oxilite-n3`

- [x] 2.1 Crate, workspace entry, `eyeron` 0.8.25 with default features off
- [x] 2.2 Term mapping between `oxrdf` and eyeron, storable triples only
- [x] 2.3 `RuleSet::parse` with line numbers; analysis: body predicates, variable predicate, monotonicity
- [x] 2.4 `reason_over`: limits, pinned clock, proofs, fuses, unstorable conclusions
- [x] 2.5 `ReasonJob` and `MaterializeJob`, completeness required for materialization
- [x] 2.6 Annotate the code with `@tg: implements::` for each requirement of `n3-rules`

## 3. Surfaces

- [x] 3.1 `oxilite` feature `n3`; `Store::n3_reason(_with)`, `n3_materialize(_with)`, async equivalents
- [x] 3.2 `oxilite n3` with `--materialize`, `--producer`, `--graph`, `--inferred`, `--no-prune`, `--proof`, `--now`, `--max-iterations`

## 4. Tests and docs

- [x] 4.1 Store tests for every scenario, each citing its `tests#N3 reasoning` section
- [x] 4.2 lat.md: architecture section, decision D44, test specs
- [x] 4.3 Article on the site and README entry
- [x] 4.4 `tg check`, `tg ontology check`, `openspec validate --strict`
