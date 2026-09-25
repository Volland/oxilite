# Versioned store — design

Time travel over an immutable quad log, Git-style branches and merge, and push/pull between stores. Evaluation and go/no-go: [assessment.md](assessment.md).

## Phase 1 as built (2026-09-25)

Phase 1 shipped with these differences from the design below. [`lat.md/architecture.md` § Versioning](../../../lat.md/architecture.md) is the reference for the implementation, and decisions D29–D32 record the rationale.

- **Levels are `off`, `stamped` and `log`.** `history` (branches) is phase 2. The as-of index is a `log` option (`as_of_index`), not a level.
- **The clock is `ticks` itself.** Every write opens a tick with `INSERT INTO ticks … SELECT max(t) + 1`, carrying its time, author and message. There is no counter row. The store adds that statement to any atomic request that writes `quads` (`version::prepare`, run by `VersionedBackend` in Rust and by the job context in wasm), so writers are unchanged.
- **The genesis always records the whole store.** Each existing quad becomes one log row, so the snapshot is no longer optional. Inferring the baseline from un-logged quads is wrong after a freeze and resume; see D31.
- **The log keeps a `tx` index.** Listings, changes and diffs need it, which puts `log` at +42 % rows written rather than +21 %. The as-of index adds 2 more rows per change.
- **A tick records its net effect.** If a change is undone within the same tick, its log row is deleted. Only the current tick is mutable, and only a purge deletes older rows.
- **Commit ids are ticks.** Content-addressed ids wait for phase 3.
- **Not built yet:**
  - the `<oxilite:history>` graph
  - the Datalog `at` suffix and history built-ins
  - a Cypher version option (Cypher reads `quads` directly)

## Context

oxilite changes rows in place: `quads` is updated by `INSERT OR IGNORE` and `DELETE`, so history is lost. The goal is three features:
1. **Time travel:** query any past state.
2. **An immutable source of truth:** rows are appended, never updated or deleted.
3. **A Git model:** commits, branches, merge, and push/pull between stores (local SQLite ↔ D1).

Decisions already made:
- An append-only log plus a `quads` table that caches the current branch head.
- The work is phased: P1 time travel, P2 branches and merge, P3 push/pull.

This ships as an opt-in module on the existing store, not as a separate engine. There's no drop-in library that runs on D1. We borrow designs instead:
- **Datomic/XTDB:** as-of semantics.
- **TerminusDB:** add/remove delta layers.
- **Quit Store and R43ples:** Git over SPARQL.
- **BEAR benchmark (from OSTRICH):** test corpus.
- **PROV-O:** vocabulary for commit metadata.

Why it fits oxilite: hash term IDs (D2) are identical on every client. As a result:
- Commits can be content-addressed.
- Log rows can be copied between stores without remapping.

## Design

### Versioning levels

Versioning is a store option, `StoreOptions::versioning`, with four nested levels. Each level adds to the one before and adds nothing to the levels below it. The default is `off`, which is today's store, byte for byte. The pattern already exists: `graph_index` and `text_index` are optional schema parts, each measured by `bench` `write-cost`.

| Level | Adds | Gives | Extra D1 rows written |
|---|---|---|---|
| `off` (default) | nothing | today's D1-optimized store | 0 |
| `stamped` | `quads.t` column, `ticks` table, clock counter | when each present quad was added, a monotonic order, "added since" queries, a feed of inserts only | 0 per quad; +2 per batch |
| `log` | `quad_log` (primary key only), `commits` (linear), capture triggers | time travel, a change feed including deletes, history as data, `purge` | +1 per change; +4 per batch |
| `history` | full log indexes, `refs`, branches, merge (phase 2) | fast as-of on any pattern, branches, merge, push/pull | +3 per change; +4 per batch |

**Cost to the `off` store.** The design guarantees there is none:
- **Schema:** no table, column or trigger is created unless its level is chosen. D1 migrations come from `schema_sql(options)` as today, so an `off` database gets today's migration.
- **Write path:** `off` emits today's statements. `stamped` and above differ only in the quad insert, which gains a `t` column.
- **Read path:**
  - `Entailment::base()` branches once, on `as_of`, which is `None` by default.
  - A version-only construct on an `off` or `stamped` store is refused with "store keeps no history". This covers `as_of`, `SERVICE <oxilite:version/…>`, `<oxilite:history>`, and the Datalog `@version`, `at` and history built-ins.
  - A golden test asserts that the generated SQL and plans for the default store are identical to those before this change, across the BSBM and compatibility queries.
- **Statement budget:** only batches from `log` and up pay the commit open/close statements (+3 statements against D1's 50 per batch). `off` batches keep the full budget.
- **Engineering:** the part that isn't free is the test matrix. The full suite runs on `off` (default) and `history`. `stamped` and `log` get targeted suites. Each level is also added to the `write-cost` bench.

The level is recorded in `oxilite_meta` (`versioning`) and read at open, like `graph_index`. How users choose and change it is described in [Choosing and changing the level](#choosing-and-changing-the-level).

### Choosing and changing the level

**Choosing at creation.** The level is one option, spelled the same way on every surface. Two sub-options go with it:
- `stampIndex`: an index on `t` for fast "added since" queries, +1 row per quad.
- `snapshot`: used when upgrading.

| Surface | Create a store at a level |
|---|---|
| Rust | `Store::open_with_options(path, StoreOptions { versioning: Versioning::Log, ..Default::default() })` |
| CLI | `oxilite --location db.sqlite --versioning log …` (like today's `--text-index`) |
| JS / D1 driver | `new Engine(null, JSON.stringify({ versioning: "log" }))`, and `{ versioning: "log" }` in the `@oxilite/d1` / `@oxilite/node` options |
| D1 migration | `oxilite_d1::migration_sql(&StoreOptions { versioning: Versioning::Log, .. })`, which changes the signature from `graph_index: bool`. Also a new subcommand, `oxilite schema --versioning log > migrations/0001_oxilite.sql`. |

**Opening an existing store.** The stored level always wins:
- Opening with a lower level, or with no option, keeps the stored level. Opening never downgrades.
- Opening with a higher level is refused with "use `set_versioning` to upgrade".

This is where versioning deliberately differs from `text_index`. That option upgrades implicitly at open. An upgrade to `log` is an explicit choice: it is billed on D1, it may take a snapshot, and it starts history. It must not happen because an old config file sets a flag.

**Changing the level on a live store.** Level changes are explicit, one-step-at-a-time operations. Multi-step changes are applied as a sequence. Each step is one atomic request and records the change:
- Rust: `store.set_versioning(Versioning::Log, LevelChange { snapshot: false, message: Some(..), allow_loss: false })`
- CLI: `oxilite versioning set log [--snapshot] [--allow-loss]`
- JS: `engine.setVersioning("log", { snapshot })`
- Status: `store.versioning()` returns the level, sub-options, genesis tick and any history gaps. CLI: `oxilite versioning status`.

**D1.** Production D1 schemas change through `wrangler d1 migrations`, and `open_existing` runs no DDL. The same change can therefore be emitted as a migration:
- `oxilite versioning migration --from off --to log [--snapshot] > migrations/0004_versioning.sql`
- `oxilite_d1::level_change_sql(from, to, &LevelChange)`

After applying it, the Worker opens with the new level. Running `set_versioning` directly against D1 through the sidecar or studio connection also works; the studio asks for confirmation and shows its row estimate.

**Upgrades.** These are always possible and never lose data.

| Step | What happens | Cost |
|---|---|---|
| `off → stamped` | `ALTER TABLE quads ADD COLUMN t … DEFAULT 0` (metadata only, verified). Creates `ticks`, starts the clock. Existing quads read tick 0 ("before stamping"). | a few statements, no per-quad rows |
| `stamped → log` | Creates `quad_log`, `commits` and the capture triggers. Writes a **genesis** commit. As-of before genesis is an error. With `snapshot`, all current quads are copied into the log as genesis additions, so history is complete from here. | no snapshot: a few rows; snapshot: 1 row per quad (lean log) |
| `log → history` | Adds the two log indexes (built from existing log rows) and `refs` with `main` at the head. | 2 index entries per log row, once |

**Downgrades.** These are allowed, never silent, and never pretend history exists:

| Step | Default (soft, keeps data) | With `--allow-loss` (hard) |
|---|---|---|
| `history → log` | Refused if any branch other than `main` exists, because those commits would become unreachable. Otherwise drops `refs` and the extra log indexes. | Drops other branches too; their commits remain in the log but are no longer named. |
| `log → stamped` | Drops the capture triggers and **freezes** the history. It stays read-only and queryable as of any commit up to the freeze. The freeze is recorded, so a later re-upgrade shows a **gap**: as-of inside the gap is an error naming it. | Also drops `quad_log` and `commits`, after an audited record like `purge`. |
| `stamped → off` | Stops stamping. `t` and `ticks` stay; new quads get tick 0. No table rewrite, so it is free on D1. | Drops `quads_t`, then `ALTER TABLE quads DROP COLUMN t`, which rewrites the whole table (verified: it works once the index is dropped first; billed as a rewrite on D1) and drops `ticks`. |

**Re-upgrading after a downgrade** is an ordinary upgrade. `stamped` resumes the clock from its stored value, so ticks stay monotonic. `log` writes a new genesis commit whose parent is the frozen head, so the frozen history and the gap stay visible in `log()` and `<oxilite:history>`.

### Monotonic timestamp (`stamped`)

`t` is a store-wide logical clock: one tick per atomic batch.
- The batch begins with `UPDATE oxilite_meta SET value = value + 1 WHERE key = 'clock'` and `INSERT INTO ticks(t, time) SELECT value, unixepoch('subsec') …`. This is 2 rows written per batch, whatever its size.
- Quad inserts take `t` from an uncorrelated scalar subquery on the clock, which SQLite evaluates once per statement. That is one extra row read per statement.
- The clock is strictly monotonic because D1 and SQLite have a single writer. `ticks` maps each tick to wall time, so `t` doubles as a timestamp, while ordering never depends on clocks.
- `INSERT OR IGNORE` doesn't touch an existing row, so `t` is the time the quad was **first added since it was last absent** (verified).
- A delete leaves no trace at this level. `stamped` is therefore an insert clock, not history: it can't answer as-of queries.
- `t` is in the table only, not in the secondary indexes. Queries that don't mention `t` stay index-only. "Added since" queries scan unless the optional `quads_t` index is chosen (+1 row per quad).
- **Rejected:** a client-side hybrid logical clock (HLC). It avoids the counter row, but it isn't strictly monotonic across clients, and `log` needs a total order anyway.

`log` and `history` reuse the same clock as their `tx`, which is why the levels nest.

### Schema (added in `crates/oxilite-core/src/schema.rs#create_schema` by level)

- `quad_log(s, p, o, g, tx, op)`:
  - `PRIMARY KEY (s,p,o,g,tx) WITHOUT ROWID, STRICT`, with `op` 1 = add and 0 = remove.
  - `log` level: the primary key only. `history` level: adds indexes `(p,o,s,g,tx)` and `(o,s,p,g,tx)`, mirroring D3.
  - `BEFORE UPDATE` and `BEFORE DELETE` triggers call `RAISE(ABORT, 'oxilite: log is immutable')`.
  - Only *effective* changes are logged: a re-add of a present quad or a removal of an absent one writes nothing.
- `commits(tx INTEGER PRIMARY KEY, id TEXT UNIQUE, parent INTEGER, parent2 INTEGER, branch TEXT, time REAL, author TEXT, message TEXT)`:
  - `id` is xxh3/sha256 over (parents, sorted delta), so commits are content-addressed.
  - `tx` is a local sequence number used for fast range filters.
  - Rows are append-only (triggers).
- `refs(name TEXT PRIMARY KEY, tx INTEGER NOT NULL)` (`history` level): branch heads. This is the only mutable versioning table, as in Git.
- `ticks(t INTEGER PRIMARY KEY, time REAL NOT NULL)` and the `quads.t` column (`stamped` level and up); see [Monotonic timestamp](#monotonic-timestamp-stamped).
- `oxilite_meta`:
  - `head` = the branch currently materialized in `quads`.
  - `versioning` = the level; `clock` = the last tick.
  - `schema_version` stays 1 for `off` stores. Versioned tables come from `schema_sql(options)`, so D1 gets them only in a versioned store's migration.
- `quads`, `quads_inf` and the other tables are unchanged. `quads` is now documented as the materialized HEAD cache, rebuildable from the log.

### Write path (P1)

**Triggers capture the log, so no write path changes.** `AFTER INSERT ON quads` and `AFTER DELETE ON quads` triggers append to `quad_log`:
- The `tx` value is the current tick: `(SELECT value FROM oxilite_meta WHERE key = 'clock')`, advanced once at the start of the batch.
- Each trigger has a `WHEN` guard on `oxilite_meta.log_on = '1'`, which is off during checkout and rebuild.

Two SQLite properties make this correct:
- A row that `INSERT OR IGNORE` ignores does not fire the trigger.
- A `DELETE` that matches no row does not fire it.

The log therefore records only effective changes, with no extra SQL.

D1 supports triggers. Every existing writer is captured automatically, so none can bypass the log: SPARQL UPDATE, bulk load, Cypher writes, JSON-LD/VC documents, and the studio.

A versioned write batch becomes:
1. Open the commit: advance the clock, and insert the `ticks` and `commits` rows.
2. Run the unchanged write statements.
3. Close the commit: advance `refs` (`history` level). A guard rejects a close for an empty change set, if wanted.

The hook is a prefix and suffix around existing batches. `writer.rs#atomic_request` already takes a prefix and suffix, so it can be reused. Bulk loads that span batches become one commit per batch, or one commit that stays open until the load ends; this is documented, like the existing non-atomic `BulkLoader`.

### Read path: as-of

- Add `QueryOptions::as_of: Option<AsOf>`, where `AsOf = Commit(tx) | Time(f64) | Branch(name)`, in `crates/oxilite-core/src/compiler/mod.rs`.
- `Entailment::base()` (`crates/oxilite-core/src/reason.rs:332`) is the single quad source for SPARQL, Cypher and Datalog. When `as_of` is set, it returns a derived table instead of `quads`:
  ```sql
  (SELECT s,p,o,g FROM quad_log l WHERE l.tx IN <visible> AND l.op = 1
     AND NOT EXISTS (SELECT 1 FROM quad_log r WHERE r.s=l.s AND r.p=l.p AND r.o=l.o AND r.g=l.g
                     AND r.tx > l.tx AND r.tx IN <visible>))
  ```
  - P1 (linear history): `<visible>` is `tx <= T`.
  - P2 (branches): `<visible>` is the ancestor set, a recursive CTE over `commits.parent/parent2`. It is computed once per query as a CTE, and emitted inline when the history is linear.
- `hide_schema` and `include_inferred` compose as they do today. With `as_of`, inferred quads are excluded unless `quads_inf` is re-materialized, and that limitation is documented.
- SPARQL surface: the option can be set from the API, CLI (`--as-of`) and studio. Optionally, the named-graph convention `FROM <oxilite:commit/abc123>` maps to `as_of` in the dataset resolver.

### Query surface (SPARQL and Datalog)

**Shared version reference:** `VersionRef` is parsed once in `oxilite-version` and resolves to a `tx` in one request. The accepted forms are:
- `main` (a branch head)
- `main~3` (three commits back)
- `main@2026-09-01T12:00:00Z` (the state at a time)
- `a1b2c3` (a commit id prefix)
- `HEAD`

There are three levels, from cheapest to most expressive:

1. **Whole query at one version, with no syntax change.**
   - `QueryOptions::as_of` is set from the API, CLI `--as-of main~3`, HTTP `?version=main~3`, or studio.
   - Datalog adds a directive: `@version "main~3" .`
2. **Per-pattern version inside one query, to compare versions.**
   - SPARQL uses the standard `SERVICE` syntax, so spargebra needs no grammar fork: `SERVICE <oxilite:version/main~3> { ?s ex:status ?old }`. The compiler recognizes the reserved IRI scheme and compiles the inner group with `base()` swapped to that version's derived table. `GRAPH` inside still selects the named graph.
   - Datalog adds an `at` suffix on atoms (the parser and lexer are our own): `ex:status(?s, ?old) at "main~3"`.
3. **History as data, for queries about change itself.**
   - SPARQL gets a virtual graph `<oxilite:history>` in PROV-O. A commit is a `prov:Activity` with `prov:startedAtTime`, `prov:wasAssociatedWith` and parents. Changes use RDF 1.2 triple terms, for example `?c ov:added <<( ?s ?p ?o )>>` and `ov:removed`. It compiles to joins over `commits` and `quad_log`; no storage is added.
   - Datalog gets built-in relations beside `triple/3` and `triple/4`:
     - `commit(?c, ?parent, ?time, ?author)`
     - `added(?s, ?p, ?o, ?g, ?c)`
     - `removed(?s, ?p, ?o, ?g, ?c)`
     - `branch(?name, ?c)`

     Ancestry is then plain Datalog recursion: `ancestor(?a, ?b) :- commit(?a, ?b, _, _).` and `ancestor(?a, ?c) :- commit(?a, ?b, _, _), ancestor(?b, ?c).`
   - **Temporal Datalog:** `atom at ?c` is allowed when a positive `commit(?c, …)` atom binds `?c`. This is added to the safety check in `program.rs#analyse`.

**Writes:**
- Target a branch with the `branch` option or HTTP `?branch=dev`, and give the commit message with the `message` option.
- An update that names a past version is rejected, because history is immutable.
- Datalog materialization writes to the head branch only.

### Branches and merge (P2)

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

### Push/pull (P3)

- **Exchange unit:** commits missing on the other side, plus their `quad_log` rows and the `terms` and `triple_terms` rows they reference.
- **Format:** N-Quads delta patches (RDF Patch style `A`/`D` lines), so the format is readable and survives across backends. Hash IDs make re-encoding deterministic.
- **Commands:** `oxilite push/pull <remote>` in `oxilite-cli`. Remotes are file paths or D1 connections (`studio/d1.rs` already handles D1 connections).
- **Fast-forward:** only the ref moves.
- **Divergent histories:** handled with the P2 merge.

### Crate layout

- **New crate `crates/oxilite-version`.** It contains commit/branch/merge/diff/push/pull as sans-IO step machines (the `job.rs` pattern).
- **Core changes stay minimal:**
  - schema DDL
  - the `as_of` option
  - the `base()` swap
  - the write hook
- **API:** `crates/oxilite/src/version_store.rs` exposes it, mirroring `datalog_store.rs` / `schema_store.rs`.
- **Wasm/JS:** exposed through `oxilite-wasm` like the other jobs.

### Tradeoffs to document (new decisions D29–D31 in `lat.md/decisions.md`)

- **D29:** The log is the truth and `quads` is a cache. Current-state queries stay unchanged; the cost is +21–62 % rows written on D1, depending on the log indexes (see Part B). A trigger captures the log, so no writer can bypass it. Rejected: log-only (every query is slower) and validity intervals (rows get updated, so the table isn't immutable).
- **D30:** Content-addressed commits over hash IDs make pull a row copy.
- **D31:** Set-semantics three-way merge; conflicts are only add-vs-remove.
- **Open item:** compaction/snapshots for long histories (checkpoint tables), deferred until BEAR numbers exist.

## Files

- `crates/oxilite-core/src/schema.rs`: DDL by level, `StoreOptions::versioning`, level upgrades.
- `crates/oxilite-core/src/reason.rs` (`Entailment::base`) and `compiler/mod.rs` (`QueryOptions::as_of`).
- `crates/oxilite-core/src/writer.rs` and `update.rs`: logged writes.
- `crates/oxilite-core/src/compiler/mod.rs`: compiles `SERVICE <oxilite:version/…>` and the virtual `<oxilite:history>` graph.
- `crates/oxilite-datalog/src/{lexer,parser,ast,program,sql}.rs`: the `@version` directive, the `at` suffix, and the built-ins `commit/added/removed/branch`.
- `crates/oxilite-d1`: migration for the new tables.
- `crates/oxilite-version/` (new) and `crates/oxilite/src/version_store.rs`.
- `crates/oxilite-cli/src/main.rs`: `commit/log/branch/checkout/merge/diff/push/pull/--as-of`.
- `lat.md/architecture.md`: new "Versioning" section, with "Storage schema" and "Updates and atomicity" updated. Also `lat.md/decisions.md` (D29–D31) and `lat.md/tests.md` (a "Versioning" spec section with `@lat:` refs).

## Verification

Per phase, in `crates/oxilite/tests/versioning.rs`:
- **Log immutability:** `UPDATE`/`DELETE` on `quad_log` aborts. A re-insert or a remove of an absent quad logs nothing.
- **As-of equivalence:** a random sequence of updates is applied. For every commit T, SPARQL `SELECT * {?s ?p ?o}` with `as_of = T` equals a snapshot captured at T. The same holds for Cypher and Datalog.
- **Head cache consistency:** `quads` equals the as-of-head view after every write, checkout and merge.
- **Merge:** disjoint changes merge cleanly, add-vs-remove is reported, and the SHACL gate aborts on a violation.
- **Push/pull round trip:** a local store is pulled into a fresh store, and both have identical commit ids and as-of results.
- **D1:** the same tests run through the Miniflare JS driver, checking that batches stay under 50 statements and 90 KB.
- **Performance:** the BEAR-B subset in `bench/`, comparing as-of query latency against head queries.
- **Lint and docs:** `cargo test --workspace` and `cargo clippy` (with `PATH=~/.cargo/bin:$PATH`), then `lat check`.

