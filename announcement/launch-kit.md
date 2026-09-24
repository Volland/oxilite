# oxilite launch kit

Everything needed for the 0.2.2 announcement, in one place. Written for whoever presses the buttons on launch day.

Canonical article: <https://oxilitedb.com/articles/introducing-oxilite>
Substack draft: [`substack-introducing-oxilite.md`](substack-introducing-oxilite.md)

---

## The message

**One sentence.** oxilite is an Oxigraph-compatible RDF database and SPARQL 1.1 engine that stores everything in plain SQLite, so a knowledge graph now runs on Cloudflare D1, in a Durable Object, in a mobile app, or in the same file as the rest of your application.

**Three proof points, in this order.**

1. **It is Oxigraph, not Oxigraph-inspired.** Same data model, same semantics, same Rust API — and Oxigraph's own W3C and store tests run against it, with every divergence allow-listed and published.
2. **It works where RocksDB cannot.** One SQL statement per query, read-free writes, three covering indexes, one atomic batch per write. That is a design for a database billed per row over a network, not a port that happens to compile.
3. **It holds Verifiable Credentials properly.** The exact issued JSON byte for byte, plus its claims as a named graph you can query — the two things a wallet actually needs, without pretending either one is the other.

**What to avoid claiming.** It is not faster than Oxigraph overall (it loads slower and the file is bigger; it wins on the aggregate-heavy mix). It does not verify credential proofs. Federation is out of scope. Prebuilt Node binaries are darwin-arm64 only.

---

## Sequencing

Publish in this order, same day, roughly an hour apart. Everything points at the canonical article, never at a copy.

| # | Channel | Notes |
|---|---|---|
| 1 | Website article | Already live at `/articles/introducing-oxilite` once `site/` is pushed to `main`. Verify it before anything links to it. |
| 2 | GitHub release | Tag the version, use the release note below, link the article. |
| 3 | Substack | Paste the draft. Set the canonical URL to the website article if the plan supports it. |
| 4 | Hacker News (Show HN) | Post the article URL, then the comment below as the first comment. Weekday morning US Eastern. Do not ask anyone to upvote. |
| 5 | X / Mastodon / Bluesky | The thread below. |
| 6 | LinkedIn | The post below. |
| 7 | Reddit: r/rust, r/semanticweb | Two different posts, not the same text twice. r/rust leads with the compiler; r/semanticweb leads with RDF and credentials. |
| 8 | Cloudflare Developers Discord, `#d1` | The short note below. Lead with the D1 design constraints, not the product. |

Niche places worth a message once the above is out: the Oxigraph repository discussions (as a courtesy, since it builds on their crates), the rudof project, the W3C Credentials Community Group mailing list, and any RDF/SPARQL newsletter you read.

---

## Pre-flight checklist

Do not announce until every line is ticked.

- [ ] `cargo test --workspace` green; `lat check` clean.
- [ ] `site/` pushed to `main` and the Pages workflow succeeded; open <https://oxilitedb.com/articles/introducing-oxilite> in a private window.
- [ ] Every link in the new article resolves (see the link check in the "Verification" section below).
- [ ] The article renders on a phone — the SVG figure and the wide tables are the risky parts.
- [ ] crates.io shows 0.2.2 for every published crate; npm shows the matching `@oxilite/*` versions.
- [ ] `npm install @oxilite/node` works in a clean directory on a machine that is not yours.
- [ ] A fresh `cargo add oxilite` + the README's first Rust example compiles and runs.
- [ ] The D1 quickstart works end to end against a *real* D1, not only Miniflare.
- [ ] README badges and the `homepage` field point at oxilitedb.com.
- [ ] GitHub repository description, topics and social preview image are set.
- [ ] Issue templates exist — the first hour after a Show HN is when people file things.
- [ ] Decide who is answering comments, and block two hours for it.

---

## GitHub release note

> **oxilite 0.2.2 — a knowledge graph in plain SQLite**
>
> oxilite is an Oxigraph-compatible RDF database and SPARQL 1.1 engine whose only storage engine is SQLite. It runs anywhere SQLite runs, including Cloudflare D1 and Durable Objects.
>
> Milestones M1–M7 are implemented:
>
> - **Storage and query.** Hash-id term encoding, three covering `WITHOUT ROWID` indexes, and a compiler that turns a whole SPARQL query — joins, `OPTIONAL`, `UNION`, `MINUS`, aggregates, property paths, `ORDER BY` — into a single `SELECT`. 100% of the W3C SPARQL 1.1 query suite passes; 95% of evaluations run fully in SQL.
> - **Atomic updates on D1.** One request is one transaction is one `batch()`. `DELETE/INSERT … WHERE` stages its results inside the same batch, so SPARQL's evaluate-then-apply semantics hold without interactive transactions.
> - **Reasoning.** RDFS and OWL-QL by query rewriting against a materialized TBox closure — no extra rows written, answers always fresh. OWL 2 RL materialization is opt-in and runs on every backend.
> - **Validation.** SHACL and ShEx through rudof's engines, unchanged. Results over an oxilite store are identical to rudof's over its in-memory graph on the W3C SHACL core and shexTest suites.
> - **openCypher.** The same quads as a property graph, OWL- and SHACL-aware. 3733 of 3880 TCK scenarios pass.
> - **JSON-LD and Verifiable Credentials.** Documents stored byte for byte under their `id`, their RDF in a named graph of the same IRI, metadata indexed, contexts resolved offline. 450 W3C `toRdf` tests pass.
>
> Compatibility with Oxigraph is tested, not assumed: Oxigraph's own W3C runner, store tests and JS tests run against oxilite, and every divergence is allow-listed in `COMPATIBILITY.md`.
>
> Read the full overview: <https://oxilitedb.com/articles/introducing-oxilite>
>
> ```
> cargo add oxilite
> npm install @oxilite/node
> npm install @oxilite/d1
> ```
>
> MIT or Apache-2.0.

---

## Show HN

**Title** (80 characters is the hard limit; this is 74):

> Show HN: Oxilite – an Oxigraph-compatible RDF and SPARQL engine on SQLite

**URL:** `https://oxilitedb.com/articles/introducing-oxilite`

**First comment** (post immediately after submitting):

> Author here. I wanted a knowledge graph in the places where I actually deploy things — a Cloudflare Worker, a Durable Object, a phone, an app that already has a SQLite file — and the good RDF engines all want RocksDB and a filesystem.
>
> So oxilite keeps Oxigraph's data model, semantics and Rust API, and implements them with nothing but portable SQL on a standard SQLite. No extensions, no required user-defined functions.
>
> The two decisions that made it work on a remote, per-row-billed database:
>
> Every term is a tagged 64-bit xxh3 hash computed in Rust, so a write never reads anything back and a query constant is a compile-time integer literal. Canonical integers and booleans are inlined in the id itself, so `FILTER(?age > 30)` is arithmetic on the id and a range becomes an index range scan.
>
> A whole query compiles to one `SELECT`. Joins, OPTIONAL, UNION, MINUS, subqueries, aggregates, property paths as recursive CTEs. On a local SQLite that's just fast; on D1 it's the difference between one round-trip and one per triple pattern per row. When something genuinely can't be expressed, the largest compilable subtrees still run as SQL and only the rest falls back to Oxigraph's `spareval`.
>
> Some things I didn't expect going in:
>
> - SQLite's planner can't order an N-way self-join of one table usefully — it has no way to tell `rdf:type` from a rare predicate. oxilite orders patterns itself from per-predicate statistics and pins the order with `CROSS JOIN`. On one benchmark that's 75 µs vs 7.2 ms.
> - A single `+` character matters: without SQLite's unary-plus "don't index this" hint on graph conditions, it picks the graph index for `g = 0`, which nearly every quad matches. Star queries were two orders of magnitude slower.
> - Generated SQL has to parse on builds with `YYSTACKDEPTH=100` (stock macOS and Ubuntu), so expression depth is a correctness constraint. Triple-term equality and ordering are precomputed at write time because the expression would otherwise be too deep.
>
> Honest comparison with Oxigraph on BSBM: Oxigraph loads 16× faster and the file is half the size, and it's ahead on the lookup-heavy mix. oxilite is faster on the aggregate-heavy business-intelligence mix. The trade is load speed and disk for running anywhere SQLite runs.
>
> Compatibility is tested rather than claimed — Oxigraph's own W3C runner, its store tests and its JS tests run against oxilite, and each divergence is allow-listed with the decision that caused it.
>
> There's also openCypher over the same quads (96% of the TCK), RDFS/OWL-QL reasoning by query rewriting rather than materialization, SHACL/ShEx through rudof, and a JSON-LD + Verifiable Credentials layer that keeps the issued JSON byte for byte while putting its claims in a named graph you can query.
>
> MIT or Apache-2.0. Happy to answer anything, and genuinely interested in benchmarks on data that isn't mine.

**If someone asks "why not just use Oxigraph?"**
> You should, if you can run RocksDB. oxilite exists for the cases where you can't: D1, Durable Objects, a host that ships its own SQLite with extensions disabled, or an app that wants the graph in a file it already backs up. It reuses Oxigraph's parsers, algebra, result formats and fallback evaluator, so it's a different storage and execution strategy, not a competing ecosystem.

**If someone asks "is SQLite-as-a-triple-store new?"**
> Not at all — it's a well-trodden idea. What's new here is the design for a *remote* SQLite: read-free writes from hash ids, one statement per query, three covering indexes because each one is a billed write, explicitly refreshed statistics, and one atomic batch per write because `batch()` is all D1 gives you.

**If someone says the benchmarks are unfair or thin.**
> Fair. It's BSBM with the official tools on one laptop, and the D1 row is local Miniflare behind an HTTP sidecar, so it measures the code path rather than Cloudflare's latency. `bench/bsbm.sh` reproduces the whole table — if you run it on other data I'd like to see the numbers, including the bad ones.

---

## X / Mastodon / Bluesky thread

**1/**
> Oxigraph is a great RDF database. It needs RocksDB, so it can't run on Cloudflare D1, in a Durable Object, or in an app that ships its own SQLite.
>
> oxilite keeps Oxigraph's data model, semantics and Rust API — with plain SQLite as the only storage engine.
>
> oxilitedb.com/articles/introducing-oxilite

**2/**
> Every RDF term becomes a tagged 64-bit xxh3 hash computed in Rust.
>
> A write never reads anything back. A query constant is a compile-time integer literal. Canonical integers live *inside* the id, so FILTER(?age > 30) is arithmetic and a range is an index range scan.

**3/**
> A whole SPARQL query compiles to ONE SELECT. Joins, OPTIONAL, UNION, MINUS, subqueries, aggregates, property paths as recursive CTEs.
>
> Locally that's fast. On D1 it's the difference between one round-trip and one per triple pattern, per row.

**4/**
> SQLite's planner can't order an N-way self-join of one table — it has no way to tell rdf:type from a rare predicate.
>
> So oxilite orders the patterns itself from per-predicate statistics and pins the order with CROSS JOIN.
>
> 75 µs vs 7.2 ms on one benchmark query.

**5/**
> Reasoning without writing a single row: RDFS and OWL-QL by rewriting queries against a small materialized TBox closure.
>
> Queries stay single statements. Answers are always fresh. On a billed-per-write database, that matters.
>
> Full OWL 2 RL materialization is there when you ask for it.

**6/**
> SHACL and ShEx aren't reimplemented — rudof's engines run against the store unchanged.
>
> On the W3C SHACL core suite and shexTest, results over an oxilite store are *identical* to rudof's over its in-memory graph.

**7/**
> The same quads are also a property graph. openCypher, 96% of the TCK.
>
> A relationship is an asserted triple, so a hop is one join. Properties and parallel relationships live on RDF 1.2 reifiers, created only when needed.
>
> Write with Cypher, read with SPARQL. Same data.

**8/**
> And Verifiable Credentials, which is the part I care most about.
>
> The issued JSON, byte for byte, under its id — because a copy rebuilt from a database is not the document that was signed.
>
> Its claims as a named graph of the same IRI, so "which credential says this?" is GRAPH ?cred { … }.

**9/**
> Proofs land in their own graphs, so claim queries never trip over a proofValue.
>
> Issuer, subject, type and validity are indexed columns.
>
> W3C contexts are bundled — nothing hits the network, on D1 either.
>
> oxilite does NOT verify proofs. Verify the stored JSON, then act on it.

**10/**
> Honest numbers: on BSBM, Oxigraph loads 16× faster, the file is half the size, and it wins the lookup-heavy mix. oxilite wins the aggregate-heavy one.
>
> The trade is load speed and disk for running anywhere SQLite runs.
>
> MIT/Apache-2.0 · github.com/Volland/oxilite

---

## LinkedIn

> **A knowledge graph that runs where your code runs.**
>
> I have spent a long time on RDF systems, and the same wall kept coming up: the good engines need a native storage engine and a local filesystem, and a growing share of software no longer has either. Serverless edge runtimes, mobile apps, sandboxed hosts that ship their own SQLite with extensions disabled.
>
> So I built oxilite. It keeps Oxigraph's data model, SPARQL semantics and Rust API, and implements them with nothing but portable SQL on a standard SQLite. The same graph runs on a laptop, inside an application file, and on Cloudflare D1 at the edge.
>
> What that took, in short:
>
> • Terms are 64-bit hashes computed in Rust, so writes never read anything back and query constants are compile-time integers.
> • A whole SPARQL query compiles into a single SELECT — the difference between one network round-trip and one per triple pattern when your database is remote.
> • Every write is one atomic batch, because on D1 a batch is the only atomic unit there is.
> • Reasoning happens by rewriting queries against a small schema closure instead of materializing inferences, so entailment costs no writes and answers are never stale.
>
> On top of that: SHACL and ShEx validation through rudof's engines unchanged, openCypher over the same data, and a JSON-LD layer that stores a Verifiable Credential exactly as it was issued while making its claims queryable as a named graph — the two things a wallet genuinely needs.
>
> Compatibility with Oxigraph is tested rather than asserted: Oxigraph's own W3C test suites and API tests run against oxilite, and every deliberate divergence is documented.
>
> Full write-up, including where it is slower than Oxigraph and what it deliberately does not do: https://oxilitedb.com/articles/introducing-oxilite
>
> MIT or Apache-2.0. github.com/Volland/oxilite
>
> #RDF #SPARQL #SQLite #KnowledgeGraph #VerifiableCredentials #Rust #Cloudflare

---

## Reddit

**r/rust** — title: *oxilite: compiling SPARQL to a single SQLite SELECT, so an RDF database runs on Cloudflare D1*

> Oxigraph is the Rust RDF database most people reach for, and it stores data in RocksDB. I needed a graph in places where RocksDB can't go, so oxilite reimplements the storage and execution on plain SQLite while reusing Oxigraph's crates for everything else — `oxrdf`, `oxttl`/`oxrdfio`, `spargebra`, `sparesults`, `oxsdatatypes`, and `spareval` as a fallback evaluator.
>
> The parts that might interest this crowd:
>
> - **Sans-IO core.** `oxilite-core` never performs I/O. Every operation yields SQL `Request`s and consumes `Response`s, and multi-round-trip operations are step machines. That's why the same compiler serves rusqlite, a `dlopen`'ed libsqlite3, a Rust Worker on D1, and a wasm build behind a TypeScript driver — each backend is a few dozen lines.
> - **Term encoding.** Tagged 64-bit xxh3 hashes computed in Rust: writes never read back, query constants are compile-time integer literals, canonical integers and booleans are inlined in the id. Collisions are caught by a `BEFORE INSERT` trigger rather than assumed away.
> - **The compiler.** A whole SPARQL algebra tree becomes one `SELECT` whenever possible. The interesting constraints turned out to be SQLite's fixed parser stack (`YYSTACKDEPTH=100` on stock macOS/Ubuntu) and the fact that SQL has no common subexpressions, so expression size grows when an operand is used repeatedly. There are four specific rules keeping it linear.
> - **The planner.** SQLite can't order an N-way self-join of one table. Greedy statistics-driven ordering, pinned with `CROSS JOIN`. 75 µs vs 7.2 ms on one query.
> - **Partial fallback.** When something won't compile, the largest compilable subtrees are rewritten into `SERVICE <urn:oxilite:sql:N>` calls answered with precompiled SQL, and only the rest runs in `spareval`.
>
> BSBM numbers against Oxigraph (it loads 16× faster and wins the lookup mix; oxilite wins the aggregate mix), the full design rationale, and the honest limits are in the write-up: https://oxilitedb.com/articles/introducing-oxilite
>
> MIT/Apache-2.0, happy to talk about any of it.

**r/semanticweb** — title: *oxilite: an Oxigraph-compatible RDF store on SQLite, with JSON-LD and Verifiable Credentials kept byte for byte*

> oxilite is an RDF 1.2 store and SPARQL 1.1 engine that uses SQLite as its only storage engine, so it runs on Cloudflare D1, in a Durable Object, on a phone, or in an application's existing database file.
>
> For this subreddit the interesting parts are probably these:
>
> **Reasoning without materialization.** RDFS and OWL-QL entailment happens by rewriting queries against a small TBox closure table computed with recursive CTEs. Each pattern's quad table is swapped for a derived table of entailed triples, so queries stay single SQL statements, property paths and OPTIONAL see the same entailments, and no rows are written. OWL 2 RL materialization exists as an explicit opt-in and agrees with `reasonable` on sample ontologies.
>
> **Validation by reuse.** rudof's SHACL and ShEx engines run unchanged over the store through its RDF traits. On the W3C SHACL core suite and shexTest, the reports are identical to rudof's over an in-memory graph.
>
> **JSON-LD and Verifiable Credentials.** A document is stored byte for byte under its `@id`, and its RDF goes into the named graph of that same IRI. So "which credential asserts this claim?" is `GRAPH ?cred { … }`, and you can go straight back to the original JSON for verification. Proofs land in their own graphs (JSON-LD declares `proof` as a graph container), so claim queries never mix with signatures. Blank nodes are labelled deterministically per document, so a repeated put is idempotent and two credentials never share an anonymous node. Contexts resolve entirely offline. 450 of the W3C `toRdf` tests pass. Proofs are not verified — that stays with a signature library.
>
> **Property graphs as an RDF view.** openCypher over the same quads: labels are `rdf:type`, relationships are asserted triples, relationship properties live on RDF 1.2 reifiers. Because it's RDF underneath, OWL reasoning and SHACL shapes apply to Cypher queries too.
>
> Write-up with the design reasoning and the limits: https://oxilitedb.com/articles/introducing-oxilite

---

## Cloudflare Developers Discord (`#d1`)

> Built an RDF/SPARQL database that targets D1 specifically, and the constraints shaped the whole design — sharing in case the approach is useful to anyone else storing complex data there.
>
> No interactive transactions, so every write is exactly one `batch()`. `DELETE/INSERT … WHERE` stages its read results into a table inside the same batch to keep the evaluate-then-apply semantics.
> Billed per row written, so three covering indexes rather than nine (4.81 rows written per triple by default, 3.81 without the graph index), statistics refreshed explicitly rather than per write, and no read-back on insert — ids are hashes computed client-side.
> Round-trips dominate, so a whole query compiles to one SELECT rather than one scan per pattern.
> Statement limits are in the compiler's capability record: SQL under 90 KB, ≤50 statements per batch, ≤5 terms per compound SELECT (larger UNIONs nest), GLOB patterns under 50 bytes, ids selected as TEXT because JS numbers lose precision above 2^53.
>
> Works from a Rust Worker or from TypeScript via a wasm core, and the same package runs on a Durable Object's SQLite through a small `ctx.storage.sql` adapter.
>
> https://oxilitedb.com/articles/introducing-oxilite

---

## Verification

Run before announcing:

```bash
# every local link in the new article resolves to a file that exists
grep -o 'href="[^"#]*"' site/articles/introducing-oxilite.html \
  | sed 's/href="//;s/"//' | grep -v '^https\?:' | sort -u \
  | while read -r l; do
      case "$l" in ./) t=site/articles/index.html ;; ../) t=site/index.html ;;
                   ../*) t="site/${l#../}" ;; *) t="site/articles/$l" ;; esac
      [ -e "$t" ] || echo "MISSING: $l -> $t"
    done

# external links still answer
grep -o 'https://[^"<)]*' site/articles/introducing-oxilite.html | sort -u \
  | xargs -n1 -P8 curl -sS -o /dev/null -w '%{http_code} %{url_effective}\n'
```

After the Pages deploy:

```bash
curl -sI https://oxilitedb.com/articles/introducing-oxilite | head -1   # expect 200
curl -s  https://oxilitedb.com/sitemap.xml | grep introducing-oxilite   # expect the new URL
```

---

## Answering the hard questions

Keep these answers short and do not get defensive. Concede the true parts.

**"This is just a triple store on SQLite, people have done that for twenty years."**
Correct, and the README says so. The contribution is the design for a *remote*, per-row-billed SQLite: read-free writes, one statement per query, few indexes, explicit statistics, one atomic batch per write — plus Oxigraph API compatibility that is tested against Oxigraph's own suites.

**"RDF is dead / nobody uses SPARQL."**
Don't argue the premise. Point at what's shipping on it: Verifiable Credentials are JSON-LD, which is RDF, and wallets need both the signed bytes and queryable claims. Agent memory needs provenance and multi-hop recall. Both are in the article with runnable examples.

**"Why not Durable Objects SQLite instead of D1?"**
Both work — the same `@oxilite/d1` package runs on a Durable Object's SQLite through a small adapter, and that's the per-agent-graph story. D1 was the harder target (remote, batch-only, billed per row), so designing for it made the DO case fall out.

**"Your benchmark is one laptop and local Miniflare."**
Yes, and it's stated in the article. `bench/bsbm.sh` reproduces it. Numbers from other people's data are welcome, including unflattering ones.

**"Does it verify credentials?"**
No, deliberately. It runs a structural check with `ssi-vc` and stores the bytes. Verify the stored JSON with a signature library, then act on it. Claiming otherwise would be the worst possible bug in this domain.

**"What's missing?"**
Federation (`SERVICE`), garbage collection of the term dictionary, prebuilt Node binaries beyond darwin-arm64, credential status-list revocation, and JSON-LD framing on read. On D1 specifically, anything needing the Rust fallback evaluator or over 90 KB of SQL returns `unsupported` rather than running slowly.

---

## The week after

- Watch GitHub issues and the HN thread for the first 48 hours; a fast, specific answer converts more people than the post did.
- Collect every "can it do X?" into a list. That list is the roadmap for the next release, and it is better than a roadmap you wrote alone.
- Ship prebuilt Node binaries for linux-x64 and darwin-x64 if anyone asks twice — it is the most likely first friction point.
- Write the follow-up while the questions are fresh. The two obvious candidates: the term-encoding and planner internals for a Rust audience, and a wallet built end to end on `@oxilite/d1` for the credentials audience.
