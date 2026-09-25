## Context

Planned last, when a user needs to synchronize stores (a laptop and D1, several devices). It depends on
`version-branches` for merging diverged histories.

## Design sketch (from the original design)

### Push and pull

- **Exchange unit:** commits missing on the other side, plus their `quad_log` rows and the `terms` and `triple_terms` rows they reference.
- **Format:** N-Quads delta patches (RDF Patch style `A`/`D` lines), so the format is readable and survives across backends. Hash IDs make re-encoding deterministic.
- **Commands:** `oxilite push/pull <remote>` in `oxilite-cli`. Remotes are file paths or D1 connections (`studio/d1.rs` already handles D1 connections).
- **Fast-forward:** only the ref moves.
- **Divergent histories:** handled with the P2 merge.

### Content-addressed commit ids

Commit ids are local tick numbers today. Sync needs ids that are equal in every store holding the same
history: a hash of the parents and the sorted change set, stored in `commits`. Hash term ids (D2) are equal on
every client, so the change set hashes identically without re-encoding.
