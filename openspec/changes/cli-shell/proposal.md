## Why

`oxilite` is only usable one command at a time: every `query` or `update` opens the store,
runs one statement and exits. Without `--location` the store is `:memory:`, so an update is
lost before the next command can see it, and there is no way to explore a store the way
`sqlite3` lets you explore a database. Trying oxilite should take one word: `oxilite`.

## What Changes

- Running `oxilite` with no subcommand opens an **interactive SPARQL shell** on a transient
  in-memory store with the full schema. `oxilite FILE` (or `oxilite -l FILE`) opens the SQLite
  file, creating it with the schema when missing. `--library`, `--no-graph-index`,
  `--text-index` and `--d1-sidecar` apply as for the other commands.
- Statements (queries and updates) span lines and run when complete; results print as a
  coloured, width-aware table, or as JSON, XML, CSV, TSV or an RDF format (`.mode`).
- Context-aware **Tab completion**: dot-commands and their arguments, file names, SPARQL
  keywords, prefixes, and the store's own predicates and classes in the right triple position
  (the studio's completion engine), plus live syntax highlighting and history hints.
- **Session prefixes**: well-known prefixes are predeclared, `PREFIX` declarations and loaded
  Turtle files add to them, and every statement gets the declarations it uses but lacks.
- **Dot-commands** in the style of `sqlite3`: `.exit`, `.quit`, `.help`, `.open`, `.save`,
  `.load`, `.read`, `.dump`, `.mode`, `.prefix`, `.explain`, `.datalog`, `.graphs`, `.stats`,
  `.optimize`, `.timer`, `.maxrows`.
- When standard input is not a terminal, the shell runs it as a script (no banner, no prompt,
  exit status 1 if any statement failed), like `sqlite3 db < script.sql`.
- Persistent history in `~/.oxilite_history`.

## Capabilities

### New Capabilities
- `cli-shell`: the interactive shell of the `oxilite` binary.

## Impact

- `crates/oxilite-cli`: new `shell` module, `rustyline` dependency; `Command` becomes optional
  and the top-level command takes a database argument and the store flags.
- `studio::conn::Vocab` gains a constructor over any query function so the shell shares it.
- No change to the core, the store or any binding. Existing subcommands behave as before.
