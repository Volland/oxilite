## 1. Entry point

- [x] 1.1 Optional subcommand; top-level `DATABASE` argument and store flags open the shell
- [x] 1.2 Banner naming the store (transient in-memory or file) and pointing to `.help`

## 2. Input

- [x] 2.1 Statement accumulator: balance, trailing `;`, parse check, empty line
- [x] 2.2 `rustyline` editor with continuation prompt, history file and history hints
- [x] 2.3 Syntax highlighting from the studio scanner
- [x] 2.4 Completion: dot-commands, arguments, file names, `lang::complete` with lazy `Vocab`
- [x] 2.5 Script mode for non-terminal input

## 3. Execution and output

- [x] 3.1 Session prefixes: seed, inject, remember, harvest from loaded files
- [x] 3.2 Table renderer fitted to the width, term display, colours, `.maxrows`
- [x] 3.3 Other modes: SPARQL results formats and RDF formats; Turtle for graphs in table mode
- [x] 3.4 Dot-commands: exit, quit, help, open, save, load, read, dump, mode, prefix, explain,
      datalog, graphs, stats, optimize, timer, maxrows

## 4. Tests and docs

- [x] 4.1 Unit tests: completeness, prefix injection, table fitting, completion
- [x] 4.2 Binary tests: file created with schema, in-memory default, `.exit`, script errors, `.save`
- [x] 4.3 `lat.md` architecture and test sections, CLI README; `lat check`
