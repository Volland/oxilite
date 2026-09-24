# oxilite-datalog

Datalog over [oxilite](https://github.com/Volland/oxilite): recursive rules with stratified
negation, constraints and aggregation, compiled to SQL and run wherever SQLite runs — including
Cloudflare D1.

SPARQL's only recursion is a property path: one predicate, a fixed regular expression, no way to
join another relation or filter part-way through. This crate removes that ceiling, over the same
quads, the same term encoding and the same backends.

```rust
use oxilite::store::Store;

let store = Store::new()?;
store.update(
    "PREFIX ex: <http://example.org/>
     INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
)?;

let r = store.datalog(r#"
  @prefix ex: <http://example.org/> .

  ancestor(?x, ?y) :- ex:parent(?x, ?y).
  ancestor(?x, ?z) :- ex:parent(?x, ?y), ancestor(?y, ?z).

  ?- ancestor(ex:ada, ?who).
"#)?;
assert_eq!(r.rows.len(), 2);   // bob and cy
# Result::<_, Box<dyn std::error::Error>>::Ok(())
```

## The language

RDF-native, spelled like Turtle and SPARQL.

| Form | Means |
|---|---|
| `ex:parent(?x, ?y)` | the triple pattern `?x ex:parent ?y` |
| `ex:Person(?x)` | `?x rdf:type ex:Person` |
| `triple(?s, ?p, ?o)` | any triple in the default graph, predicate variable included |
| `triple(?s, ?p, ?o, ?g)` | any quad |
| `p(?x, ?y)` | a relation defined by rules |
| `not p(?x, ?y)` | stratified negation |
| `?a >= 18`, `?a * 2 < 100`, `STRSTARTS(?n, "A")` | constraints, in SPARQL's expression language |
| `n(?x, COUNT(?y)) :- …` | aggregation, grouped by the head's other arguments |
| `?- p(?x, ?y).` | the goal: what the program returns |

Variables are `?x`, `_` is anonymous, prefixes come from `@prefix`, and literals carry `^^`
datatypes and `@` language tags. `%` and `#` start a comment.

## How it compiles

Every relation — stored or derived — is a set of rows of tagged 64-bit term ids, so a derived
predicate composes with a triple pattern with no conversion. A non-recursive predicate becomes a
plain common table expression, a linear recursive component becomes one `WITH RECURSIVE` member,
and a mutually recursive component becomes one member carrying a discriminant column. The whole
program is **one statement**, so it is one round trip.

SQLite's recursive-CTE restrictions turn out to be Datalog's classical safety conditions — one
self-reference per recursive term means rules must be linear, no self-reference under
`NOT EXISTS` means negation must be stratified — so the compiler reports them as rule diagnostics
before any SQL exists: an unstratified program names the cycle, and an unsafe rule names the
unbound variable.

A non-linear rule, such as `path(?x,?z) :- path(?x,?y), path(?y,?z).`, has no single-statement
form. It is iterated to a fixpoint in the `datalog_work` table instead — one request per round,
driven by the same step machine as everything else, so it runs on D1 too. The result reports the
rounds each component took, `Options::max_iterations` bounds them, and the rows are scoped to the
evaluation and deleted when it ends. Because the rounds write, a program with a non-linear
component needs a writable store.

## Storing what rules derive

`datalog_materialize()` writes a program's conclusions into `quads_inf`, the table OWL 2 RL
materialization already uses, in one atomic request. SPARQL and Cypher then see those facts under
`include_inferred`, so user rules extend the reasoner rather than running beside it. The two share
one inference set: running either replaces it.

## Limits

- Aggregates must produce a value the encoding can inline, so `COUNT`, `COUNT DISTINCT`, `SUM`,
  `MIN`, `MAX` and `SAMPLE` are available and `AVG` and `GROUP_CONCAT` are refused.
- A component whose rules are non-linear costs one request per round and needs write access; the
  relation may be at most six columns wide.
- Existential rules and unstratified negation are out of scope.

See [`lat.md/architecture.md`](../../lat.md/architecture.md) (`Datalog frontend`) for the design
and [`openspec/changes/datalog-dialect/`](../../openspec/changes/datalog-dialect/) for the
requirements.

## Beyond Rust

`oxilite datalog -l db.sqlite -f rules.dl` runs a program from the command line, with `--explain`
and `--materialize`, or reads it from standard input. `@oxilite/node` and `@oxilite/d1` expose
`datalog()`, `datalogMaterialize()` and `explainDatalog()`, returning RDF/JS terms; in the
WebAssembly core the dialect is an off-by-default feature costing about 0.2 MB.

## License

MIT or Apache-2.0, at your option.
