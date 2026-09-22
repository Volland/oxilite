## 1. rudof integration

- [ ] 1.1 Pin rudof crates to versions on the Oxigraph 0.5 family; check wasm32 builds
- [ ] 1.2 `Rdf` / `NeighsRDF` implementation on `blocking::Store`
- [ ] 1.3 `QueryRDF` implementation routed through the oxilite compiler
- [ ] 1.4 `validate()` API for SHACL and ShEx returning rudof report types

## 2. D1 prefetch

- [ ] 2.1 Shape analysis: target classes/nodes, mentioned predicates, `sh:node` depth
- [ ] 2.2 Batched subgraph fetch into `SRDFGraph`, with a size limit
- [ ] 2.3 Async validate on D1

## 3. Verification

- [ ] 3.1 rudof SHACL/ShEx suites against an oxilite-backed store
- [ ] 3.2 Report equality against rudof's in-memory graph (differential)
- [ ] 3.3 Update `lat.md/` and run `lat check`
