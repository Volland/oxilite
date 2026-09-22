## Why

Validating data against SHACL and ShEx shapes is a core need for knowledge graphs. rudof already provides complete Rust engines for both, built on the same Oxigraph crate family oxilite uses, so we can reuse them unchanged instead of writing a validator.

## What Changes

- New crate `oxilite-validate`, behind a `validation` feature because rudof's dependency tree is large.
- Implementations of rudof's `srdf` traits (`Rdf`, `NeighsRDF`, `QueryRDF`) on the native blocking store. rudof's SPARQL-based paths then run through the oxilite compiler.
- A prefetch adapter for D1 (async, non-blocking):
  - loads the relevant subgraph into rudof's in-memory `SRDFGraph` with batched SQL: target nodes, predicates mentioned in shapes, and the `sh:node` / `sh:property` closure to a configured depth;
  - runs rudof on that graph;
  - fails loudly above a configurable triple limit.
- A `validate(shapes, format) -> ValidationReport` API returning rudof's report types.

## Capabilities

### New Capabilities
- `shape-validation`: SHACL and ShEx validation of oxilite data with rudof.

### Modified Capabilities
<!-- none -->

## Impact

- New dependencies: rudof `srdf`, `shacl_validation`, `shex_validation`, pinned to versions that use the same Oxigraph 0.5 crates.
- wasm32 compatibility of rudof must be verified early. If it fails, D1 validation is offered through a native CLI instead.
