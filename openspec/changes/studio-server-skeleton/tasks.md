## 1. Server

- [x] 1.1 `studio-server` subcommand with `--store`, LSP over stdio with `lsp-server`
- [x] 1.2 Project store: discover RDF files, graph per file, rebuild on start
- [x] 1.3 Load errors as diagnostics at the syntax error position
- [x] 1.4 `oxilite/query`, `oxilite/status`, `oxilite/reload`, `oxilite/storeChanged`
- [x] 1.5 Reload on `workspace/didChangeWatchedFiles`

## 2. Tests and docs

- [x] 2.1 In-memory LSP tests: initialize loads files, query returns rows, syntax error gives a
      diagnostic, reload picks up a new file
- [x] 2.2 `lat.md` architecture section and test specs; `lat check`
