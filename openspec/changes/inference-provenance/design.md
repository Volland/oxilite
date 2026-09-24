## Decisions

- **A side table, not a key column.** Adding `src` to `quads_inf`'s key would duplicate rows
  every reader must then de-duplicate. The side table keeps readers and the fixpoint loop as
  they are (decision D28).
- **Claim, don't track.** After writing, a producer claims every unclaimed inferred quad. A
  quad two producers can derive is claimed by the first; it survives resetting the other, and
  a full re-run in order (OWL 2 RL, rules, OWL 2 RL) restores every attribution.
- **Producer ids are hashes** of their names, computed without a lookup.
