## Why

OWL 2 RL materialization and Datalog materialization both write `quads_inf`, and each run
replaced the whole table. oxilite studio materializes OWL 2 RL and several rule files side by
side, and explains inferences by producer, so one producer's run must not erase the others'.

## What Changes

- A side table `quads_inf_src(src, s, p, o, g)` attributes each inferred quad to its
  producer, with names in `inf_producers`; `quads_inf` and every reader are unchanged.
- Materializing resets only its own producer, then claims the quads nobody claims yet.
- `Options::producer` names a Datalog program's conclusions (default `datalog`); OWL 2 RL is
  `owl2rl` for both the SQL rules and `reasonable`.
- `Store::clear_inferences_of(producer)` and `Store::inference_producers(quad)`, and their
  async twins.

## Capabilities

### Modified Capabilities
- `reasoning`: inferences carry their producer.

## Impact

`oxilite-core` (schema, `reason`, `ops`), `oxilite-datalog` (materialization, options),
`oxilite-reason`, `oxilite` (store API). One extra row per inference; on D1 it is billed.
