## As built

- **Commits are ticks.** A commit is its tick as an inline `xsd:integer` (id = tick + the integer offset), which
  SQL can compute. Hash ids cannot be computed in SQL.
- **The tick records its terms.** The write tick's time (from the host clock), author and message become terms
  in the same batch (`version::write_tick_statements`), and `ticks` records their ids. That is two statements
  of every versioned batch instead of one. Measured on D1: `stamped` writes 3 rows per batch, and 4.82 rows
  per triple against 4.81.
- **Vocabulary.** The vocabulary is `oxl:` (`https://oxilite.dev/ns#added` / `#removed`), the namespace
  `oxl:textMatch` already uses.
- **Datalog `at ?c`.** It accepts a commit bound by any positive atom, and is placed by a placeholder replaced
  once the rule's bindings are known. Datalog results read the default graph's id 0 as unbound.
- **Cypher.** The version is resolved once per statement (in `@oxilite/d1`, by `resolveVersion` before
  `cypher`), and its four direct read queries use the as-of source.

## Context

Phase 1's as-of source (`version::as_of_sql`) and version resolution are reused. The sketch below comes from
the original versioning design (archived with `2026-09-25-versioned-store`, § Query surface).

## Decisions

- **`<oxilite:history>`** is recognized by the compiler like `SERVICE <oxilite:version/…>`. A `GRAPH
  <oxilite:history>` group compiles to a derived table:
  - each commit is `prov:Activity`, with `prov:startedAtTime` (from `ticks.time`), `prov:wasAssociatedWith`
    (author), `rdfs:comment` (message) and `prov:wasInformedBy` (previous commit);
  - each change is `?c ov:added <<( s p o )>>` or `?c ov:removed <<( s p o )>>`, read through `quad_log_tx`.

  The terms are hashed in SQL where possible. Otherwise they come from constants in the query.
- **Datalog `at`:** an atom's quad source is chosen per atom (`as_of_sql` of its tick). `at ?c` joins the tick
  column, and is safe only when a positive `commit(?c, …)` atom binds `?c`.
- **Datalog built-ins:** `commit(?c, ?parent, ?time, ?author)` reads `ticks` and `commits`; `added` and
  `removed` read `quad_log`; `branch(?name, ?c)` reads `refs` once branches exist, and `main` until then.
- **Cypher:** `exec.rs` builds about seven `FROM quads` reads. They take the source from the options (the
  as-of table when a version is set), and versions are resolved before execution, as for SPARQL.

## Risks

Triple terms in the history graph need term rows for quads whose terms may only exist in the log. Terms are
never garbage-collected, so they exist.
