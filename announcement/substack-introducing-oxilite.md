# Introducing oxilite: a knowledge graph in plain SQLite

*Oxigraph's data model, semantics and Rust API, with SQLite as the only storage engine — so the same graph runs on your laptop, inside your mobile app, and on Cloudflare D1 at the edge. This is the long tour: how it works inside, what it can do, and where its limits are.*

> Version 0.2.2 · Milestones M1–M7 implemented · [github.com/Volland/oxilite](https://github.com/Volland/oxilite) · MIT or Apache-2.0 · [oxilitedb.com](https://oxilitedb.com)

---

## The gap

[Oxigraph](https://github.com/oxigraph/oxigraph) is an excellent Rust RDF database and SPARQL engine. It stores data in RocksDB, which needs native code, a local filesystem and a storage engine you can link against. That rules it out in exactly the places where a growing share of software now runs:

- **Serverless SQLite at the edge.** Cloudflare D1 hands you a managed SQLite you can send SQL to. You cannot install extensions, load native libraries or run RocksDB, and every call is a network round-trip billed per row read and written.
- **Hosts that ship their own SQLite.** Mobile apps, embedded devices, sandboxes and platforms where the SQLite library is provided for you and extensions are disabled.
- **Apps that already use SQLite** and want a knowledge graph in the same file, backed up, shipped and replicated with the same tools they already have.

oxilite closes that gap. It keeps the Oxigraph experience — the same data model, the same SPARQL semantics, the same Rust API — and implements it with **nothing but portable SQL on a standard SQLite**. No extensions. No custom functions (they are used when they happen to exist, and never required). No filesystem access beyond what the host SQLite already provides.

In one line: `use oxigraph::store::Store;` becomes `use oxilite::store::Store;`, and your graph now runs wherever SQLite runs.

## What you get

| Capability | State |
|---|---|
| RDF 1.2 data model, all Oxigraph parsers and serializers | Turtle, TriG, N-Triples, N-Quads, RDF/XML, JSON-LD; W3C syntax suites pass |
| SPARQL 1.1 Query, compiled to SQL | 100% of the W3C query suite; 95% of evaluations run fully in one SQL statement |
| SPARQL 1.1 Update, atomic | W3C update suite passes on every backend, D1 included |
| RDFS and OWL-QL reasoning, OWL 2 RL materialization | Per-query rewriting, no extra writes; materialization agrees with `reasonable` |
| SHACL and ShEx validation | rudof's engines unchanged; W3C SHACL core and shexTest results identical to rudof in memory |
| openCypher over the same data | 96.2% of the openCypher TCK |
| JSON-LD documents and Verifiable Credentials | Byte-for-byte storage, a named graph each; 450 W3C `toRdf` tests pass |
| Full-text search | FTS5, natively and on D1 |
| SPARQL 1.1 Protocol endpoint and CLI | `oxilite serve`, on the routes of `oxigraph serve` |

## The shape of the thing

The centre of oxilite is a core that **never touches a database**. Every operation is a pure function from RDF or SPARQL input to SQL statements, and from result rows back to RDF terms. A backend is only ever asked: *run these statements, atomically if I say so.*

```
             SPARQL · Cypher · RDF · Store API
                            │
            ┌───────────────▼────────────────┐
            │  oxilite-core   ·   sans-IO    │
            │  algebra → planner → SQL       │
            │  Response → decoder → results  │
            └───────────────┬────────────────┘
                            │  Request { statements, Read | Atomic }
     ┌──────────────┬───────┴───────┬────────────────┐
     ▼              ▼               ▼                ▼
  rusqlite       dylib          D1 · Rust        D1 · wasm
 bundled     your libsqlite3,   a Worker on    TypeScript driver
 SQLite      loaded at runtime  worker::D1     on env.DB
```

Operations that need several round-trips — a query, then a lookup of the terms in its results — are step machines: `step(response)` returns either the next request or the finished output.

That single decision is what makes the edge case work. A backend that answers over the network, in batches, with no ability to run your code, is just another implementation of *run these statements*. The compiler adapts to it through a `Capabilities` record: maximum SQL length, maximum statements per request, whether user-defined functions exist, whether interactive transactions exist, whether 64-bit integers have to travel as text.

## Terms are integers you can compute

Every RDF term becomes a tagged, positive 64-bit integer: one sign bit, four tag bits, fifty-nine bits of payload. The tags cover IRIs, blank nodes, simple strings, language strings, directional language strings, other typed literals and RDF 1.2 triple terms. The payload is an xxh3 hash of a canonical key.

Two things follow, and they are the reason oxilite is viable on a remote database:

- **Writes never read anything back.** The same term hashes to the same id in every process, on every machine, forever. Inserting is `INSERT OR IGNORE` and nothing else — no dictionary lookup, no round-trip, no sequence.
- **Query constants become SQL integer literals at compile time.** `?s a ex:Person` compiles to `q.p = 2305843…` before a single row is touched.

Canonical `xsd:integer` values (up to ±2⁵⁸) and booleans are *inlined*: they live inside the id itself and need no dictionary row at all. Integer ids carry `value + 2^58`, so they sort by numeric value, and `FILTER(?age > 30)` becomes arithmetic on the id — or, when a range is bound, an index range scan. Non-canonical lexical forms such as `"012"^^xsd:integer` are hashed instead, so term identity stays exact.

Hashes can collide. At 190 000 terms the probability is around 10⁻⁹, and oxilite does not rely on luck: a `BEFORE INSERT` trigger on the dictionary aborts the whole batch if an id ever maps to a different term.

## Seven small tables

The schema is deliberately plain. Any SQLite tool can open it, and any SQLite backup is a graph backup.

```sql
CREATE TABLE quads (s INTEGER NOT NULL, p INTEGER NOT NULL, o INTEGER NOT NULL,
                    g INTEGER NOT NULL DEFAULT 0,              -- 0 = the default graph
                    PRIMARY KEY (s, p, o, g)) WITHOUT ROWID, STRICT;
CREATE INDEX quads_posg ON quads(p, o, s, g);
CREATE INDEX quads_ospg ON quads(o, s, p, g);
CREATE INDEX quads_gspo ON quads(g, s, p, o);                  -- optional

CREATE TABLE terms (id INTEGER PRIMARY KEY, lex TEXT NOT NULL, dt TEXT, lang TEXT,
                    dir INTEGER, num REAL, nt INTEGER, ts REAL) STRICT;
-- + triple_terms (RDF 1.2), graphs, stats_pred, stats_class, update_buffer, oxilite_meta
```

Three points are worth dwelling on.

**Three covering permutations, not nine.** `quads` is a `WITHOUT ROWID` clustered index on `spog`; the two secondary indexes contain every column, so *every* triple-pattern scan is index-only — SQLite never has to fetch a row. Oxigraph keeps nine index orders; oxilite keeps three (plus an optional graph index) because on D1 each extra index is a billed write on every insert. Measured on a local D1, the default schema writes 4.81 rows per triple, 3.81 without the graph index.

**Typed side columns.** The dictionary carries `num` (numeric value), `nt` (numeric type rank) and `ts` (epoch seconds), with partial indexes. Numeric and date comparisons never re-parse a lexical form at query time.

**Facts that SQL would need deep expressions for are precomputed at write time.** Triple-term value equality and sort order, whether a date carries a timezone — all computed in Rust when the term is stored. This is not only speed: generated SQL has to parse on SQLite builds with a fixed parser stack (`YYSTACKDEPTH=100`, as on stock macOS and Ubuntu), so expression depth is a correctness constraint, not a preference.

## One SQL statement per query

This is the heart of it. A SPARQL query — joins, `OPTIONAL`, `UNION`, `MINUS`, subqueries, aggregates, property paths, `ORDER BY`, `LIMIT` — compiles into a *single* `SELECT` whenever it possibly can.

On a local SQLite that is merely fast. On D1 it is the difference between a working database and a toy: an evaluator that scans one triple pattern at a time pays a network round-trip per pattern, per row. oxilite pays one or two round-trips for the whole query.

Here is the compiler's own output for a three-pattern star with a filter:

```sparql
SELECT ?name WHERE {
  ?p a ex:Person ; ex:age ?age ; ex:name ?name .
  FILTER(?age > 30)
}
```

```sql
SELECT q2.o AS v2
FROM quads q1 CROSS JOIN quads q0 CROSS JOIN quads q2
WHERE q1.p = 2305843… AND +q1.g = 0
  AND (CASE (q1.o >> 59) WHEN 6 THEN (q1.o & 576460752303423487) - 288230376151711744
       WHEN 5 THEN (SELECT num FROM terms WHERE id = q1.o AND nt IS NOT NULL) END) > 30
  AND q0.s = q1.s AND q0.p = 2305843… AND q0.o = 2305843… AND +q0.g = 0
  AND q2.s = q1.s AND q2.p = 2305843… AND +q2.g = 0
```

Read it closely and the design shows through. `ex:age` is scanned *first*, not `rdf:type`, because statistics say it is more selective. The filter is applied immediately after that scan, before any join. Inline integers are compared arithmetically — the `CASE` arm for tag 6 unpacks the value straight out of the id — and only non-inline numbers such as decimals and doubles consult the dictionary. The unary `+` on graph conditions is SQLite's "do not use an index for this" convention: without it SQLite happily picks the graph index for `g = 0`, which nearly every quad matches. That one character made star queries two orders of magnitude faster.

### How the operators map

| SPARQL | SQL |
|---|---|
| Basic graph pattern | self-joins on `quads`, order forced with `CROSS JOIN` |
| `OPTIONAL` | `LEFT JOIN`, filter in the `ON` clause, parenthesised to keep index use |
| `UNION` | `UNION ALL` of sealed branches, missing variables padded with NULL |
| `MINUS` | `NOT EXISTS` with compatibility and domain-overlap conditions |
| `FILTER EXISTS` | correlated `EXISTS` with outer bindings substituted |
| `VALUES` / `BIND` | a `VALUES` table / a computed column |
| `GRAPH` | conditions on `g`; merged datasets deduplicate with `NOT EXISTS` on a lower `g` |
| `path*`, `path+`, `path?` | recursive CTEs, seeded from a constant endpoint when one exists |

Property paths deserve a note. When a closure is joined with a pattern that binds one end — `?c a ex:C . ?c ex:knows+ ?x` — the recursive CTE is seeded from that pattern's distinct values rather than computing the transitive closure of the whole graph. That is the classic magic-set trick, and `explain()` flags the cases where it could not be applied.

SPARQL's three-valued logic falls out for free: a SPARQL value is a bundle of SQL fragments (kind, lexical form, datatype, language, number, timestamp, boolean), and SQL `NULL` *is* the SPARQL error. Functions are tiered — SQL built-ins first, then rewrites (a simple `REGEX` becomes `instr`/`substr`), then native UDFs where the backend has them.

### When SQL cannot express it

Some queries do not compile. A custom function inside a `FILTER`, or SQL that would exceed the backend's statement limit, raises `Unsupported`. On native backends oxilite does not simply give up and fall back to a slow evaluator for the whole query. It rewrites the query so that each **largest compilable subtree** becomes a `SERVICE <urn:oxilite:sql:N>` call, hands it to Oxigraph's own `spareval`, and answers those service calls with precompiled SQL. Only the operators genuinely above SQL's reach run in Rust.

On D1 there is no synchronous access, so there is no fallback: the query reports `unsupported` rather than silently becoming slow. `explain()` tells you why and lists which subqueries would still have run as SQL. Using `explain()` in development to confirm your queries compile fully is the single best habit for a D1 deployment.

## The planner

SQLite's query planner sees an N-way self-join of one table and has only per-index averages to work with. It cannot tell `rdf:type` — which matches a third of the database — from a predicate that matches four triples. So oxilite orders the patterns itself, greedily: most selective first, then repeatedly the cheapest pattern *connected* to an already-bound variable, which avoids accidental Cartesian products. `CROSS JOIN` pins the order; SQLite still chooses the index for each pattern, which it is good at.

The right-hand side of an `OPTIONAL` is planned as if the left side were already bound, because SQLite evaluates it once per left row. That keeps `OPTIONAL` on a foreign key linear rather than quadratic.

Statistics are per-predicate triple counts, distinct subject and object counts, and per-class instance counts, plus a skew table: for predicates with at most 1024 distinct objects, predicate–object pairs at least four times more frequent than that predicate's average, the 2000 most frequent kept. A constant object then gets its real count instead of an average.

They are refreshed **explicitly**, by `optimize()` or after a bulk load — never on every write. On D1 maintaining them per write would double the write bill and create a hot row, and stale statistics only ever degrade a plan, never a result.

On 350 010 quads, on a laptop, in-process:

| query | oxilite planner | SQLite's planner |
|---|---|---|
| star with a rare badge | 75 µs | 7.2 ms |
| friends of badged people | 96 µs | 87 µs |
| city + age range filter | 478 µs | 7.8 ms |

## Atomic means one batch

D1 has no interactive transactions. `batch()` is the only atomic unit it offers. So oxilite adopts a single universal rule: **one request is one transaction is one D1 batch**, and every write compiles to self-reading SQL that fits inside it.

`DELETE/INSERT … WHERE` is the interesting case, because SPARQL requires the `WHERE` clause to be evaluated against the state *before* any modification. oxilite evaluates it once into a staging table, computing the rows to delete and the rows to insert, then deletes, then inserts, then clears the staging table — all inside the same batch. The semantics hold without ever holding a transaction open across a round-trip.

Conditions that SQL cannot express as data become constraint failures instead. A guard table has `CHECK` columns named after the SPARQL condition they protect — `graph_does_not_exist`, `graph_already_exists`, `computed_value_not_storable` — so `CREATE GRAPH` on an existing graph inserts a non-`NULL` value, fails the check, aborts the batch, and is mapped back to the right SPARQL error. `explain_update()` shows the SQL of every operation.

## Reasoning without extra writes

Reasoning is chosen per query: `None` (the default, as in Oxigraph), `Rdfs`, or `OwlQl`.

```rust
// ex:Dog rdfs:subClassOf ex:Animal . ex:rex a ex:Dog .
let opts = QueryOptions { reasoning: Reasoning::Rdfs, ..Default::default() };
store.query_output("SELECT ?x WHERE { ?x a <http://ex/Animal> }", &opts)?;   // ex:rex
```

The mechanism is *query rewriting against a materialized TBox closure*, not data materialization. A small table holds the class closure, the RDFS and OWL property closures (subproperty, equivalence, inverses and symmetry composed with a direction bit), the transitive properties, and the classes a property's subjects and objects belong to via domains and ranges. It is computed from the asserted quads with recursive CTEs, and refreshed automatically — in the same atomic request — by any write that touches a schema triple.

The compiler then swaps each pattern's `quads` table for a derived table of entailed triples: a `SELECT DISTINCT` over `UNION ALL` arms for asserted triples, sub- and inverse properties, types through the class closure, and domains and ranges. Queries stay single statements, property paths and `OPTIONAL` and even the fallback evaluator all see the same entailments, and **no rows are written**. On a billed-per-write database that matters a great deal, and the answers are always fresh.

Full OWL 2 RL is available when you want it, explicitly. `materialize()` runs the rule set as `INSERT OR IGNORE … SELECT` statements, one atomic request per round, until a round adds nothing — the same code on bundled SQLite, on a loaded library and on D1. Natively, `materialize_with_reasonable()` computes the same closure in memory with [reasonable](https://github.com/gtfierro/reasonable), and an agreement test checks the two against each other on sample ontologies. Materialized inferences are not maintained; re-run it when the data changes.

## Validation: rudof's engines, unchanged

SHACL and ShEx validation is not reimplemented. `oxilite-validate` implements [rudof](https://github.com/rudof-project/rudof)'s RDF traits over the store, so rudof's own SHACL (native and SPARQL engines) and ShEx validators run against oxilite directly. Neighbourhood lookups become SQL pattern scans, and rudof's SPARQL-mode validation runs through the oxilite compiler. Because rudof already builds on the Oxigraph 0.5 crate family, there are no type conversions in between.

```rust
let report = validate_shacl(&store, shapes_ttl, &ShaclValidationMode::Native)?;
let results = validate_shex(&store, shexc, "http://example.com/",
                            "<http://example.com/alice>@<http://example.com/Person>")?;
```

On the W3C SHACL core suite (226 reports per backend) and the shexTest validation suite (1161 validations), the results over an oxilite store are *exactly* rudof's results over its in-memory graph.

rudof does not build its validators for wasm32, so D1 is validated from native code — a CLI, a server, CI — over the D1 HTTP API. A prefetch pulls in only what the shapes need: targets, the class hierarchy, and a breadth-first neighbourhood to a configured depth, one request per hop and chunk. Past a configured limit it fails loudly with the size it found, rather than truncating and reporting a conformance it cannot justify.

## The same data as a property graph

oxilite also speaks **openCypher**, over the very same quads. Nodes are IRIs or blank nodes, labels are `rdf:type`, node properties are literal triples, and a relationship is an asserted triple — so following `-[:KNOWS]->` is one join on a covering index. Relationship properties and parallel relationships live on RDF 1.2 reifiers, created only when actually needed.

```js
store.cypher("CREATE (:Person {name: 'Ada'})-[:KNOWS {since: 2020}]->(:Person {name: 'Alan'})");
// SPARQL sees the same data:
// ASK { ?a ex:KNOWS ?b . ?r rdf:reifies <<( ?a ex:KNOWS ?b )>> ; ex:since 2020 }
```

Cypher reads are lowered to SPARQL algebra and go through the same compiler, planner and reasoning rewrites; what SQL cannot express — writes, lists, maps, `collect()`, temporal arithmetic — runs in Rust over the returned rows. A writing statement is one read followed by one atomic write. `shortestPath` is a breadth-first search doing one SQL request per level for all sources at once, so it works on D1 too.

Because it is all RDF underneath, OWL and SHACL apply: with reasoning on, labels match subclasses and relationship types match their subproperties and inverses; SHACL shapes stored in the dataset act as the property graph's schema, and a write that breaks `sh:datatype`, cardinality, `sh:in` or `sh:pattern` is rejected before anything is sent. 3733 of the 3880 openCypher TCK scenarios pass. There is [a whole walkthrough](https://oxilitedb.com/articles/property-graphs-cypher) on this.

## JSON-LD and Verifiable Credentials

The newest part of oxilite, and the one that most changes what it is *for*.

A Verifiable Credential is a JSON-LD document, and the people who hold one need it in two incompatible ways at once. They need it **as JSON, exactly as issued**, because they will present it, forward it, archive it and verify its signature — and a copy rebuilt from a database is not the document that was signed. They also need it **as data**: which degrees are still valid, who issued them, which credential asserts this particular claim.

oxilite keeps both, and keeps them honest about which is which.

### The raw document is the source of truth

The JSON is stored byte for byte in a keyed table. Its RDF is *derived data*, written into a named graph of its own. The bytes come back unchanged; re-serializing from RDF would lose member order, formatting and everything JSON-LD drops on the way in.

Derived data can drift — SPARQL UPDATE is allowed to edit those graphs, and sometimes that is exactly what you want. Rather than put triggers on the quad table (a write cost on every single quad, billed on D1), oxilite gives you two explicit operations: `check()` reconverts every document and reports the difference, and `rebuild()` regenerates a graph from the stored bytes.

```js
store.update(`DELETE WHERE { GRAPH <${id}> { ?d <https://schema.org/name> ?n } }`);
vcs.documents.check();       // [{ key: id, missing: 1, extra: 0 }]
vcs.documents.rebuild(id);   // true, and check() is [] again
```

### One graph per credential

By default a document is keyed by its `@id`/`id`, and its triples go into the named graph of that same IRI. That single choice pays for itself repeatedly:

- **Provenance is free.** "Which credential says this?" is just `GRAPH ?cred { … }`, and `documentForGraph(?cred)` hands you back the original JSON.
- **Replace and remove need no read.** Clearing a graph clears exactly that credential's triples, so a put is one atomic batch with no round-trip first.
- **Proofs stay out of the way.** JSON-LD declares `proof` as a graph container, so each proof lands in its own graph, owned by the credential and linked with `sec:proof`. Your claim queries never trip over a `proofValue`, and removing the credential removes its proofs.

```js
const vcs = store.credentials();
const id = vcs.put(degreeJson);        // checked, stored, converted — one atomic write

store.query(`SELECT ?cred ?name WHERE {
  GRAPH ?cred { ?s <https://www.w3.org/ns/credentials/examples#degree> ?d .
                ?d <https://schema.org/name> ?name }
}`);
// → the credential IRI, and "Bachelor of Science and Arts"
```

Keys and graphs are configurable: the key can be a JSON Pointer such as `/credentialSubject/id`, a content hash, or an explicit value; the graph can be an IRI template, one fixed graph, or the default graph. VCDM 2.0 makes `id` optional, so credentials without one get `urn:oxilite:doc:sha256:<hex>` — which also means storing the same credential twice keeps a single copy.

Blank nodes are labelled from a hash of the document's key and their position, never from a global counter. Two credentials therefore never share an anonymous node, and re-storing a document produces identical labels, so a repeated put is genuinely idempotent.

### Metadata you can look up without SPARQL

Issuer, subject, types and the validity window are copied into indexed columns as the document is stored, so the everyday questions are one SQL statement with keyset paging:

```js
vcs.find({ issuer: "did:example:academy", validAt: new Date() });
vcs.find({ subject: "did:example:ebfeb1f712ebc6f1c276e12ec21", type: "ExampleDegreeCredential" });
```

`validAt` reads `validFrom`/`validUntil` from VCDM 2.0 credentials and `issuanceDate`/`expirationDate` from 1.1 ones. Which indexes exist is your choice, because on D1 every index entry is a billed write: `store.credentials({ indexes: { subject: false } })`.

A presentation is stored as itself, and each credential it embeds is *also* stored as a credential in its own right, in the same atomic write, with the keys recorded in the presentation's `refs`. You can find, query and remove an embedded credential separately.

### Offline by construction

JSON-LD normally wants the network: contexts are IRIs, and expansion fetches them. Inside a Worker that is unacceptable, and in a wallet it is a privacy leak. So oxilite's context loader never performs I/O inside the JSON-LD processor. It answers from memory — contexts registered on the handle, then the W3C credential, security and DID contexts bundled by `ssi-json-ld` — and records what it could not find. The job reads those misses from a `jsonld_contexts` table and retries, in rounds, since a context may import others.

```js
const docs = store.jsonld();
docs.putContext("https://example.org/contexts/recipe.jsonld",
  { "@context": { "@vocab": "https://schema.org/", ingredients: "recipeIngredient" } });
docs.put(`{"@context": "https://example.org/contexts/recipe.jsonld",
           "@id": "https://example.org/recipes/pancakes", "@type": "Recipe",
           "name": "Pancakes", "ingredients": ["flour", "milk", "eggs"]}`);
```

An unknown context fails with a `JsonLdError` carrying the JSON-LD error code, and *nothing is written*. Downloading is opt-in, native-only, and `cacheFetched` persists what it fetched so the document works on D1 afterwards. The conversion itself uses the [json-ld](https://crates.io/crates/json-ld) crate and passes 450 tests of the W3C JSON-LD 1.1 `toRdf` suite.

### What it is not

Storing a credential says **nothing** about whether it is valid. oxilite runs a structural check with `ssi-vc` — context order, required types, issuer, subject — and stores the bytes. It does not verify proofs. Verify the stored JSON with a library such as [ssi](https://github.com/spruceid/ssi), then act on it. Status-list revocation checks, framing or compaction on read, and enveloped JOSE/COSE credentials inside presentations are all out of scope today.

Two walkthroughs go deeper: [storing credentials](https://oxilitedb.com/articles/verifiable-credentials-jsonld) and [querying them](https://oxilitedb.com/articles/querying-jsonld-credentials), both asserted end to end by runnable examples in the repository.

## Where it runs

Four backends, one API. Native Rust on a bundled SQLite; a `libsqlite3` you point at by path at runtime (only the stable C API is bound, so any SQLite ≥ 3.37 works); a Rust Worker on D1; and the core compiled to WebAssembly behind a TypeScript driver, so a Workers project needs no Rust toolchain at all. The same `@oxilite/d1` package also runs on a Durable Object's embedded SQLite through a small adapter, which gives every agent or user a private graph — [there is a guide](https://oxilitedb.com/articles/agent-memory-durable-objects) for that.

```rust
use oxilite::store::Store;             // was: use oxigraph::store::Store;

let store = Store::open("data.sqlite")?;
store.load_from_reader(RdfFormat::Turtle, TURTLE.as_bytes())?;
store.update("INSERT DATA { <http://ex/a> <http://ex/p> 42 }")?;
store.optimize()?;                      // refresh statistics (replaces RocksDB compact)
```

```ts
import { D1Store } from "@oxilite/d1";
import wasm from "@oxilite/d1/oxilite.wasm";

const store = await D1Store.open(env.DB, { wasm, migrated: true });
await store.update(sparqlUpdate);        // one atomic D1 batch
await store.queryJson(sparqlQuery);
```

There is a CLI too — `oxilite load`, `query`, `explain`, and `oxilite serve`, which exposes the SPARQL 1.1 Protocol on the same routes as `oxigraph serve`, against a file, a library you name, or a D1 database behind a local sidecar.

## What it costs

Measured with the Berlin SPARQL Benchmark and its official tools against Oxigraph 0.5.11 on RocksDB, one laptop, 1000 products (374 911 triples). QMpH is query mixes per hour, higher is better.

| engine | load (s) | size (MB) | explore QMpH | business intelligence QMpH |
|---|---|---|---|---|
| oxigraph (RocksDB) | 0.30 | 43.9 | 489 260 | 520 |
| oxilite (bundled SQLite) | 4.80 | 85.4 | 335 015 | 899 |
| oxilite (system SQLite) | 2.85 | 85.6 | 348 838 | 1233 |
| oxilite (local D1) | 16.55 | – | 7982 | 841 |

Read this honestly. Oxigraph loads sixteen times faster and produces a file half the size — oxilite pays for three covering indexes over integer ids in one SQLite file. On the lookup-heavy explore mix Oxigraph is ahead. On the aggregate-heavy business-intelligence mix oxilite is *faster*, because those queries become one SQL statement that SQLite executes well. The D1 row runs against a local Miniflare D1 behind an HTTP sidecar, so each query pays about 55 ms of round trips; it measures the D1 code path, not Cloudflare's production latency. Every query returned the same results on every engine.

That is the trade: load speed and disk for the ability to run anywhere SQLite runs.

## "Behaves like Oxigraph" is tested, not claimed

The compatibility harness runs Oxigraph's *own* tests against oxilite:

- Oxigraph's W3C manifest runner, ported, over `rdf-tests` and `oxigraph-tests`. Every test gets three verdicts: oxilite vs. expected, Oxigraph vs. expected, and oxilite vs. Oxigraph.
- Oxigraph's store API tests (`lib/oxigraph/tests/store.rs`) against `oxilite::blocking::Store`, changing only the import.
- Oxigraph's JavaScript tests (`js/test/store.test.ts`) against `@oxilite/node`: 32 of 33, with the one difference allow-listed.
- A differential corpus run on both engines — with statistics on and off, with our planner and with SQLite's — whose results must match.

Divergences are allow-listed individually, each linked to the decision that caused it, and a `COMPATIBILITY.md` is generated from the run. The deliberate ones: `backup` is `VACUUM INTO` and `compact` is `optimize()`; ORDER BY over incomparable literals uses a total SQL order where Oxigraph's comparator is non-transitive; a triple present in several merged default graphs matches once, as the RDF merge requires, where Oxigraph may return duplicates; decimals are compared as IEEE doubles, though the lexical form is always preserved exactly; and on D1, queries needing the fallback evaluator report `unsupported` instead of running.

## Honest limits

- **Loading is slower than Oxigraph**, and the file is larger. If you need fast bulk ingest into a local process and nothing else, RocksDB wins.
- **On D1, some queries will not run.** Anything needing the Rust fallback, and anything whose SQL exceeds 90 KB, reports `unsupported`. `explain()` is how you find out before your users do.
- **Terms are never garbage-collected on delete**, as in Oxigraph. The dictionary only grows.
- **`SERVICE` (federation) is out of scope** for v1.
- **A Cypher writing statement on D1 is not isolated** between its read and its batch — the batch itself is atomic, but another writer may act in between.
- **Prebuilt Node binaries are darwin-arm64 only** up to 0.2.2; other platforms build the addon from source.

## Start here

```bash
cargo add oxilite                      # Rust: rusqlite (default), dylib, d1, cypher, jsonld, vc, reasonable
cargo install oxilite-cli              # the `oxilite` command and a SPARQL endpoint
npm install @oxilite/node              # Node.js
npm install @oxilite/d1                # Cloudflare D1 and Durable Objects (WebAssembly)
```

Milestones M1 through M7 are implemented: storage core, full SPARQL 1.1 query, atomic update and D1, reasoning, validation, performance work, openCypher — plus the JSON-LD and Verifiable Credentials layer. The design reasoning lives in the repository as a knowledge graph of its own, in `lat.md/`, and every milestone has a specification under `openspec/changes/`.

---

**The takeaway.** oxilite is a bet that the interesting constraint of the next decade is not how fast a triple store can go on one machine, but *where* it is allowed to run. A knowledge graph that is a single SQLite file — queryable with SPARQL and Cypher, reasoned over with OWL, validated with SHACL, and able to hold a Verifiable Credential exactly as it was signed — can live in a Worker, in a phone, in a Durable Object, or in the same file as the rest of your application.

It is MIT or Apache-2.0, like Oxigraph. Issues, benchmarks on your own data, and disagreements are all welcome at [github.com/Volland/oxilite](https://github.com/Volland/oxilite).
