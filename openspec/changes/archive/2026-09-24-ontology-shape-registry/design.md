## Context

See proposal.md for the motivation. The usual oxilite constraints apply:
- the engine may be remote (D1 bills every row and index entry, has no UDFs, and a batch is its
  only atomic unit);
- the core stays sans-IO: an operation produces `Request`s and consumes `Responses`;
- derived data never replaces the source (D18's rule, applied here to schema graphs).

This change adds decision D22 in `lat.md/decisions.md`.

## Goals / Non-Goals

**Goals:**
- A graph can be declared to *be* an ontology or a shapes graph, and that declaration is
  queryable, versionable and reversible.
- Reasoning can be scoped to chosen ontologies instead of the whole dataset.
- SHACL shapes have one source of truth, shared by Cypher and by rudof.
- Reading the shapes costs no round trip on a Cypher write.
- A store that registers nothing behaves exactly as before, bit for bit.

**Non-Goals:**
- `owl:imports` resolution, shape derivation, validate-on-write for SPARQL UPDATE, CLI and
  bindings (see proposal.md).
- Per-graph closures. One `tbox_closure` still serves the whole store; the registry decides
  which graphs *feed* it, not how many closures exist.
- Hiding schema graphs from `named_graphs()` or from `dump_to_writer()`. The dataset is still
  the dataset; hiding is a query option.

## Decisions

### Register graphs, do not move triples (D22)

A schema graph is an ordinary named graph. `schema_graphs` labels it:

```sql
CREATE TABLE schema_graphs (
  g INTEGER PRIMARY KEY, role INTEGER NOT NULL,
  iri TEXT, version TEXT, sha256 TEXT, imports TEXT,
  active INTEGER NOT NULL DEFAULT 1, loaded_at REAL NOT NULL) STRICT
```

*Alternative:* separate `ontology_quads` / `shape_quads` tables. Rejected. SHACL shapes are RDF
that people query, and `GRAPH ?g` already separates them; a second table would force a `UNION`
into every pattern scan, which is the one thing the compiler exists to avoid. Registering also
makes drop and replace a single `DELETE FROM quads WHERE g = ?` with no read, exactly as
`jsonld_graphs` does for documents.

`role` is an integer, not a class IRI, because it is a storage-level discriminator and the SQL
that filters on it must be constant-folded at compile time.

### Empty registry means "every graph"

Every scoping predicate has the shape

```sql
(NOT EXISTS (SELECT 1 FROM schema_graphs WHERE role = R AND active = 1)
 OR g IN (SELECT g FROM schema_graphs WHERE role = R AND active = 1))
```

So a store that never registers anything keeps today's behaviour — the closure reads all
graphs, the shape index sees all graphs — and registering the first ontology is the act that
narrows it. This makes the feature purely additive and keeps the existing test suites valid.

*Alternative:* a `StoreOptions` flag choosing between the two modes. Rejected: it makes the
same database behave differently depending on how it was opened.

### Scoping lives in SQL, not in Rust

The filter is a correlated subquery inside the existing `WITH RECURSIVE … INSERT OR IGNORE`
statements, so `closure_statements()` stays a pure function of no inputs, still runs inside a
write's atomic request, and still costs one request on D1. Nothing needs to read the registry
before writing.

### Hiding schema graphs goes through `Entailment::base()`

`QueryOptions::include_schema_graphs = false` makes `Entailment::base()` return
`(SELECT s, p, o, g FROM quads WHERE g NOT IN (SELECT g FROM schema_graphs))` instead of
`quads`. Every quad source in the compiler already goes through `base()` or `source()`, so one
change covers patterns, paths, `OPTIONAL`, `GRAPH ?g` and the merged-default-graph
`NOT EXISTS`. `Entailment::active()` is deliberately *not* changed, so turning hiding on does
not switch the default graph onto the `GraphFilter::Merge` path and does not alter dedup
semantics.

Hiding ignores `active`: an inactive ontology graph is still schema, still noise.

### The shape index is compiled in SQL, not by running SPARQL

`shapes_index` and `shapes_in` are filled by `INSERT … SELECT` statements written directly
against `quads` and `terms`, with an `rdf:rest*/rdf:first` walk as a recursive CTE for `sh:in`.

*Alternative:* keep `schema_query()` and have Rust write the rows back. Rejected: that is a read
followed by a write, so it cannot live inside the write's batch (D5), and it would leave the
index stale exactly when a write depends on it.

The index stores **text**, not term ids: target, path and datatype are IRIs, `sh:pattern` is a
string. `sh:in` values can be any term, so `shapes_in` keeps the id plus the `terms` columns
(`lex`, `dt`, `lang`, `dir`) and Rust rebuilds them with the existing `decode_row` /
`decode_inline`, with no second request.

Several shapes may target the same `(target, path)`. The SQL aggregates with `GROUP BY` and
`MAX(...)`, which ignores NULLs and so reproduces the field-by-field merge the current Rust
loop performs, but deterministically.

### Shape refresh is triggered by SHACL predicates

`shapes::is_shape_quad` mirrors `reason::is_schema_quad`: writing any of `sh:targetClass`,
`sh:property`, `sh:path`, `sh:datatype`, `sh:minCount`, `sh:maxCount`, `sh:pattern`, `sh:class`,
`sh:node`, `sh:in` refreshes the index in the same atomic request.

`rdf:first` / `rdf:rest` are **not** triggers, although `sh:in` lists are built from them.
Including them would refresh the index on every write that touches any RDF list anywhere in the
dataset. The consequence is documented: appending to an existing `sh:in` list without touching a
`sh:` predicate leaves the index stale until the next `optimize()`. In practice a shape and its
value list are written together, and the `sh:in` triple itself is a trigger.

## Risks / Trade-offs

- **Cypher regression risk.** The Cypher schema path moves from a SPARQL query to an index read.
  The two must agree, including the `sh:in` list walk and the multi-shape merge. Mitigated by
  keeping `schema_query()` and `Shapes::from_output` in place and adding an agreement test that
  compares both paths on the same dataset.
- **Statement count.** A write that touches a SHACL predicate now carries the shape-refresh
  statements as well as the closure statements. Both are bounded by schema size, not data size,
  and only fire on schema writes.
- **`STRICT` tables and NULL counts.** `min_count` / `max_count` are nullable integers decoded
  from inline ids; a non-canonical integer literal falls back to `terms.num`. Anything else
  decodes to NULL, i.e. "not declared", which is the same as the current Rust parse failure path.

## Migration Plan

The three tables are created with `IF NOT EXISTS` by `create_schema`, which every `open()`
already runs, so an existing database gains empty tables and no behaviour change. No data
migration, no version gate, nothing to roll back: dropping the tables restores the previous
behaviour exactly.
