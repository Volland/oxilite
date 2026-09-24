## 1. Crate and language front end

- [x] 1.1 `oxilite-datalog` crate: workspace member, `[workspace.dependencies]` entry, no new
      third-party dependencies beyond `oxilite-core`, `oxrdf`, `spargebra`, `thiserror`
- [x] 1.2 `lexer`: tokens for `:-`, `?-`, `,`, `.`, `not`, `@prefix`, variables, `_`, CURIEs,
      IRIs, RDF literals with datatype and language tags, numbers, operators
- [x] 1.3 `ast`: `Program`, `Rule`, `Head`, `Atom`, `Term`, `Constraint`, `Goal`, `Aggregate`
- [x] 1.4 `parser`: recursive descent over the token stream, with spans in errors
- [x] 1.5 `error`: parse, safety, stratification and compile errors, each naming the offending
      rule, variable or component

## 2. Program analysis

- [x] 2.1 Predicate dependency graph with positive, negative and aggregating edges
- [x] 2.2 Tarjan SCCs and the condensation's topological order
- [x] 2.3 Stratification: reject a negative or aggregating edge inside an SCC, naming it
- [x] 2.4 Safety: head, negated-atom and constraint variables must be bound positively
- [x] 2.5 Linearity classification per SCC: linear single-predicate, linear mutual, non-linear

## 3. Lowering to SPARQL algebra

- [x] 3.1 Atom to `TriplePattern`; `triple/3` and `triple/4`; CURIE and prefix resolution
- [x] 3.2 Body conjunction to `GraphPattern::Join`, negation to `Minus` / `Filter(Not(Exists))`
- [x] 3.3 Constraints to `spargebra::algebra::Expression`, including functions
- [x] 3.4 Head aggregation to `GraphPattern::Group`
- [x] 3.5 Rule union per predicate; projection to the head's argument order
- [x] 3.6 Stratum compilation through `core::query::compile_query`

## 4. Recursion

- [x] 4.1 `Capabilities::compound_recursive_cte`, conservative default, set per backend
- [x] 4.2 Linear single-predicate SCC to an inlined `(WITH RECURSIVE … SELECT …)` subquery,
      `UNION` in the recursive term, seeded from bound goal arguments where possible
- [x] 4.3 Mutual recursion as one tagged CTE with one recursive term per rule
- [x] 4.4 `datalog_work` table, scoped by run, created with the rest of the schema
- [x] 4.5 Iterate a non-linear component to a fixpoint: seed, step, count, bound, cleanup
- [x] 4.6 Strategy selection and the reason it was chosen, recorded for `explain()`

## 5. Execution and results

- [x] 5.1 `plan`: strata to `Job` steps
- [x] 5.2 `exec`: drive `Request`/`Response`, decode rows, resolve terms via `TermResolver`
- [x] 5.3 `explain`: strata, per-component strategy, SQL, join order, remote-round-trip warning

## 6. Materialization

- [x] 6.1 Triple-shaped head check, before any write
- [x] 6.2 Derive and insert into `quads_inf` in one atomic request, reusing `materialize_reset`
- [x] 6.3 Report what was replaced, since the inference store has one lifecycle

## 7. Integration

- [x] 7.1 `datalog` feature on `oxilite`; `datalog_store.rs` mirroring `cypher_store.rs`
- [x] 7.2 `Store` methods: `datalog`, `datalog_with`, `explain_datalog`, `datalog_materialize`,
      and the `AsyncStore` equivalents
- [x] 7.3 `oxilite-cli datalog` subcommand: run, `--explain`, `--materialize`, stdin or `--file`
- [x] 7.4 `oxilite-wasm` (off-by-default `datalog` feature, ~0.2 MB), `@oxilite/common` types,
      `@oxilite/node` and `@oxilite/d1` wrappers

## 8. Verification

- [x] 8.1 Lexer and parser unit tests, including error spans
- [x] 8.2 Stratification and safety rejection tests, each asserting the named cycle or variable
- [x] 8.3 Differential tests against SPARQL: every non-recursive program matches an equivalent
      SPARQL query's solutions, and linear recursion matches the equivalent property path
- [x] 8.4 One-statement assertions: non-recursive and recursive programs compile to a single statement
- [x] 8.5 Cyclic data terminates, for every recursion strategy
- [x] 8.6 Mutual recursion, with and without `compound_recursive_cte`
- [x] 8.7 Non-linear recursion agrees with the linear formulation; rounds reported; bound
      enforced; work rows cleaned up
- [x] 8.8 Aggregation against equivalent SPARQL `GROUP BY`
- [x] 8.9 Materialization: visible to SPARQL and Cypher with inferences on, invisible without,
      re-running replaces, asserted data untouched
- [x] 8.10 D1 parity: statement length, compound terms, no compound recursive term, no UDFs,
      materialization statement budget, no bound parameters
- [x] 8.11 `cargo clippy` clean; existing SPARQL, Cypher, reasoning and validation suites pass
- [x] 8.12 Update `lat.md/` (architecture, decisions D23–D27, milestones M8, tests) and run
      `lat check`
- [x] 8.13 Update the website and README for the new dialect
