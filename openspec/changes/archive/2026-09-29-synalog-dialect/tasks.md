## 1. Crate

- [x] 1.1 `oxilite-synalog` crate: workspace member and `[workspace.dependencies]` entry; depends
      on `synalog` 1.2 with default features off, `oxilite-core`, `oxrdf`, `thiserror`
- [x] 1.2 `error`: parse, verification (every message), compile, unsupported and store errors
- [x] 1.3 `check` and `compile_for_engine`: the dialect without a store, panics turned into errors

## 2. The store as tables

- [x] 2.1 Term decoding in SQL: native value, `kind`, `datatype`, `lang`, graph
- [x] 2.2 `triples` over the quad source chosen by the options (inferred, `as_of`, graph scope)
- [x] 2.3 `# @table` / `# @class` pragmas and `Options::tables`, selecting by term id
- [x] 2.4 Inject only the referenced tables as `NOT MATERIALIZED` CTEs in front of the statement

## 3. Portable SQL

- [x] 3.1 String- and parenthesis-aware scanner for function calls
- [x] 3.2 Rewrite `MagicalEntangle`, `IN_LIST`, `JOIN_STRINGS`, `DistinctListAgg`, `SortList`
- [x] 3.3 Reject runtime-only calls and multi-statement output, naming the construct
- [x] 3.4 Check `max_sql_len` and `max_compound_select` before sending

## 4. Execution and surfaces

- [x] 4.1 `SynalogJob` (one read request) and `SynalogResult` (columns, SQL-value rows)
- [x] 4.2 `Store::synalog`, `synalog_with`, `synalog_sql`; `AsyncStore::synalog`, `synalog_with`;
      version resolution shared with Datalog
- [x] 4.3 `oxilite` feature `synalog`; `oxilite-cli` subcommand `synalog` with `--sql`, `--engine`,
      `--limit`, `--offset`, `--union-graph`, `--inferred`, `--as-of`

## 5. Tests and docs

- [x] 5.1 Store tests for every scenario of the `synalog-query` spec
- [x] 5.2 Unit tests for the scanner and the rewrites
- [x] 5.3 `lat.md`: architecture section, decision, test specs, milestone; crate README
- [x] 5.4 `cargo clippy`, `cargo test -p oxilite-synalog`, `lat check`, `openspec validate`

## 6. Bindings

- [x] 6.1 `json` feature of `oxilite-synalog`: options (scope, pagination, `asOf`, tables) and results
- [x] 6.2 `oxilite-wasm` feature `synalog` (`synalog`, `synalog_sql`, `asOf` resolved first); D1 driver
      `synalog()` / `synalogSql()` with a clear error on a core without it
- [x] 6.3 `@oxilite/common` types; `@oxilite/node` `synalog()` / `synalogSql()`
- [x] 6.4 Python `Store.synalog()` / `synalog_sql()`, `SynalogResult`, error mapping
- [x] 6.5 Tests: Node, Python, D1 (Miniflare, on a `synalog` core); docs and the website article
