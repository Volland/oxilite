## 1. Core

- [x] 1.1 `quads_inf_src` and `inf_producers` tables
- [x] 1.2 `inference_reset`, `inference_attribute`, `materialize_reset_for`, producer ids
- [x] 1.3 OWL 2 RL rounds attribute; `reasonable` resets and attributes its producer
- [x] 1.4 Datalog `Options::producer`, JSON option, scoped reset and attribution
- [x] 1.5 `clear_inferences_of`, `inference_producers` (sync and async); `clear` and
      `clear_inferences` empty the side table

## 2. Tests and docs

- [x] 2.1 Producers keep their own conclusions (Datalog test suite)
- [x] 2.2 Full workspace suite passes
- [x] 2.3 D28 and the reasoning and materialization sections in `lat.md`
