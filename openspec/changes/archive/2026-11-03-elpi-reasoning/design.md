## Context

`leanelpi::Program::declare(name, signature)` makes a predicate a host predicate, and
`Program::solve_host(goal, limit, fuel, host)` answers each call of one with the closure
`host(name, args) -> rows`, synchronously, all rows of a call at once. Arguments arrive as terms,
unbound ones as variables; each row holds a term for every output argument, and ELPI unifies the
rows with the call.

## Decisions

### Mirror tiramemsu's engine shape, not its persistence

The prelude declares the term types and the host predicates, the program follows it, and the
predicates the program defines with reserved names carry its outputs: `ox.violation` for checks,
`ox.conclude` for materialization (tiramemsu's `tm.violation` and `tm.conclude`). oxilite has no
eids, valid time or support cascade, so a violation names focus terms, a conclusion is just a
triple, and materialization replaces the producer's inferences as a whole.

- motivated_by: [[decisions#D28 Inferences are attributed to producers]]

### Each host call is one pattern scan

`ox.triple S P O` turns its ground arguments into a pattern (`s = id`, `p = id`, `o = id`) over the
same input relation N3 uses: asserted quads of the scope, optionally other producers' inferences.
Loading the whole scope as facts was rejected: a goal about one subject would read the store.
`ox.triple` merges graphs (a triple in two graphs answers once); `ox.quad` keeps them.

### Typed terms, natives for arithmetic

ELPI is typed, so RDF terms are values of a type `term`. `xsd:integer` and `xsd:double` literals
become ELPI `int` and `float` under `num` and `dbl`, so `A >= 18` works on stored ages; other
literals keep lexical form and datatype. Bound arguments match by term id, so `(num 5)` matches the
stored integer 5.

### Native library behind a feature

`libleanelpi` carries the Lean runtime and is built for darwin and linux. The crate's `native`
feature links it; the default build keeps the API and fails runs with `Unsupported`, so CI and
`cargo test --workspace` need no library. The CLI's build script adds the library's directory as a
run path, so `oxilite elpi` finds it without `DYLD_LIBRARY_PATH`.

### Blocking stores only

Host calls happen inside `solve_host`, on the calling thread; an async backend cannot answer them.
A D1 store fails with `Unsupported`. Prefetching a scope for D1 is a possible follow-up.

## Risks / Trade-offs

- lean-elpi is about 2.4× slower than OCaml ELPI 1.18.
- A program that calls `ox.triple` with nothing bound reads the scope; the row budget bounds it.
