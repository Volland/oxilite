## Why

oxilite studio, a VS Code extension, needs the engine in a separate process: a panic or a long
materialization must not take down the editor, and SHACL (`oxilite-validate`) has no JavaScript
binding. The studio decided on a Rust language server living next to the core, so provenance and
justification work later lands in the same pull requests as the features that use it.

This change is the studio's walking skeleton (milestone S0): the thinnest server that proves the
path from the editor to the store and back.

## What Changes

- An `oxilite studio-server` subcommand on `oxilite-cli`: a Language Server Protocol server over
  standard input and output, built on `lsp-server` and `lsp-types`.
- A **Project store**: on `initialize`, every RDF file under the workspace root is loaded into a
  scratch store at `<root>/.oxilite/studio.sqlite`, each file into its own named graph (the
  file's `file:` IRI). The store is rebuilt from scratch on start and on reload.
- Load errors become `textDocument/publishDiagnostics` at the syntax error's position.
- Custom requests `oxilite/query` (SPARQL over the union of all file graphs, returning the
  `output_to_json` payload the JavaScript packages already use, capped by a row limit),
  `oxilite/status` and `oxilite/reload`, plus an `oxilite/storeChanged` notification.
- File changes reported through `workspace/didChangeWatchedFiles` trigger a full reload.

## Capabilities

### New Capabilities
- `studio-server`: the language server behind oxilite studio.

## Impact

- `crates/oxilite-cli`: new `studio` module, two new dependencies (`lsp-server`, `lsp-types`).
- No change to the core, the store or any binding.
