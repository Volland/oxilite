## 1. rudof integration

- [x] 1.1 Pin rudof crates to versions on the Oxigraph 0.5 family (`rudof_rdf`, `shacl`, `shex_validation` 0.3.21); check wasm32 builds (rudof compiles its validators out on wasm32, so D1 is validated natively through the prefetch)
- [x] 1.2 `Rdf` / `NeighsRDF` implementation on `blocking::Store`
- [x] 1.3 `QueryRDF` implementation routed through the oxilite compiler
- [x] 1.4 `validate()` API for SHACL and ShEx returning rudof report types

## 2. D1 prefetch

- [x] 2.1 Shape analysis: target classes/nodes, mentioned predicates, `sh:node` depth
- [x] 2.2 Batched subgraph fetch into `SRDFGraph`, with a size limit
- [x] 2.3 Async validate on D1

## 3. Verification

- [x] 3.1 rudof SHACL/ShEx suites against an oxilite-backed store
- [x] 3.2 Report equality against rudof's in-memory graph (differential)
- [x] 3.3 Update `lat.md/` and run `lat check`
