// The "Querying JSON-LD and Verifiable Credentials" walkthrough
// (site/articles/querying-jsonld-credentials.html). One wallet of credentials, an issuer
// registry written as JSON-LD, and a small ontology; then SPARQL, metadata lookups, full-text
// search, reasoning and Cypher over the same data.
// `demo` runs unchanged on @oxilite/node (sync) and @oxilite/d1 (async): every call is awaited.
// The store must be created with the text index (`textIndex: true`).
import assert from "node:assert";
import { readFileSync } from "node:fs";

const fixture = (name) =>
  readFileSync(new URL(`../../crates/oxilite-vc/tests/fixtures/${name}`, import.meta.url), "utf8");

const CONTEXT = ["https://www.w3.org/ns/credentials/v2", "https://www.w3.org/ns/credentials/examples/v2"];
const UNIVERSITY = "https://university.example/issuers/565049";
const ALICE = "did:example:alice";
const BOB = "did:example:bob";

const credential = (id, issuer, type, subject, validFrom, validUntil) =>
  JSON.stringify({
    "@context": CONTEXT,
    id,
    type: ["VerifiableCredential", type],
    issuer,
    validFrom,
    ...(validUntil && { validUntil }),
    credentialSubject: subject,
  });

export const PREFIXES = `
PREFIX cred:   <https://www.w3.org/2018/credentials#>
PREFIX ex:     <https://www.w3.org/ns/credentials/examples#>
PREFIX reg:    <https://example.org/registry#>
PREFIX schema: <https://schema.org/>
PREFIX xsd:    <http://www.w3.org/2001/XMLSchema#>
PREFIX oxl:    <https://oxilite.dev/ns#>
`;

/** Rows as plain objects of lexical values, for comparisons. */
const plain = (rows) => rows.map((r) => Object.fromEntries([...r].map(([k, v]) => [k, v.value])));

export async function fill(store) {
  const vcs = store.credentials();
  // The W3C degree credential (with a Data Integrity proof) and a presentation of two more.
  await vcs.put(fixture("degree-v2.json"));
  await vcs.putPresentation(fixture("presentation-v2.json"));
  await vcs.put(credential("urn:uuid:bob-msc", UNIVERSITY, "ExampleDegreeCredential",
    { id: BOB, degree: { type: "ExampleMasterDegree", name: "Master of Science in Graph Databases" } },
    "2021-06-30T00:00:00Z", "2041-06-30T00:00:00Z"));
  await vcs.put(credential("urn:uuid:alice-job", "did:example:acme", "ExampleEmploymentCredential",
    { id: ALICE, jobTitle: "Knowledge Engineer", employer: "did:example:acme" },
    "2020-01-01T00:00:00Z", "2024-12-31T00:00:00Z"));
  await vcs.put(credential("urn:uuid:bob-job", "did:example:acme", "ExampleEmploymentCredential",
    { id: BOB, jobTitle: "Data Engineer", employer: "did:example:acme" },
    "2022-03-01T00:00:00Z"));

  // An issuer registry: any JSON-LD document, with its own context, in its own graph.
  const docs = store.jsonld();
  await docs.putContext("https://example.org/contexts/registry.jsonld", {
    "@context": { "@vocab": "https://example.org/registry#", accreditedBy: { "@type": "@id" } },
  });
  await docs.put(JSON.stringify({
    "@context": "https://example.org/contexts/registry.jsonld",
    "@id": "https://example.org/registry",
    "@graph": [
      { "@id": UNIVERSITY, name: "Example University", accreditedBy: "https://example.org/accreditors/eqar" },
      { "@id": "did:example:academy", name: "Example Academy" },
      { "@id": "did:example:acme", name: "ACME Corp" },
    ],
  }));

  // A small ontology in the default graph: both degree types are degrees.
  await store.load(`
    @prefix ex: <https://www.w3.org/ns/credentials/examples#> .
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
    ex:ExampleBachelorDegree rdfs:subClassOf ex:Degree .
    ex:ExampleMasterDegree rdfs:subClassOf ex:Degree .`, { format: "text/turtle" });
}

export async function demo(store) {
  await fill(store);
  const vcs = store.credentials();
  const sparql = async (q, options) => plain(await store.query(PREFIXES + q, options));

  // 1. Provenance: every credential is a named graph, so GRAPH ?cred says who said it.
  assert.deepStrictEqual(await sparql(`
    SELECT ?cred ?achievement WHERE {
      GRAPH ?cred { ?holder ex:achievement ?achievement }
      FILTER(isIRI(?cred))                       # skip the presentation's embedded copies
    } ORDER BY ?achievement`), [
    { cred: "urn:uuid:vc-a", achievement: "Chess master" },
    { cred: "urn:uuid:vc-b", achievement: "Go master" },
  ]);

  // 2. Join credentials with other JSON-LD data: degrees from accredited issuers.
  assert.deepStrictEqual(await sparql(`
    SELECT ?holder ?degree ?issuerName WHERE {
      GRAPH ?cred { ?cred cred:issuer ?issuer ; cred:credentialSubject ?holder .
                    ?holder ex:degree/schema:name ?degree }
      GRAPH <https://example.org/registry> { ?issuer reg:accreditedBy ?body ; reg:name ?issuerName }
    } ORDER BY ?degree`), [
    { holder: "did:example:ebfeb1f712ebc6f1c276e12ec21", degree: "Bachelor of Science and Arts", issuerName: "Example University" },
    { holder: BOB, degree: "Master of Science in Graph Databases", issuerName: "Example University" },
  ]);

  // 3. Aggregates: credentials per issuer.
  assert.deepStrictEqual(await sparql(`
    SELECT ?issuerName (COUNT(?cred) AS ?credentials) WHERE {
      GRAPH ?cred { ?cred a cred:VerifiableCredential ; cred:issuer ?issuer }
      FILTER(isIRI(?cred))
      GRAPH <https://example.org/registry> { ?issuer reg:name ?issuerName }
    } GROUP BY ?issuerName ORDER BY DESC(?credentials) ?issuerName`), [
    { issuerName: "ACME Corp", credentials: "2" },
    { issuerName: "Example Academy", credentials: "2" },
    { issuerName: "Example University", credentials: "2" },
  ]);

  // 4. Validity windows are typed xsd:dateTime values: compare them in SPARQL.
  assert.deepStrictEqual(await sparql(`
    SELECT ?cred ?until WHERE {
      GRAPH ?cred { ?cred a ex:ExampleEmploymentCredential ; cred:validFrom ?from
                    OPTIONAL { ?cred cred:validUntil ?until } }
      FILTER(?from <= "2026-01-01T00:00:00Z"^^xsd:dateTime
             && (!BOUND(?until) || ?until > "2026-01-01T00:00:00Z"^^xsd:dateTime))
    }`), [{ cred: "urn:uuid:bob-job" }]);

  // 5. A holder's view: everything any credential says about Bob.
  assert.deepStrictEqual(await sparql(`
    SELECT ?cred ?claim ?value WHERE {
      GRAPH ?cred { <${BOB}> ?p ?o OPTIONAL { ?o schema:name ?name } }
      BIND(STRAFTER(STR(?p), "#") AS ?claim)
      BIND(COALESCE(?name, ?o) AS ?value)
    } ORDER BY ?cred ?claim`), [
    { cred: "urn:uuid:bob-job", claim: "employer", value: "did:example:acme" },
    { cred: "urn:uuid:bob-job", claim: "jobTitle", value: "Data Engineer" },
    { cred: "urn:uuid:bob-msc", claim: "degree", value: "Master of Science in Graph Databases" },
  ]);

  // 6. Reasoning: with RDFS, both degree types answer a question about ex:Degree.
  const degrees = `SELECT ?name WHERE { GRAPH ?cred { ?d a ex:Degree ; schema:name ?name } } ORDER BY ?name`;
  assert.deepStrictEqual(await sparql(degrees), []);
  assert.deepStrictEqual(await sparql(degrees, { reasoning: "rdfs" }), [
    { name: "Bachelor of Science and Arts" },
    { name: "Master of Science in Graph Databases" },
  ]);

  // 7. Full-text search over claims (FTS5).
  assert.deepStrictEqual(await sparql(`
    SELECT ?cred ?title WHERE {
      GRAPH ?cred { ?holder ex:jobTitle ?title }
      FILTER(oxl:textMatch(?title, "engineer"))
    } ORDER BY ?title`), [
    { cred: "urn:uuid:bob-job", title: "Data Engineer" },
    { cred: "urn:uuid:alice-job", title: "Knowledge Engineer" },
  ]);

  // 8. Metadata lookups without SPARQL: indexed columns, keyset paging.
  const keys = (docs) => docs.map((d) => d.key);
  assert.deepStrictEqual(keys(await vcs.find({ subject: BOB })), ["urn:uuid:bob-job", "urn:uuid:bob-msc"]);
  assert.deepStrictEqual(keys(await vcs.find({ issuer: "did:example:acme", validAt: new Date("2026-01-01") })), ["urn:uuid:bob-job"]);
  assert.deepStrictEqual(keys(await vcs.find({ type: "ExampleDegreeCredential" })), ["http://university.example/credentials/3732", "urn:uuid:bob-msc"]);
  const page1 = await vcs.find({ profile: "vc2", limit: 2 });
  const page2 = await vcs.find({ profile: "vc2", limit: 2, after: page1.at(-1).key });
  assert.deepStrictEqual(keys(page2), ["urn:uuid:bob-job", "urn:uuid:bob-msc"]);

  // 9. Back to the exact JSON of whatever a query found, e.g. to verify its proof.
  const [hit] = await store.query(PREFIXES + `SELECT ?cred WHERE { GRAPH ?cred { ?s ex:achievement "Go master" } FILTER(isIRI(?cred)) }`);
  const doc = await vcs.documents.documentForGraph(hit.get("cred"));
  assert.strictEqual(JSON.parse(doc.json).credentialSubject.achievement, "Go master");
  const degree = await vcs.get("http://university.example/credentials/3732");
  assert.strictEqual(degree.json, fixture("degree-v2.json"));

  // 10. Cypher over the same credentials: the default graph is the union of all graphs.
  const cypher = (q, params = {}) => store.cypher(q, params, {
    base: "https://www.w3.org/ns/credentials/examples#",
    prefixes: { cred: "https://www.w3.org/2018/credentials#", reg: "https://example.org/registry#", schema: "https://schema.org/" },
    useDefaultGraphAsUnion: true,
  });

  let r = await cypher(`
    MATCH (c:ExampleEmploymentCredential)-[:\`cred:credentialSubject\`]->(p),
          (c)-[:\`cred:issuer\`]->(org)
    RETURN p.jobTitle AS job, org.\`reg:name\` AS employer ORDER BY job`);
  assert.deepStrictEqual(r.records, [
    { job: "Data Engineer", employer: "ACME Corp" },
    { job: "Knowledge Engineer", employer: "ACME Corp" },
  ]);

  r = await cypher(`
    MATCH (c:\`cred:VerifiableCredential\`)-[:\`cred:issuer\`]->(i)
    WHERE i.\`reg:name\` IS NOT NULL
    RETURN i.\`reg:name\` AS issuer, count(DISTINCT c) AS credentials
    ORDER BY credentials DESC, issuer`);
  assert.deepStrictEqual(r.records.map((x) => x.issuer), ["ACME Corp", "Example Academy", "Example University"]);

  // Colleagues: two holders whose employment credentials name the same employer.
  r = await cypher(`
    MATCH (a)<-[:\`cred:credentialSubject\`]-(:ExampleEmploymentCredential)-[:\`cred:issuer\`]->(org),
          (org)<-[:\`cred:issuer\`]-(:ExampleEmploymentCredential)-[:\`cred:credentialSubject\`]->(b)
    WHERE a.jobTitle < b.jobTitle
    RETURN a.jobTitle AS a, b.jobTitle AS b, org.\`reg:name\` AS at`);
  assert.deepStrictEqual(r.records, [{ a: "Data Engineer", b: "Knowledge Engineer", at: "ACME Corp" }]);

  // Holders with their degrees, as a map per holder.
  r = await cypher(`
    MATCH (p)-[:degree]->(d)
    RETURN p.jobTitle AS job, collect(d.\`schema:name\`) AS degrees ORDER BY job`);
  assert.deepStrictEqual(r.records, [
    { job: "Data Engineer", degrees: ["Master of Science in Graph Databases"] },
    { job: null, degrees: ["Bachelor of Science and Arts"] },
  ]);

  // 11. explain: the SQL a query compiles to. One statement, no fallback.
  const plan = await store.explain(PREFIXES + `SELECT ?cred WHERE { GRAPH ?cred { ?s ex:achievement "Go master" } }`);
  assert.match(plan, /fully compiled to SQL/);
}
