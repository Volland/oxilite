## Context

The CLI already opens every kind of store through `db::Db` (bundled SQLite, a dlopen'ed
library, D1 behind the sidecar), and the studio server already has a lenient SPARQL scanner
and a role-aware completion engine (`studio::lang::complete`) fed by the store's vocabulary
(`studio::conn::Vocab`). The shell is a front end over both.

## Goals / Non-Goals

Goals: zero-argument start, persistence by naming a file, SPARQL queries and updates that span
lines, completion that knows the store, readable results, `sqlite3`-like dot-commands, script
mode for pipes.

Non-goals: Cypher statements in the shell (Datalog is reachable through `.datalog`), paging
results through an external pager, a server connection (`oxilite serve` covers that).

## Decisions

- **Top-level arguments, not a `shell` subcommand.** `Args` takes an optional subcommand, an
  optional positional `DATABASE` and the flattened store flags, with
  `args_conflicts_with_subcommands`. `oxilite` and `oxilite data.sqlite` open the shell;
  `oxilite query …` is unchanged. A file literally named like a subcommand is opened with `-l`.
- **Opening is creating.** `Db::open` already creates the file and the schema; the shell adds
  nothing. No argument means `:memory:`, and the banner says the store is transient and how to
  persist it (`.open FILE`, `.save FILE`), like `sqlite3`.
- **`rustyline`.** Synchronous like the rest of the CLI, with completion, highlighting, hints,
  history and a file-name completer. `reedline` would need `crossterm` event handling and gives
  little more for a line-oriented shell.
- **Line-by-line reading with a continuation prompt.** Each line is read with `oxilite> ` or
  `   ...> ` and appended to a pending buffer, as `sqlite3` does. The completer and highlighter
  see the pending buffer plus the current line, so prefixes declared on earlier lines resolve.
  The whole statement goes into history as one entry.
- **When a statement is complete.** SPARQL has no terminator, and `;` also separates
  predicate-object lists. After each line the buffer runs when brackets, braces and parentheses
  are balanced outside strings, IRIs and comments, and either it ends with `;` (removed before
  parsing), or the line just entered is empty (so a broken statement reports its error instead
  of waiting forever), or it is a single line that parses as a query or an update. A statement
  over several lines never runs just because it parses: `SELECT … { … }` on its own lines is
  complete, yet `ORDER BY` or `LIMIT` may follow. A dot-command is recognised only on the first line of a statement.
- **Session prefixes.** A table seeded with the studio's `WELL_KNOWN` prefixes and `oxl:`.
  Before running, the statement is scanned; for every prefixed name whose prefix the text does
  not declare but the session knows, a declaration is prepended on the first line (so error
  line numbers stay right). `PREFIX` declarations of a successful statement join the session,
  and `.load` of a text RDF file adds its `@prefix` declarations. Results compact IRIs with the
  same table.
- **Completion reuses the studio.** The completer scans the session prologue plus the buffer
  and calls `lang::complete` with the store's `Vocab`, then filters by the typed word (LSP
  clients filter, `rustyline` does not). `Vocab` is computed lazily on the first Tab after the
  store changed (start, update, load, `.open`), since it scans the store. `Vocab::compute` is
  split so any query function can feed it.
- **Output.** The default `table` mode draws a box table sized to the terminal: widest columns
  shrink first and cells end in `…`; IRIs print as prefixed names, plain strings bare,
  language strings with their tag, numbers and booleans by value, blank nodes as `_:x`;
  colours only on a terminal and never with `NO_COLOR`. At most `.maxrows` rows print (default
  200) with a count of the rest. Graph results print as Turtle with the session prefixes that
  are used. `json`, `xml`, `csv`, `tsv` use the SPARQL results serializers; RDF modes
  (`turtle`, `ntriples`, `nquads`, `trig`, `rdfxml`) apply to graph results, and solutions
  then print as a table.
- **Dump and save are SPARQL.** `.dump` selects default-graph and named-graph quads and writes
  N-Quads; `.save FILE` loads that dump into a new file store, refusing an existing file. Both
  work unchanged on every backend.
- **Script mode.** Not a terminal on standard input: the same accumulator, no banner, prompt,
  colours or history; errors go to standard error with the statement's first line number and
  the exit status is 1 if any statement failed. `.exit` stops at once, like `sqlite3`.
