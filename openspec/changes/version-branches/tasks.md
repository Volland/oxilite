## 1. Gate

- [ ] 1.1 BEAR-B subset: as-of latency against the present, with the as-of index
- [ ] 1.2 Decide on checkpoints (materialized snapshots every N commits) for full-history reads; record it as a decision

## 2. Branches

- [ ] 2.1 `refs`, `branch`, list and delete; writes to a named branch; branch-aware version references
- [ ] 2.2 Ancestor-set visibility for as-of on branches
- [ ] 2.3 `checkout` by diff, with logging suspended

## 3. Merge

- [ ] 3.1 Three-way merge, fast-forward, conflict policies
- [ ] 3.2 SHACL-gated merge

## 4. Surfaces and docs

- [ ] 4.1 CLI `branch`, `checkout`, `merge`; JavaScript packages; studio branch switcher and merge review
- [ ] 4.2 Tests on every engine; `lat.md`; website
