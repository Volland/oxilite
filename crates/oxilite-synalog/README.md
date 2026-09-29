# oxilite-synalog

[Synalog](https://github.com/SynaLinks/synalog), the Datalog-family language for AI agents, over
[oxilite](https://github.com/Volland/oxilite): run unmodified Synalog programs on the triple
store, on bundled SQLite, Turso, a system SQLite library or Cloudflare D1.

Synalog (a Rust rewrite of Logica) gives an agent a semantic layer of named concepts and rules:
named arguments, expressions, head aggregation, functors, `@OrderBy`/`@Limit` for pageable
results, and a verifier whose errors are written for a model to read. This crate uses its parser,
verifier and compiler unchanged and makes the store look like tables.

```rust
use oxilite::store::Store;

let store = Store::new()?;
store.update(
    "PREFIX ex: <http://example.org/>
     INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy .
                   ex:ada ex:age 42 . ex:bob ex:age 17 }",
)?;

let r = store.synalog(r#"
  # @table parent <http://example.org/parent>
  # @table age <http://example.org/age>

  @Recursive(Ancestor, 10);
  Ancestor(x:, y:) distinct :- parent(subject: x, object: y);
  Ancestor(x:, y:) distinct :- Ancestor(x:, y: m), parent(subject: m, object: y);

  AdultAncestor(x:, y:) distinct :- Ancestor(x:, y:), age(subject: x, object: a), a >= 18;
"#, "AdultAncestor")?;
assert_eq!(r.columns, ["x", "y"]);
assert_eq!(r.rows.len(), 2);   // ada → bob, ada → cy
# Result::<_, Box<dyn std::error::Error>>::Ok(())
```

Enable it with `oxilite = { version = "0.9", features = ["synalog"] }`.

## The store as tables

| Table | Columns |
|---|---|
| `triples` | `subject`, `predicate`, `object`, `kind`, `datatype`, `lang`, `graph` |
| `# @table NAME <IRI>` | `subject`, `object`, `kind`, `datatype`, `lang`, `graph` — the triples of one predicate |
| `# @class NAME <IRI>` | `subject`, `graph` — the instances of one class |

Terms decode to native SQL values, so comparisons and arithmetic mean what they say: integers and
other numeric literals are numbers, booleans 1/0, blank nodes `_:label`, IRIs and other literals
their lexical form. `kind` is `iri`, `blank`, `literal` or `triple`; `graph` is NULL for the
default graph. Declared tables select by term id and are answered by an index; prefer them to
filtering `triples` by a predicate string. The pragmas are Synalog comments, so the program still
runs with Synalog's own CLI.

`Options` sets the same scope as SPARQL and Datalog — `union_default_graph`, `include_inferred`,
`as_of` — plus `limit`, `offset` and tables declared from code.

## Without a store

`check(program)` runs the verifier; `compile_for_engine(program, predicate, engine, limit, offset)`
returns Synalog's SQL for `duckdb`, `sqlite`, `psql`, `bigquery`, `trino`, `presto` or
`databricks`.

## What the store will not run

Synalog's SQLite output relies on Logica runtime functions, which D1 cannot register. Those with
a plain SQL form are rewritten; the rest are rejected before anything runs, naming the construct:
`ArgMax=`/`ArgMin=`, file, solver and model calls, record assembly, and anything that compiles to
several statements — `@Ground`, `@AttachDatabase`, and a `@Recursive` bound above 20 (which
Synalog evaluates iteratively through tables it creates). Results are SQL values, not RDF terms, so
they are queried, not materialized; use `oxilite-datalog` for rules whose conclusions should become
triples.

## Command line

```sh
oxilite synalog Ancestor -l data.sqlite -f rules.l          # run, print tab-separated rows
oxilite synalog Ancestor -l data.sqlite -f rules.l --sql    # the SQL it runs on the store
oxilite synalog Ancestor -f rules.l --engine duckdb         # Synalog's SQL for another engine
```

Licensed under MIT or Apache-2.0, like oxilite; Synalog itself is Apache-2.0.
