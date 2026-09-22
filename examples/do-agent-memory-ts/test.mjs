// End-to-end check of the agent-memory Worker on a local Durable Object (Miniflare, SQLite
// storage): bundle like wrangler, then remember, query across hops, revise and explain.
import assert from "node:assert";
import { mkdirSync } from "node:fs";
import { build } from "esbuild";
import { Miniflare } from "miniflare";

mkdirSync("build", { recursive: true });
await build({
  entryPoints: ["src/index.ts"],
  bundle: true,
  format: "esm",
  outfile: "build/index.js",
  loader: { ".wasm": "copy" },
  external: ["cloudflare:workers"],
  conditions: ["workerd", "worker", "browser"],
  logLevel: "warning",
});
const mf = new Miniflare({
  modules: true,
  scriptPath: "build/index.js",
  modulesRules: [
    { type: "ESModule", include: ["**/*.js"] },
    { type: "CompiledWasm", include: ["**/*.wasm"] },
  ],
  durableObjects: { MEMORY: { className: "GraphMemory", useSQLite: true } },
  compatibilityDate: "2026-08-01",
});
const call = (path, body) => mf.dispatchFetch(`http://localhost${path}`, { method: "POST", body });
const select = async (agent, q) => {
  const r = await call(`/agents/${agent}/sparql`, q);
  assert.strictEqual(r.status, 200, await r.clone().text());
  return (await r.json()).results.bindings;
};
const PREFIX = "PREFIX ex: <http://example.com/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> ";

let r = await call(
  "/agents/ada/remember?source=urn:chat:1",
  `@prefix ex: <http://example.com/> .
   ex:maria ex:worksAt ex:acme ; ex:prefers ex:email .
   ex:acme ex:partOf ex:globex .
   ex:globex ex:headquarteredIn ex:berlin .`,
);
assert.strictEqual(r.status, 204, await r.text());
r = await call("/agents/ada/remember?source=urn:crm:export", "@prefix ex: <http://example.com/> . ex:berlin ex:timezone \"Europe/Berlin\" .");
assert.strictEqual(r.status, 204, await r.text());

// Multi-hop: which time zone should the meeting with Maria use? Four hops across two sources.
let rows = await select(
  "ada",
  PREFIX +
    `SELECT ?tz WHERE {
       GRAPH ?g1 { ex:maria ex:worksAt ?org }
       GRAPH ?g2 { ?org ex:partOf*/ex:headquarteredIn ?city }
       GRAPH ?g3 { ?city ex:timezone ?tz }
     }`,
);
assert.deepStrictEqual(rows.map((b) => b.tz?.value), ["Europe/Berlin"]);

// Provenance: where did each fact about Maria come from?
rows = await select("ada", PREFIX + "SELECT ?p ?g WHERE { GRAPH ?g { ex:maria ?p ?o } } ORDER BY ?p");
assert.deepStrictEqual(rows.map((b) => b.g.value), ["urn:chat:1", "urn:chat:1"]);

// Belief revision in one transaction: Maria now prefers chat.
r = await call("/agents/ada/update", PREFIX + "DELETE { GRAPH ?g { ex:maria ex:prefers ?old } } INSERT { GRAPH <urn:chat:2> { ex:maria ex:prefers ex:chat } } WHERE { GRAPH ?g { ex:maria ex:prefers ?old } }");
assert.strictEqual(r.status, 204, await r.text());
rows = await select("ada", PREFIX + "SELECT ?c ?g WHERE { GRAPH ?g { ex:maria ex:prefers ?c } }");
assert.deepStrictEqual(rows.map((b) => [b.c.value, b.g.value]), [["http://example.com/chat", "urn:chat:2"]]);

// Isolation: another agent has its own graph.
rows = await select("bob", "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }");
assert.strictEqual(rows.length, 0);

// Reasoning: with a small ontology, an Employee is also found as a Person.
r = await call("/agents/ada/remember?source=urn:ontology", "@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . ex:Employee rdfs:subClassOf ex:Person .");
assert.strictEqual(r.status, 204, await r.text());
r = await call("/agents/ada/remember?source=urn:chat:3", "@prefix ex: <http://example.com/> . ex:maria a ex:Employee .");
assert.strictEqual(r.status, 204, await r.text());
const people = PREFIX + "SELECT ?x WHERE { GRAPH ?g { ?x a ex:Person } }";
assert.strictEqual((await select("ada", people)).length, 0);
r = await call("/agents/ada/sparql?reasoning=rdfs", people);
assert.strictEqual(r.status, 200, await r.clone().text());
assert.deepStrictEqual((await r.json()).results.bindings.map((b) => b.x.value), ["http://example.com/maria"]);

// recall is read-only: an update sent to /sparql is rejected.
r = await call("/agents/ada/sparql", PREFIX + "INSERT DATA { ex:x ex:y ex:z }");
assert.strictEqual(r.status, 400);

r = await mf.dispatchFetch("http://localhost/agents/ada/explain?query=" + encodeURIComponent("SELECT * WHERE { ?s ?p ?o }"));
assert((await r.text()).startsWith("-- oxilite: fully compiled"));
console.log("Durable Object agent memory: ok");
await mf.dispose();
