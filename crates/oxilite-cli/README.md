<p align="center">
  <a href="https://oxilitedb.com"><img src="https://raw.githubusercontent.com/Volland/oxilite/main/site/assets/logo.png" alt="oxilite" width="120"></a>
</p>

# oxilite-cli

[![crates.io](https://img.shields.io/crates/v/oxilite-cli.svg)](https://crates.io/crates/oxilite-cli) [![docs.rs](https://img.shields.io/docsrs/oxilite-cli)](https://docs.rs/oxilite-cli) [![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Volland/oxilite#license)

**The `oxilite` command line:** an interactive SPARQL shell, load, query and explain a SQLite-backed RDF store, or serve it over the SPARQL 1.1 protocol with the same routes as `oxigraph serve`.

**[Website](https://oxilitedb.com)** · [API docs](https://docs.rs/oxilite-cli) · **[Guide and architecture](https://github.com/Volland/oxilite#readme)** · [Changelog and issues](https://github.com/Volland/oxilite/issues)

## Install

```bash
cargo install oxilite-cli
```

## Shell

```bash
oxilite                 # a transient in-memory store with the full schema
oxilite data.sqlite     # that SQLite file, created with the schema if missing
oxilite data.sqlite < script.rq   # run a script; exit status 1 if a statement failed
```

```text
oxilite> PREFIX ex: <http://example.com/>
oxilite> INSERT DATA { ex:alice a foaf:Person ; foaf:name "Alice" ; ex:knows ex:bob }
OK · 412 µs
oxilite> SELECT ?who ?name WHERE {
   ...>   ?who a foaf:Person ; foaf:name ?name
   ...> };
┌──────────┬───────┐
│ who      │ name  │
├──────────┼───────┤
│ ex:alice │ Alice │
└──────────┴───────┘
1 row · 380 µs
oxilite> .exit
```

A statement runs as soon as it is complete on one line; a statement over several lines runs when it ends with `;` or an empty line. Tab completes dot-commands, SPARQL keywords, prefixes, variables and the store's own predicates and classes in the position they fit. Well-known prefixes (`rdf`, `rdfs`, `owl`, `xsd`, `foaf`, `schema`, …) are predeclared, and `PREFIX` declarations and loaded Turtle files add to them. Results are tables fitted to the terminal; `.mode` switches to JSON, XML, CSV, TSV or an RDF format. `.help` lists the commands: `.open`, `.save`, `.load`, `.read`, `.dump`, `.explain`, `.datalog`, `.graphs`, `.stats`, `.prefix`, `.timer`, `.maxrows`, `.exit` and more. History is kept in `~/.oxilite_history`.

## Commands

```bash
oxilite load     -l data.sqlite -f dump.nt data.ttl              # bulk load, then refresh statistics
oxilite query    -l data.sqlite -q 'SELECT * WHERE { ?s ?p ?o } LIMIT 5'
oxilite update   -l data.sqlite -u 'INSERT DATA { <http://ex/a> <http://ex/p> 1 }'
oxilite explain  -l data.sqlite -q 'SELECT …'                    # the SQL and the join order
oxilite optimize -l data.sqlite                                  # planner statistics and reasoning closure
oxilite serve    -l data.sqlite -b 127.0.0.1:7879                # /query, /update, /store
oxilite serve    -l data.sqlite --library /usr/lib/libsqlite3.so # the same file on the system SQLite
```

### Versioning

```bash
oxilite update -l kb.sqlite --versioning log -m "seed" --author ada -u 'INSERT DATA { … }'  # a new store at level log
oxilite query  -l kb.sqlite --as-of HEAD~1 -q 'SELECT …'     # the store one commit ago (#42, @2026-09-01T12:00:00Z)
oxilite versioning status -l kb.sqlite                       # level, head tick, genesis, commits
oxilite versioning log    -l kb.sqlite -n 20                 # commits: tick, time, +added -removed, message, author
oxilite versioning diff   -l kb.sqlite HEAD~3 HEAD           # net changes, as RDF Patch lines (A / D)
oxilite versioning changes -l kb.sqlite --since 40           # every change after tick 40
oxilite versioning set    -l kb.sqlite log                   # raise an existing store's level (off, stamped, log)
oxilite versioning purge  -l kb.sqlite --subject http://ex/alice --reason "erasure request" --yes
oxilite versioning migration --from off --to log             # the same change as SQL, for wrangler d1 migrations
oxilite schema --versioning log                              # the whole schema of a new versioned store
```

A store keeps its level: opening never changes it. Lowering a level freezes the history (queryable up to the freeze) and deletes it only with `--allow-loss`. `serve` answers `/query?version=HEAD~1`.

`serve` speaks the SPARQL 1.1 Protocol and Graph Store Protocol at `/query`, `/update` and `/store`, so clients and tools written for `oxigraph serve` work unchanged. Query results come as JSON, XML, CSV or TSV, graph results in any RDF format. Run `oxilite help <command>` for every option.

The database is an ordinary SQLite file: open it from Rust with [`oxilite`](https://crates.io/crates/oxilite), from Node.js with [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node), or with any SQLite tool.

## The oxilite family

oxilite is an Oxigraph-compatible RDF database and SPARQL 1.1 engine that stores its data in SQLite, so it runs anywhere SQLite runs: in-process, on a system or vendor `libsqlite3`, on Cloudflare D1 and in Durable Objects. The same data can be queried with SPARQL and openCypher, reasoned over with RDFS / OWL, and validated with SHACL and ShEx. Read the overview on **[oxilitedb.com](https://oxilitedb.com)** and the full guide in the [main README](https://github.com/Volland/oxilite#readme).

| Package | What it is for |
|---|---|
| [`oxilite`](https://crates.io/crates/oxilite) | The store: a drop-in for `oxigraph::store::Store`, plus `AsyncStore` for D1 |
| [`oxilite-core`](https://crates.io/crates/oxilite-core) | The sans-IO core: term encoding, schema, SPARQL → SQL compiler and planner |
| [`oxilite-rusqlite`](https://crates.io/crates/oxilite-rusqlite) | In-process backend with a bundled SQLite (the default) |
| [`oxilite-dylib`](https://crates.io/crates/oxilite-dylib) | Backend that loads your own `libsqlite3` at runtime |
| [`oxilite-d1`](https://crates.io/crates/oxilite-d1) | Cloudflare D1 backend for Rust Workers |
| [`oxilite-cypher`](https://crates.io/crates/oxilite-cypher) | openCypher over the same data, OWL- and SHACL-aware |
| [`oxilite-jsonld`](https://crates.io/crates/oxilite-jsonld) | JSON-LD documents stored verbatim, one named graph each |
| [`oxilite-vc`](https://crates.io/crates/oxilite-vc) | Verifiable Credentials: stored under their id, indexed, queryable |
| [`oxilite-reason`](https://crates.io/crates/oxilite-reason) | OWL 2 RL materialization with `reasonable` |
| [`oxilite-validate`](https://crates.io/crates/oxilite-validate) | SHACL and ShEx validation with rudof |
| [`oxilite-cli`](https://crates.io/crates/oxilite-cli) | The `oxilite` command and a SPARQL endpoint like `oxigraph serve` |
| [`@oxilite/node`](https://www.npmjs.com/package/@oxilite/node) | Node.js bindings, API of Oxigraph's JS package |
| [`@oxilite/d1`](https://www.npmjs.com/package/@oxilite/d1) | Cloudflare D1 and Durable Objects from TypeScript (WebAssembly core) |
| [`@oxilite/common`](https://www.npmjs.com/package/@oxilite/common) | RDF/JS terms and shared TypeScript types |

## License

Dual-licensed under [MIT](https://github.com/Volland/oxilite/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/Volland/oxilite/blob/main/LICENSE-APACHE), at your option, like Oxigraph.
