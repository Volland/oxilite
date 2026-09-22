## Why

Knowledge-graph applications need ontology-aware answers (subclass, subproperty, inverse and transitive properties) without paying for materialized inferences on every write. That cost is especially high on D1, where every written index entry is billed.

## What Changes

- New crate `oxilite-reason`.
- A `tbox_closure(kind, sub, sup)` table: the transitive closure of subClassOf and subPropertyOf, plus inverseOf, SymmetricProperty, TransitiveProperty, domain and range facts. It is recomputed by `optimize()` and after schema-affecting updates.
- A per-query reasoning option `None | Rdfs | OwlQl`, defaulting to `None` (Oxigraph behaviour). The option rewrites triple patterns against `tbox_closure`, so queries remain single SQL statements.
- Explicit `materialize()` for OWL 2 RL:
  - writes into `quads_inf`, which has the same indexes as `quads`;
  - uses SQL fixpoint rules (`INSERT OR IGNORE … SELECT` looped until no changes) on every backend, and `reasonable` on native backends for speed;
  - rebuilds from scratch on every run (no incremental maintenance in this milestone).
- A query option to include materialized inferences.

## Capabilities

### New Capabilities
- `reasoning`: RDFS / OWL-QL entailment by query rewriting, and opt-in OWL 2 RL materialization.

### Modified Capabilities
<!-- none -->

## Impact

- New dependency on `reasonable` (native only, behind a feature).
- Schema additions (`tbox_closure`, `quads_inf`) created with `IF NOT EXISTS`.
