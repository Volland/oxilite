## 1. Schema closure

- [x] 1.1 `tbox_closure` table and SQL computation (recursive CTEs for subClassOf/subPropertyOf)
- [x] 1.2 Recompute on `optimize()` and after updates that touch schema predicates

## 2. Query rewriting

- [x] 2.1 Per-query reasoning option (`None | Rdfs | OwlQl`)
- [x] 2.2 Rewrite `rdf:type` patterns through the subclass closure, and predicate patterns through the subproperty, inverse and symmetric closures
- [x] 2.3 Transitive properties through recursive CTEs; domain and range entailments

## 3. Materialization

- [x] 3.1 `quads_inf` table with the same indexes; query option to include it
- [x] 3.2 OWL 2 RL rules as SQL `INSERT OR IGNORE … SELECT`, looped to a fixpoint (D1-compatible)
- [x] 3.3 Native fast path through `reasonable`

## 4. Verification

- [x] 4.1 Hand-written entailment tests (RDFS, OWL-QL)
- [x] 4.2 Materialization agreement with `reasonable` on sample ontologies
- [x] 4.3 Update `lat.md/` and run `lat check`
