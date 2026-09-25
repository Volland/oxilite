## Context

Phase 1 (archived as `2026-09-25-versioned-store`) built the change log, ticks and as-of reads. This phase
adds branches and merge on top. The sketch below comes from the original design. The measurements that
decide how to build it are in the archived change's `assessment.md` § Query latency.

## Prerequisites from the measurements

- **Past-version reads cost:**
  - Selective queries at a past version cost 1–3.6× the present with the as-of index. Without the index,
    anything not bound by subject costs 10–100×.
  - A branch that is not checked out is read entirely through the log. Branches therefore require the as-of
    index; the level that adds branches creates it.
  - Aggregating a whole predicate's history costs about 17× even with the index. Design **checkpoints**
    (materialized snapshots of a version every N commits, `quad_snap(tick, s, p, o, g)`, read as base plus
    the log after it) before merge, or accept the cost with the numbers published.
- **Gate:** a BEAR-B run (the versioned-RDF benchmark) confirms or replaces the synthetic `as-of-latency`
  figures before merge is built.

## Design sketch (from the original design)

### Branches and merge

- **`branch(name, from)`:** inserts one `refs` row.
- **`checkout(name)`:** rewrites `quads` using the diff between the current head and the target. The diff is two set differences over the as-of views, and the rewrite runs in one batch (chunked on D1).
- **`commit` on a non-HEAD branch:** writes the log only; `quads` is untouched.
- **`merge(theirs)`:**
  - Find the lowest common ancestor with a recursive CTE.
  - Compute Δours and Δtheirs relative to that base.
  - Result = base + adds − removes, using set semantics.
  - A conflict exists only when one side adds a quad the other removes. It is reported and resolved by an explicit policy (`ours`, `theirs` or fail); the default is fail.
  - Write a commit with `parent2`.
  - Optional gate: run SHACL (`oxilite-validate`) on the merged view and abort on violation.
- **`log(branch)` and `diff(a, b)`:** return commits and deltas as RDF using PROV-O, so history is itself queryable.
