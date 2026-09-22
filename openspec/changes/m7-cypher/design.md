## Context

See proposal.md for the motivation. The same constraints as the rest of oxilite apply:
- the engine may be remote (D1 bills every row and index entry, has no UDFs, and a batch is its only atomic unit);
- the SQL must parse on SQLite builds with a fixed parser stack (Shallow SQL);
- the core stays sans-IO.

Decisions D13–D17 in `lat.md/decisions.md` record the choices below.

## Goals / Non-Goals

**Goals:**
- Cypher and SPARQL over one dataset, with no copy or sync.
- A Cypher `MATCH` compiles to a single SQL statement whenever the SPARQL equivalent would.
- Cypher writes are atomic in one request on every backend, D1 included.
- When the store has an ontology or shapes, OWL and SHACL change what Cypher returns and how it compiles.
- Pass at least 80% of the read-only openCypher TCK scenarios. Deviations are allow-listed and justified.

**Non-Goals:**
- Full GQL (ISO/IEC 39075). The AST is designed to grow into it later.
- Bolt protocol and Neo4j driver compatibility.
- `owl:sameAs` in Cypher: it breaks id-based joins.
- Point and duration types in the first release.
- APOC and user-defined procedures. Only the schema procedures are built in.
- A native PG storage layout.

## Decisions

### Second frontend over the RDF store (D13)
Cypher is lowered into the same compiler, planner and backends as SPARQL. *Alternative:* a separate PG engine with `nodes` and `edges` tables and JSON properties. It would make relationship properties cheaper and writes lighter, but it would duplicate four backends and the planner. It would also lose SPARQL interop and need an invented OWL/SHACL-over-PG layer. That alternative is estimated at 30–40 person-weeks, against 22–29 for this change. It would only win for edge-property-heavy OLTP workloads that need no SPARQL, OWL or SHACL.

### Relationships as asserted triples, with reifiers only when needed (D14)
`(a)-[:T]->(b)` is always the triple `a :T b`, so a hop costs one join on `posg`/`ospg`. A reifier `_:r rdf:reifies <<( a :T b )>>` is created only in three cases:
- the relationship has properties;
- a second, parallel relationship between the same nodes is created;
- the query binds the relationship to a variable that needs its identity.

Parallel relationships are several reifiers of one triple term. Deleting one relationship deletes its reifier. The asserted triple is deleted in the same batch only when no reifier remains, which one `NOT EXISTS` checks. For a relationship without a reifier, its identity for uniqueness is the tuple `(s, p, o)`. *Alternative:* every relationship as an intermediate node. That costs two joins per hop and triple write amplification.

Going from a traversed triple to its reifier needs `triple_terms(s, p, o)`. The index is optional, following D3's rule of keeping indexes few. Without it, only queries that walk relationship-first can read relationship properties.

### Generated node ids (D15)
A new inline tag, GeneratedNode (tag 8, currently unused), has a random 59-bit payload. Its IRI `urn:oxilite:n:<payload as hex>` is decoded from the id, like inline integers, so `CREATE` in a per-row update can produce ids with SQL `random()`. It needs no `terms` row and no xxh3, which D1 cannot compute. Hashing the same IRI gives a different, `Iri`-tagged id, so the encoder must map any `urn:oxilite:n:` IRI to the GeneratedNode tag. That keeps one id per term.

### Compiler IR (D16)
`compiler/mod.rs` and `compiler/ops.rs` match on `spargebra::GraphPattern` today. The refactor adds an internal `Op` (BGP, Join, LeftJoin, Filter, Union, Minus, Extend, Group, Project, Distinct, Slice, Values, Path) plus PG extensions:
- `UniqueEdges(vars)`: pairwise inequality of relationship identities;
- `VarLength { min, max, trail, seed }`: the property-path recursive CTE, extended with a depth column and an `instr`-checked visited-edge string when `trail` is set;
- `ShortestPath`: a breadth-first search step machine, one request per frontier level;
- `PathValue`: records hop ids for the decoder;
- `ListExpr` / `MapExpr`: lowered to SQLite JSON1 (`json_each`, `json_group_array`, `json_object`), which D1 provides.

`spargebra` lowers into `Op` first, and the whole SPARQL suite must stay green before any Cypher lowering lands. *Alternative:* encoding PG operations as special `SERVICE <urn:oxilite:…>` patterns in the SPARQL algebra. It was rejected as fragile, because it would give the SERVICE IRIs a second meaning.

### SHACL shapes as the PG schema (D17)
When a shapes graph is registered, the compiler reads a compiled summary: per target class and path, the datatype, `minCount`/`maxCount`, `sh:class` and `sh:in`. It uses them as follows:
- `maxCount 1` → the property is a scalar (no list aggregation);
- `minCount 1` → inner join instead of `LEFT JOIN`;
- `datatype` → a static value type for `expr.rs` pruning;
- `sh:class` → the neighbour's label is known.

Writes append guard statements to the same batch, in this form: `INSERT INTO oxilite_pg_guard(shacl_violation) SELECT 1 WHERE EXISTS (<violation over the written nodes>)`. The constraints are datatype, cardinality, class, `in` and simple `pattern`. Complete SHACL and ShEx remain M5's rudof validation. The shape summary also drives `db.schema()` and the default label and type vocabulary.

### Values and results
`CypherResults` rows hold `Value::{Null, Bool, Int, Float, String, Temporal, List, Map, Node, Relationship, Path}`. Nodes and relationships are materialised in a second batched step, which fetches `rdf:type` and property rows for all result ids. The existing term-lookup step works the same way. Values with several RDF values follow the `multi_value` policy (`list` by default, or `first` or `error`), unless SHACL says the property is scalar.

## Risks / Trade-offs

- [The IR refactor touches stable M2 code] → Land it alone, gated on W3C and differential suites, before Cypher.
- [Trail variable-length paths grow CTE path strings] → A default depth cap, and an `explain()` warning for unbounded patterns.
- [Reified relationships multiply D1 writes] → Create reifiers only when needed. The `triple_terms` index is optional.
- [TCK null, list and coercion edge cases] → Allow-list them like D11 and D12, with justification.
- [Multi-valued RDF properties from non-Cypher writers] → The `multi_value` policy, and SHACL `maxCount` where shapes exist.

## Migration Plan

This change is purely additive. The new index and the `oxilite_pg_guard` table (a separate table, because SQLite cannot add columns idempotently) use `IF NOT EXISTS`, and `schema_version` stays unchanged. Existing SPARQL behaviour is unchanged, which the W3C suites verify.
