# Vectors that know where they are: oxilite on Turso

*Vector search finds what sounds like your question. A knowledge graph knows what is actually connected to it. AI agents need both, and today they usually get them from two databases glued together in application code. oxilite now runs on Turso, SQLite rewritten in Rust, and puts vector indexes inside the knowledge graph. One store, one SQL statement, and SPARQL, Cypher and Datalog can all use them.*

> oxilite · `oxilite-turso` · Turso 0.8 · [github.com/Volland/oxilite](https://github.com/Volland/oxilite) · MIT or Apache-2.0

---

## Two halves of a memory

Ask an agent what it remembers about next week's trip, and two different kinds of lookup are needed.

The first is fuzzy. The agent has hundreds of stored memories, and it wants the ones that *mean* something close to "travel plans", whatever words they used. That is what embeddings and vector search are for.

The second is exact. Once it has found "Alice is flying to Lisbon for the ACME offsite", the agent needs to know who else works at ACME, what else it knows about those people, and which of those facts it has already used. None of that is similarity. It is structure, and structure is what a graph is for.

Most agent stacks split these two halves across two systems: a vector database for recall and a graph or relational database for facts. Every question that needs both becomes application code. It fetches the top 20 vectors, collects their ids, sends them to the other store, joins in memory, and filters again. That is slow, it is easy to get wrong, and it throws away the thing that makes a graph useful: the query planner can no longer see the whole question.

oxilite is an RDF knowledge graph that compiles SPARQL, openCypher and Datalog into SQL. As of this release it runs on Turso, and Turso has vectors. So the two halves of a memory can now live in one store and be asked about in one query.

## Why Turso

[Turso](https://github.com/tursodatabase/turso) is a rewrite of SQLite in Rust. It speaks SQLite's SQL and reads SQLite's file format. It runs in-process, needs no C toolchain, and adds things SQLite never had, including native vector types:

- `vector32`, `vector64`, `vector8` (quantized to bytes), `vector1bit` (one bit per dimension) and `vector32_sparse`
- distances: `vector_distance_cos`, `vector_distance_l2`, `vector_distance_dot` and `vector_distance_jaccard`
- index methods, including an inverted-file index for sparse vectors

oxilite was built for exactly this kind of engine swap. Its core never talks to a database. Every query, update and load is a small state machine that produces SQL and consumes rows, and a backend only has to "run these statements, atomically if asked". The same compiler already drives the bundled SQLite, a system `libsqlite3`, and Cloudflare D1 over the network. Turso is the fifth backend, in a new crate called `oxilite-turso`.

Before anything else was designed, we ran the real oxilite store against Turso to see what breaks. Most things passed first time: joins, filters, OPTIONAL, UNION, MINUS, aggregates, updates, savepoints. Three things needed work:

- **Recursive CTEs.** The stable Turso 0.7.2 has none, and oxilite needs them for property paths, schema-registry scoping and recursive Datalog. Turso 0.8.0-pre.14 has them, so the crate pins that exact version.
- **`WITHOUT ROWID` tables.** Turso cannot index them and rejects `WITHOUT ROWID, STRICT`. The backend rewrites these tables as ordinary `STRICT` tables whose composite primary key becomes a unique index. Every index oxilite relies on still builds; the cost is one extra B-tree per table.
- **FTS5.** Turso doesn't have it, so oxilite's optional full-text index is refused with a clear error instead of a confusing one.

Everything else is the same oxilite: the same Rust API, the same SPARQL semantics, the same Cypher and Datalog.

```toml
[dependencies]
# The `turso` feature ships in the next release; until then, depend on the repository.
oxilite = { git = "https://github.com/Volland/oxilite", features = ["turso", "cypher", "datalog"] }
```

```rust
let store = oxilite::store::Store::open_turso("memory.db")?;   // or Store::new_turso() in memory
```

Or from the command line, where every command takes `--turso`:

```bash
oxilite --turso memory.db
```

## A vector index is a triple

In oxilite an embedding is just a literal, stored like any other property value:

```turtle
@prefix ex: <http://example.com/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:alice a ex:Person ; ex:name "Alice" ; ex:worksFor ex:acme .
ex:bob   a ex:Person ; ex:name "Bob"   ; ex:worksFor ex:acme .
ex:acme  a ex:Company ; ex:name "ACME" .

ex:m1 a ex:Memory ; ex:text "Alice prefers aisle seats on long flights" ;
  ex:about ex:alice ; ex:at "2026-09-01"^^xsd:date ; ex:embedding "[0.9, 0.1, 0.0, 0.1]" .
ex:m2 a ex:Memory ; ex:text "Alice is flying to Lisbon for the ACME offsite" ;
  ex:about ex:alice , ex:acme ; ex:at "2026-09-20"^^xsd:date ; ex:embedding "[0.8, 0.3, 0.1, 0.0]" .
ex:m3 a ex:Memory ; ex:text "Bob is allergic to peanuts" ;
  ex:about ex:bob ; ex:at "2026-08-11"^^xsd:date ; ex:embedding "[0.1, 0.0, 0.9, 0.2]" .
ex:m4 a ex:Memory ; ex:text "Bob also travels to the ACME offsite" ;
  ex:about ex:bob , ex:acme ; ex:at "2026-09-21"^^xsd:date ; ex:embedding "[0.7, 0.2, 0.2, 0.1]" .
```

(Real embeddings have hundreds of dimensions; four keep the example readable.)

A vector index says which property holds embeddings, how many dimensions they have, and how to measure distance. Following oxilite's rule that metadata about the data belongs in the data, the index definition is itself RDF, stored in a system graph called `<oxilite:vectors>`:

```turtle
<oxilite:vector/memories> a oxl:VectorIndex ;
    oxl:indexName "memories" ;
    oxl:property ex:embedding ;
    oxl:dimensions 4 ;
    oxl:metric oxl:Cosine ;           # or oxl:Euclidean, oxl:DotProduct, oxl:Jaccard
    oxl:elementType oxl:Float32 ;     # or Float64, Int8, Bit1, SparseFloat32
    oxl:class ex:Memory .             # optional: only memories are candidates
```

Because the definition is data, you can query it, it's in your dumps and backups, and it's versioned with the store. Writing it is enough to create the index. You can do that in whichever way suits you:

```sparql
# SPARQL Update: insert the description, and the index is built
PREFIX oxl: <https://oxilite.dev/ns#>
PREFIX ex:  <http://example.com/>
INSERT DATA { GRAPH <oxilite:vectors> {
  <oxilite:vector/memories> a oxl:VectorIndex ; oxl:indexName "memories" ;
    oxl:property ex:embedding ; oxl:dimensions 4 ; oxl:metric oxl:Cosine ; oxl:class ex:Memory .
} }
```

```cypher
// Cypher, in Neo4j 5 syntax
CREATE VECTOR INDEX memories IF NOT EXISTS FOR (m:Memory) ON (m.embedding)
OPTIONS { indexConfig: { `vector.dimensions`: 4, `vector.similarity_function`: 'cosine' } }
```

```rust
// Rust
store.create_vector_index(
    &VectorIndex::new("memories", ex("embedding"), 4)
        .metric(Metric::Cosine)
        .class(ex("Memory")),
)?;
```

```text
oxilite> .vector create memories ex:embedding 4 cosine --class ex:Memory
Created vector index memories · 9.19 ms
```

Behind the definition, oxilite builds a table of embeddings. Triggers on the quad table keep it up to date inside the same transaction as every write: `INSERT DATA`, `DELETE … WHERE`, `CLEAR GRAPH`, the bulk loader and Cypher `CREATE`/`SET` alike. An index can't drift out of date, and there's no re-indexing job to schedule. An embedding that isn't a JSON array of the right length makes the write fail, with the index name and the expected dimensions in the error. A vector index that silently skips bad rows returns wrong answers without anyone noticing, so it fails loudly instead.

`SHOW VECTOR INDEXES` in Cypher, `.vector list` in the shell and `store.vector_indexes()` in Rust all read back the same state:

```text
┌──────────┬──────────────┬──────┬────────┬─────────┬───────────┬──────┬───────┐
│ name     │ property     │ dims │ metric │ element │ class     │ rows │ built │
├──────────┼──────────────┼──────┼────────┼─────────┼───────────┼──────┼───────┤
│ memories │ ex:embedding │ 4    │ cosine │ float32 │ ex:Memory │ 4    │ yes   │
└──────────┴──────────────┴──────┴────────┴─────────┴───────────┴──────┴───────┘
```

## Searching from three languages

The agent embeds its question ("what do I know about upcoming travel?") and gets `[0.85, 0.2, 0.05, 0.05]`. On its own, a nearest-neighbour search returns the closest memories:

```text
oxilite> .vector search memories [0.85, 0.2, 0.05, 0.05] 3
┌──────┬───────┬──────────┬──────────┐
│ rank │ node  │ distance │ score    │
├──────┼───────┼──────────┼──────────┤
│ 1    │ ex:m1 │ 0.010197 │ 0.994902 │
│ 2    │ ex:m2 │ 0.011444 │ 0.994278 │
│ 3    │ ex:m4 │ 0.025773 │ 0.987114 │
└──────┴───────┴──────────┴──────────┘
```

The interesting part is what happens *around* the search. Each query language has its own syntax for it, and all three compile to the same nearest-neighbour SQL, placed inside the single statement that the rest of the query becomes.

**SPARQL** names the index as a `SERVICE`, the same way oxilite already names past versions of the store:

```sparql
PREFIX oxl: <https://oxilite.dev/ns#>
PREFIX ex:  <http://example.com/>
SELECT ?text ?score (GROUP_CONCAT(?who; separator=", ") AS ?people) WHERE {
  SERVICE <oxilite:vector/memories> {
    [] oxl:query "[0.85, 0.2, 0.05, 0.05]" ; oxl:k 3 ; oxl:node ?m ; oxl:score ?score .
  }
  ?m ex:text ?text ; ex:about ?p .
  ?p a ex:Person ; ex:name ?who .
}
GROUP BY ?text ?score ORDER BY DESC(?score)
```

```text
┌────────────────────────────────────────────────┬────────────────────┬────────┐
│ text                                           │ score              │ people │
├────────────────────────────────────────────────┼────────────────────┼────────┤
│ Alice prefers aisle seats on long flights      │ 0.9949015844315562 │ Alice  │
│ Alice is flying to Lisbon for the ACME offsite │ 0.9942779229024825 │ Alice  │
│ Bob also travels to the ACME offsite           │ 0.9871135271723207 │ Bob    │
└────────────────────────────────────────────────┴────────────────────┴────────┘
3 rows · 4.93 ms
```

`oxl:distance` gives the raw distance, `oxl:score` a similarity (higher is nearer), and `oxl:query` can also be a node, meaning "find things like this one".

**Cypher** uses Neo4j's procedure, so existing queries port unchanged. The difference from a plain procedure call is that `node` is a real node, and the pattern can keep going from it:

```cypher
CALL db.index.vector.queryNodes('memories', 3, $question) YIELD node, score
MATCH (node)-[:about]->(p:Person)-[:worksFor]->(c:Company)<-[:worksFor]-(colleague:Person)
WHERE colleague <> p
RETURN node.text AS memory, colleague.name AS alsoAffected, round(score * 1000) / 1000 AS score
ORDER BY score DESC
```

```text
memory                                           alsoAffected  score
Alice prefers aisle seats on long flights        Bob           0.995
Alice is flying to Lisbon for the ACME offsite   Bob           0.994
Bob also travels to the ACME offsite             Alice         0.987
```

**Datalog** gets a built-in relation, `nearest(index, query, k, ?node, ?rank)`, which rules can use like any other fact. This is where the graph starts doing work that similarity can't:

```prolog
@prefix ex: <http://example.com/> .

recalled(?m, ?r)   :- nearest("memories", "[0.85, 0.2, 0.05, 0.05]", 3, ?m, ?r).
involved(?p)       :- recalled(?m, _), ex:about(?m, ?p), ex:Person(?p).
colleague(?p, ?c)  :- involved(?p), ex:worksFor(?p, ?co), ex:worksFor(?c, ?co), ?p != ?c.
unrecalled(?c, ?t) :- colleague(_, ?c), ex:about(?m, ?c), ex:text(?m, ?t), not recalled(?m, _).

?- unrecalled(?c, ?t).
```

```text
?c = ex:bob   ?t = "Bob is allergic to peanuts"
```

Read that answer carefully, because it's the argument for this whole feature. The question was about travel. Vector search found three travel memories, which is all it can do. The allergy has nothing to do with travel in embedding space, and no similarity threshold would ever have recalled it. But Bob is going to the same offsite, and an agent booking the team dinner needs to know. The graph found it by following *who* the recalled memories were about, not what they sounded like, and negation (`not recalled`) kept only the facts the agent hadn't already seen.

(Datalog returns a rank rather than a distance: in oxilite's Datalog every value is a stored term, and a rank is an integer term while a computed distance isn't. SPARQL and Cypher give you the distance.)

## Why this matters for AI agents

Graph-plus-vector retrieval is sometimes sold as "GraphRAG" and treated as a pipeline problem. Having both in one store turns it into a query problem. Here is what an agent gains from that.

**Recall that expands along real relationships.** Vector search is the entry point: it finds *where* in the graph to start. The graph then walks outward to the people, projects and decisions those memories are about. The allergy example is the small version of this. In a real agent it's "the customer's open tickets", "the other services this outage touched", or "the decision this document superseded".

**Filters the agent can trust.** "Similar memories, but only from this user", "only from the last two weeks", "only documents this agent may read" are graph conditions, and they run in the same statement as the search. Access control that runs *after* a vector search has already fetched the top k is how private data leaks into prompts. A `?doc ex:visibleTo ex:agent7` pattern next to the `SERVICE` is part of the query itself, so rows the agent may not see are never returned.

**Answers with provenance.** Every result is a node with triples attached: where it came from, when, which source document, which step derived it. An agent can explain *why* it recalled something ("this memory is about Bob, who works at ACME with Alice") instead of pointing at a cosine score.

**Deduplication and "more like this".** `oxl:query ex:m2` searches near a stored node's own embedding. That's how an agent merges near-duplicate memories, links a new entity to one it already knows, or finds the tickets resembling the one it's working on.

**Reasoning after retrieval.** Recalled nodes are ordinary graph nodes, so everything else oxilite does applies to them: OWL and RDFS entailment, SHACL validation, Datalog rules, and time travel over a versioned store ("what did the agent know last Tuesday?").

**One file, in-process, no services.** Turso is embedded. The agent's whole memory (facts, embeddings, index definitions, history) is one database file inside the agent's own process. No vector service to keep running, no network hop per recall, nothing to keep in sync.

### Bringing your own logic: host functions

Agents rarely stop at retrieval. They score, redact, check policies, normalize names, maybe call a model. This release also adds **host functions**: code in your application that all three query languages can call, while the rest of each query still compiles to SQL.

```rust
use chrono::NaiveDate;
use oxilite::functions::HostFunction;
use oxilite::model::{Literal, Term};

fn date(t: &Term) -> Option<NaiveDate> {
    match t {
        Term::Literal(l) => NaiveDate::parse_from_str(l.value(), "%Y-%m-%d").ok(),
        _ => None,
    }
}

store.register_function(
    HostFunction::new("http://example.com/fn#daysBefore", |args| {
        let days = (date(args.get(1)?)? - date(args.first()?)?).num_days();
        Some(Literal::from(days).into())
    })
    .cypher_name("fn.daysBefore")
    .arity(2, 2)
    .description("days between two xsd:date values"),
)?;
```

A function takes RDF terms and returns one, or `None` for "no value", which SPARQL treats like any other evaluation error.

Then recency is one more condition in the recall query:

```sparql
PREFIX fn: <http://example.com/fn#>
SELECT ?text ?age WHERE {
  SERVICE <oxilite:vector/memories> { [] oxl:query "[0.85, 0.2, 0.05, 0.05]" ; oxl:k 3 ; oxl:node ?m }
  ?m ex:text ?text ; ex:at ?at
  BIND(fn:daysBefore(?at, "2026-09-28"^^xsd:date) AS ?age)
  FILTER(?age < 14)
} ORDER BY ?age
```

```text
"Bob also travels to the ACME offsite"            7
"Alice is flying to Lisbon for the ACME offsite"  8
```

The same function is `WHERE fn.daysBefore(m.at, date('2026-09-28')) < 14` in Cypher, and `?age = fn:daysBefore(?at, "2026-09-28"^^xsd:date), ?age < 14` in a Datalog rule. For an agent, host functions are the natural place for a relevance scorer, a PII redactor, or a policy check that belongs in application code rather than in the database.

## What it doesn't do (yet)

- **It isn't available on Cloudflare.** Cloudflare D1 is one of oxilite's main targets, and the rest of oxilite runs there. But D1 runs Cloudflare's own SQLite build, which has no vector types or distance functions and doesn't allow extensions, and oxilite can only generate SQL the engine understands. On D1 a vector index refuses to be created and says it needs Turso. If your agent runs on Workers, you'll need Cloudflare Vectorize or another vector service next to D1, which brings back the two-system join that this release removes elsewhere. We'd like to close that gap, but it depends on the engine, not on oxilite.
- **Dense search is exact.** Turso currently has an approximate index only for sparse vectors (Jaccard over bag-of-words). Dense cosine, Euclidean and dot-product search scan the index table. That's exact and quick up to around a hundred thousand vectors, which covers a lot of agent memory, but it isn't an ANN index for a hundred million. When Turso ships a dense index, it plugs into the one place oxilite writes nearest-neighbour SQL.
- **Turso 0.8 is a prerelease.** oxilite pins `0.8.0-pre.14` exactly, because recursive queries need it. The pin moves when 0.8.0 is released.
- **No full-text index on Turso**, because FTS5 isn't available.
- **Embeddings come from you.** oxilite stores and searches vectors; your application or model computes them.
- **Rust first.** The Node and Python packages don't expose vector indexes or host functions yet.

## Try it

```bash
# Until the next release, install the command line from the repository:
cargo install --git https://github.com/Volland/oxilite oxilite-cli
oxilite --turso memory.db
```

```text
oxilite> .load memory.ttl
oxilite> .prefix ex: <http://example.com/>
oxilite> .vector create memories ex:embedding 4 cosine --class ex:Memory
oxilite> .vector search memories [0.85, 0.2, 0.05, 0.05] 3
```

Then paste the SPARQL query from above into the same shell. The design, including the decisions behind the RDF index definitions, the single nearest-neighbour statement, and how host functions reach Datalog, is written up in the repository, along with the test specifications each feature is held to.

Similarity tells an agent where to look. The graph tells it what's actually there. Now they can share a query.
