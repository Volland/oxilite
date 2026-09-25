## 1. SPARQL

- [x] 1.1 `GRAPH <oxilite:history>` compiled to commits (ticks as inline integers; time, author, message terms recorded by the tick) and changes (`oxl:added` / `oxl:removed` over `quad_log`)
- [x] 1.2 Tests: who removed a triple and when; every change with its author; refused without a log; on D1

## 2. Datalog

- [x] 2.1 `at "REF"` and `at ?c` on atoms; `?c` must be bound by a positive atom
- [x] 2.2 Built-ins `commit`, `added`, `removed`, `branch`; a program's own relation of that name wins
- [x] 2.3 Tests: comparing versions per atom; the value at every commit; who removed what; unsafe `at`

## 3. Cypher

- [x] 3.1 Version option (`asOf`); direct quad reads through the as-of source; resolved once; writes refused
- [x] 3.2 Tests on native, the D1 code path, `@oxilite/d1` and `@oxilite/node`

## 4. Docs

- [x] 4.1 `lat.md` Versioning section and test specs; `docs/versioning.md`; README; the articles
