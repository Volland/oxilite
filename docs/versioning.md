# Versioning: reference

oxilite can keep the history of a store in the same SQLite file or Cloudflare D1 database: a store clock, an
immutable change log, and queries on any past version. This page is the reference. For a guided tour, see
[Time travel for your knowledge graph](https://oxilitedb.com/articles/time-travel). For the design and the
costs, see [What history costs on D1](https://oxilitedb.com/articles/versioning-on-d1).

- [Levels](#levels)
- [Creating a versioned store](#creating-a-versioned-store)
- [Commits](#commits)
- [Version references](#version-references)
- [Querying the past](#querying-the-past)
- [Reading the history](#reading-the-history)
- [History as data](#history-as-data)
- [Changing the level](#changing-the-level)
- [Purge](#purge)
- [Performance](#performance)
- [Schema](#schema)
- [Errors](#errors)
- [Limits and roadmap](#limits-and-roadmap)

## Levels

Versioning is a per-store option with three nested levels. Each level keeps everything the lower levels keep.

| Level | The store keeps | Typical use | Rows written per triple on D1 |
|---|---|---|---|
| `off` (default) | the current state | caches, derived stores, anything that doesn't need history | 4.81 |
| `stamped` | a store clock: every atomic write is a *tick* with its time, author and message, and every quad records the tick that added it (`quads.t`) | "what's new since", incremental export, search indexing, sync feeds | 4.82 |
| `log` | an immutable change log of every addition and removal; any past version can be queried | audit, agent memory, curated graphs, undoing an import, reproducible analysis | 6.82 |
| `log` + as-of index | the log indexed by predicate and object as well | reading the past often | 8.82 |

- **An `off` store is the store as it was before versioning existed.** It has the same tables and pays the
  same write cost. A golden test fixes its SQL for the W3C corpus to that of oxilite 0.3.1.
- **Queries on the current state cost the same at every level.** Only a query that names a past version reads
  the log.
- **The level is recorded in the database** (`oxilite_meta.versioning`), and every driver reads it at open.

## Creating a versioned store

The level applies when a store is created: that is, when it is opened with the option while still empty.
Opening an existing store never changes its level (see [Changing the level](#changing-the-level)).

**Rust**

```rust
use oxilite::store::Store;
use oxilite::version::Versioning;
use oxilite::StoreOptions;

let store = Store::open_with_options("kb.sqlite", StoreOptions {
    versioning: Versioning::Log,  // Off | Stamped | Log
    as_of_index: false,           // Log only: index the log by predicate and object
    stamp_index: false,           // Stamped or Log: index quads.t ("added since")
    ..Default::default()
})?;
```

**Command line.** Any command that opens a store takes the store options:

```bash
oxilite update -l kb.sqlite --versioning log [--as-of-index] [--stamp-index] -u 'INSERT DATA { … }'
```

**Node.js** (`@oxilite/node`)

```ts
const store = new Store({ path: "kb.sqlite", versioning: "log", asOfIndex: false });
```

**Cloudflare D1** (`@oxilite/d1`)

```ts
const store = await D1Store.open(env.DB, { wasm, versioning: "log" });
```

On D1, production schemas usually come from migrations:

```bash
npx oxilite-d1 schema --versioning log [--as-of-index] > migrations/0001_oxilite.sql
# or, with the Rust CLI:
oxilite schema --versioning log > migrations/0001_oxilite.sql
```

The Worker then opens the store with `{ migrated: true }` and reads the level from the database.

## Commits

- **Every atomic write is one tick.** That covers a SPARQL update, an `insert`/`remove`/`extend` call, one
  batch of a bulk load, a Cypher write, and a JSON-LD document or credential. At level `log`, a tick that
  changed something is a commit. An interactive transaction (the Rust fallback for updates that don't
  compile to one statement, and Cypher writes on native stores) is one tick. On D1, one batch is one tick.
- **Only effective changes are recorded.** Re-adding a quad that is present, or removing one that is absent,
  records nothing. A quad removed and re-added in the same write, or added and removed, leaves no trace: each
  commit records its net effect.
- **Every writer is captured.** Triggers on the quad table write the log, so no write path can skip it.
- **Author and message** are recorded on the ticks of later writes:

| Surface | Author and message |
|---|---|
| Rust | `store.set_commit_info(CommitInfo { author, message })`, or `store.with_commit(info, \|s\| …)` for one block of writes |
| CLI | `--author NAME -m "message"` on any command |
| JavaScript | `store.setCommitInfo({ author, message })`, or `await store.withCommit(info, async (s) => …)` |
| HTTP | `oxilite serve --author … -m …` sets a server-wide default |

## Version references

| Form | Means |
|---|---|
| `HEAD`, `main` | the latest tick |
| `HEAD~n`, `main~n`, `~n` | *n* commits before the latest |
| `#42`, `42` | tick 42 |
| `@2026-09-01T12:00:00Z`, `HEAD@2026-09-01`, `@2026-09-01` | the latest tick at or before that instant (`xsd:dateTime` or `xsd:date`; no time zone means UTC) |

A version must lie inside the recorded history:
- A version before the genesis commit is an error.
- A version inside a gap is an error. A gap is the period between a freeze and a resume, when nothing was
  recorded.
- The tick of a freeze itself is valid.

## Querying the past

**SPARQL.** A whole query can read one version:

| Surface | How |
|---|---|
| Rust | `store.query_opt(q, QueryOptions { as_of: Some("HEAD~1".into()), ..Default::default() })` |
| JavaScript | `store.query(q, { as_of: "HEAD~1" })` |
| CLI | `oxilite query -l kb.sqlite --as-of HEAD~1 -q '…'` (also `explain --as-of` natively) |
| HTTP | `GET /query?query=…&version=HEAD~1` on `oxilite serve` |

Everything in the query reads that version: patterns, property paths, `OPTIONAL`, `GRAPH`, `DESCRIBE`. It
still compiles to one SQL statement.

**Comparing versions in one query.** `SERVICE <oxilite:version/REF>` evaluates its group at another version:

```sparql
SELECT ?t ?old ?new WHERE {
  ?t ex:status ?new
  SERVICE <oxilite:version/HEAD~1> { ?t ex:status ?old }
  FILTER(?old != ?new)
}
```

**Datalog.** A program reads one version, from a directive or an option:

```
@version "HEAD~1" .
?- ex:status(?t, "open").
```

`Options { as_of: Some(…) }` in Rust, `{ asOf: "…" }` in JavaScript, `--as-of` on `oxilite datalog`.

**Per atom, in Datalog.** An atom that reads the store takes `at "REF"`, or `at ?c` for the commit a positive
atom binds:

```
changed(?t, ?old, ?new) :- ex:status(?t, ?new), ex:status(?t, ?old) at "HEAD~1", ?old != ?new.
status_at(?c, ?v)       :- commit(?c, _, _, _), ex:status(ex:t1, ?v) at ?c.   % the value at every commit
```

**Cypher.** Set `asOf` in the options: `store.cypher(q, {}, { asOf: "HEAD~1" })` in JavaScript, and
`CypherOptions { query: QueryOptions { as_of: Some("HEAD~1".into()), .. }, .. }` in Rust. The version is
resolved once per statement. Matching and node and relationship materialization read that version. A
writing statement with a version is refused.

**Not combinable with a version:**
- Materialized inferences (`include_inferred`), query-time reasoning, and Datalog materialization. They
  describe the current state.
- Updates and writing Cypher statements. Writes apply to the current state, and an update's `WHERE` can't
  read a past version.

## Reading the history

| | Rust | JavaScript | CLI |
|---|---|---|---|
| Status | `versioning()` → `VersionStatus` | `versioning()` | `oxilite versioning status [--json]` |
| Commits and level changes, newest first | `history(limit)` → `Vec<CommitRecord>` | `history(limit)` | `oxilite versioning log [-n 20] [--json]` |
| Every change after a tick | `changes(after, until)` → `Vec<Change>` | `changes(after, until?)` | `oxilite versioning changes --since 40 [--until 90]` |
| Net difference between versions | `diff(from, to)` → `Vec<Change>` | `diff(from, to = "HEAD")` | `oxilite versioning diff HEAD~3 [HEAD]` |
| A reference's tick | `resolve_version("HEAD~2")` | — | — |

JSON shapes (JavaScript and `--json`):

```jsonc
// VersionStatus
{ "level": "log", "history": "live" /* none | live | frozen */, "stampColumn": true, "stampIndex": false,
  "asOfIndex": false, "head": 42, "headTime": 1790000000.123, "genesis": 2, "frozenAt": null, "commits": 37 }
// CommitRecord
{ "tick": 42, "time": 1790000000.123, "kind": "write" /* genesis | freeze | resume | dropped | level | purge */,
  "author": "ada", "message": "close t1", "added": 1, "removed": 1 }
// Change
{ "tick": 42, "added": true, "quad": /* RDF/JS quad */ }
```

At level `stamped`, `changes` reads `quads.t`. It lists the quads present now that were added after the tick;
removals leave no trace at that level. The CLI prints changes and diffs as
[RDF Patch](https://afs.github.io/rdf-patch/) lines, grouped by tick:

```
# #4
A <http://example.com/t1> <http://example.com/status> "done" .
D <http://example.com/t1> <http://example.com/status> "open" .
```

## History as data

The history can be queried like the rest of the dataset. A commit is its tick, an `xsd:integer` (the `42` of
`#42`).

**SPARQL: the graph `<oxilite:history>`**

| Pattern | Binds |
|---|---|
| `?c a prov:Activity` | every commit and level change |
| `?c prov:startedAtTime ?t` | its time (`xsd:dateTime`) |
| `?c prov:wasAssociatedWith ?who` | its author (a string) |
| `?c rdfs:comment ?why` | its message |
| `?c prov:wasInformedBy ?before` | the commit before it |
| `?c oxl:added <<( ?s ?p ?o )>>` | each triple it added |
| `?c oxl:removed <<( ?s ?p ?o )>>` | each triple it removed |

`oxl:` is `https://oxilite.dev/ns#` and `prov:` is `http://www.w3.org/ns/prov#`.

```sparql
PREFIX prov: <http://www.w3.org/ns/prov#>
PREFIX oxl:  <https://oxilite.dev/ns#>
SELECT ?who ?when WHERE { GRAPH <oxilite:history> {
  ?c oxl:removed <<( ex:alice ex:role ex:admin )>> ;
     prov:wasAssociatedWith ?who ;
     prov:startedAtTime ?when } }
```

The triple term's parts may be constants or variables. Changes need level `log`; at `stamped` the graph
lists commits only. A query on the history graph always compiles to SQL; it is never evaluated by the fallback.

**Datalog built-ins**

| Relation | Rows |
|---|---|
| `commit(?c, ?parent, ?time, ?author)` | every commit, the one before it, its time and author |
| `added(?s, ?p, ?o, ?g, ?c)` / `removed(…)` | the log's changes; `?g` is unbound for the default graph |
| `branch(?name, ?c)` | `"main"` and the latest tick (until branches exist) |

```
removed_by(?t, ?v, ?who) :- removed(?t, ex:status, ?v, _, ?c), commit(?c, _, _, ?who).
```

A program that defines rules named `commit`, `added`, `removed` or `branch` keeps its own relation.

## Changing the level

Levels change only on request, one atomic request per change:

| Surface | How |
|---|---|
| Rust | `store.set_versioning(Versioning::Log, LevelChange { as_of_index: Some(true), allow_loss: false, author, message, .. })` |
| JavaScript | `store.setVersioning("log", { asOfIndex: true, allowLoss: false })` |
| CLI | `oxilite versioning set -l kb.sqlite log [--with-as-of-index] [--no-stamp-index] [--allow-loss]` |
| D1 migration | `oxilite versioning migration --from off --to log` or `npx oxilite-d1 versioning-migration --from off --to log` |

| Change | What happens | Cost |
|---|---|---|
| `off → stamped` | `ALTER TABLE quads ADD COLUMN t` (metadata only; existing quads read tick 0), clock starts | a few statements |
| `stamped → log` | the whole store becomes the genesis commit | one log row per quad (two with the tx index), once |
| `stamped → log` after a freeze | a resume commit records the net changes of the gap | one log row per quad that changed during the gap |
| `log → stamped` | capture stops; the history freezes and stays queryable up to the freeze | none |
| `log → stamped`, `allow_loss` | the log and the commits are deleted | none |
| `stamped → off` | the clock stops; `t` and the ticks stay | none |
| `stamped → off`, `allow_loss` | `t` and the ticks are dropped (a table rewrite) | proportional to the store |
| same level, index options | create or drop the as-of index or the stamp index | one index build |

Opening with a higher level than the stored one:
- On an empty store, it applies the level. That is how a store is created versioned.
- On a store holding data, it fails and names the explicit change.

Opening with a lower level, or with no option, keeps the stored level.

For a D1 database that was lowered before, tell the migration generator. Pass `--stamp-column-exists` if the
store was stamped earlier and lowered to `off`, and `--frozen-history` if the history was frozen.

## Purge

For erasure requests, one operation rewrites history. It removes the quads matching a pattern from the store
and from every past version, and records a purge tick with the author and reason, but not the removed content.

```bash
oxilite versioning purge -l kb.sqlite --subject http://example.com/alice --reason "erasure request #1142" --yes
```

In Rust: `store.purge(Some(subject), None, None, None, Some(reason))`. In JavaScript:
`await store.purge({ subject }, reason)`.

## Performance

*Measured on an Apple M2 Max with the bundled SQLite, in memory, by `bench/src/bin/as-of-latency.rs`, and on a
local D1 by `bench/src/bin/write-cost.rs`. Raw results are in `bench/results/`.*

**Writes.** Rows written per triple on D1 (`write-cost`, 5,000 triples):

| Level | Rows per triple |
|---|---|
| `off` | 4.81 |
| `stamped` | 4.82 (3 rows per batch: the tick and its time term) |
| `log` | 6.82 |
| `log` + as-of index | 8.82 |

A versioned batch keeps two of D1's 50 statements for the tick.

The same store on the bundled SQLite (80,000 triples, 500 commits):

| Level | Bulk load | One commit | Size |
|---|---|---|---|
| `off` | 347 ms | 15.8 ms | 12.8 MB |
| `stamped` | 362 ms | 15.9 ms | 13.0 MB |
| `log` | 685 ms | 16.6 ms | 20.2 MB |
| `log` + as-of index | 792 ms | 17.2 ms | 27.2 MB |

**Reads of the present** cost the same at every level.

**Reads of the past**, 250 commits back, as a multiple of the same query on the present:

| Query | Plain log | + as-of index |
|---|---|---|
| subject bound (`<t123> ?p ?o`) | 3.0× | 3.1× |
| value lookup (`?t :status "blocked"`) | 10× | 1.2× |
| star join (four patterns) | 99× | 3.4× |
| aggregate over a whole predicate | 27× | 16.5× |

- **Without the as-of index**, patterns not bound by subject scan the whole log. Their cost grows with the
  store: at 400,000 triples the value lookup is 35×, and 1.1× with the index.
- **A longer history** (2,000 commits) raises subject lookups to about 6× and whole-predicate aggregates to 34×.
  Selective queries with the index stay within 5×.
- **Reading further back is cheaper**, because fewer log rows are visible at an older tick.
- **Don't pass `as_of: "HEAD"` to read the present.** Omit the version instead.

Choose the as-of index if you query the past often. Aggregates over a whole history are the case that
checkpoints, planned with branches, will address. The full analysis is in
[How much does versioning slow oxilite down?](https://oxilitedb.com/articles/versioning-benchmarks).

## Schema

| Object | Level | Content |
|---|---|---|
| `ticks(t, time, kind, author, message, time_id, author_id, message_id)` | `stamped`+ | one row per tick; `t` is `max(t) + 1`, opened by the store in front of every write batch; the `*_id` columns are the terms of its time, author and message (the history graph) |
| `ticks_boundary` | `stamped`+ | partial index on ticks that start or end the recorded history |
| `quads.t` | `stamped`+ | the tick that added the quad (0: before stamping) |
| `quads_t` | option | index on `quads.t` |
| `quad_log(s, p, o, g, tx, op)` | `log` | every effective change; key `(s, p, o, g, tx)`; `op` 1 adds, 0 removes |
| `quad_log_tx` | `log` | index by commit (listings, changes, diffs) |
| `quad_log_posg`, `quad_log_ospg` | option | the as-of index |
| `commits(tx)` | `log` | the ticks that changed something |
| `quads_log_insert`, `quads_log_delete` | `log` | capture triggers on `quads` |
| `*_immutable_*` | `stamped`+ | triggers aborting updates and deletes of past rows ("oxilite: history is immutable") |
| `oxilite_meta` keys | all | `versioning`, `history` (`none`, `live`, `frozen`), `stamp_column`, `stamp_index`, `as_of_index` |

## Errors

| Message | Why |
|---|---|
| `this store keeps no history: raise its versioning level to log …` | a version was asked of a store without a change log |
| `unknown version …` / `invalid version …` | the reference names no tick, or cannot be parsed |
| `version … is before the recorded history` | the tick is before genesis, or before the history was deleted |
| `version … falls in a gap: history was frozen at #… and not recorded after it` | the tick lies between a freeze and a resume |
| `the store's versioning level is …; opening does not change it …` | opening a store holding data with a higher level |
| `inferences and reasoning describe the current state only …` | a version combined with `include_inferred` or reasoning |
| `version … cannot be read here: SERVICE <oxilite:version/…> works in queries, not in updates …` | an update read a past version |
| `oxilite: history is immutable` | a statement tried to change or delete recorded history |
| `this store keeps no change log …` | the history graph's changes, or `added`/`removed`/`at ?c` in Datalog, on a store below `log` |
| `a writing statement cannot run at a past version …` | a Cypher write with `asOf` |
| `the as-of index needs a change log …` / `the stamp index needs the store clock …` | an index option without its level |

## Limits and roadmap

- **Not built yet:**
  - Branches and merge: [`version-branches`](../openspec/changes/version-branches/). It is gated on a
    BEAR-B run and a decision on checkpoints.
  - Push and pull between stores, with content-addressed commit ids:
    [`version-sync`](../openspec/changes/version-sync/).
- **Commit ids are ticks**, local to a store.
- **Bulk loads that span several batches** are one commit per batch. They are not atomic, as without
  versioning.
- **Design and decisions:** [`lat.md/architecture.md` § Versioning](../lat.md/architecture.md) and decisions
  D29–D32 in [`lat.md/decisions.md`](../lat.md/decisions.md). The archived change,
  [`2026-09-25-versioned-store`](../openspec/changes/archive/2026-09-25-versioned-store/), holds the
  assessment, and [`2026-09-25-version-history-queries`](../openspec/changes/archive/2026-09-25-version-history-queries/)
  holds the history queries.
