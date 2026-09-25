## Context

The studio's design (in the `oxilite-studio` repository, `lat.md/decisions.md`) fixes a
separate Rust process speaking LSP, a Project store derived from workspace files, and
serializable result payloads. This skeleton implements only the part every later milestone
builds on.

## Goals / Non-Goals

Goals: a server the extension can spawn, a derived store, one query round trip, load
diagnostics, and a test harness that drives the server in memory.

Non-goals (later milestones): language features (completion, hover), the manifest, per-file
incremental reload, reasoning levels, SHACL, cancellation, attached stores, paging.

## Decisions

- **`lsp-server` over `tower-lsp`.** The CLI is synchronous and the store is a blocking
  `Store`; `lsp-server` (rust-analyzer's transport) needs no async runtime. Cancellation comes
  later by moving work to a thread.
- **A graph per file, queried as a union.** Each file loads into the graph named by its `file:`
  IRI, so a later per-file reload is `clear_graph` plus load. Queries set
  `union_default_graph`, so plain `SELECT` patterns see every file.
- **Parse before loading.** A file is parsed to quads first; a syntax error skips the file and
  becomes a diagnostic with the parser's position, instead of a half-loaded file.
- **Payload.** `oxilite/query` returns `oxilite_core::json::output_to_json` (RDF/JS terms) plus
  `elapsedMs` and `truncated`, the shape `@oxilite/common` already defines, so the webview can
  share its term handling.
- **Scratch store location.** `<root>/.oxilite/studio.sqlite`, deleted and recreated on start and
  on reload; `--store` overrides it and `:memory:` is accepted (tests).
