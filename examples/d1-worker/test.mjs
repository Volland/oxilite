// End-to-end check of the Rust Worker on a local D1 (Miniflare): migration, load, query, update.
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { Miniflare } from "miniflare";

const mf = new Miniflare({
  modules: true,
  scriptPath: "build/index.js",
  modulesRules: [
    { type: "ESModule", include: ["**/*.js", "**/*.mjs"] },
    { type: "CompiledWasm", include: ["**/*.wasm"] },
  ],
  d1Databases: ["DB"],
  compatibilityDate: "2026-08-01",
});
const db = await mf.getD1Database("DB");
for (const stmt of readFileSync("migrations/0001_oxilite.sql", "utf8")
  .split("\n")
  .filter((l) => !l.startsWith("--"))
  .join("\n")
  .split(";\n")
  .filter((s) => s.trim())) {
  await db.prepare(stmt).run();
}
const call = (path, init) => mf.dispatchFetch(`http://localhost${path}`, init);

let r = await call("/load?format=text/turtle", {
  method: "POST",
  body: "@prefix ex: <http://example.com/> . ex:alice ex:knows ex:bob ; ex:age 42 . ex:bob ex:knows ex:carol .",
});
assert.strictEqual(r.status, 204, await r.text());
r = await call("/sparql?query=" + encodeURIComponent("SELECT ?x WHERE { <http://example.com/alice> <http://example.com/knows>+ ?x } ORDER BY ?x"));
const json = await r.json();
assert.deepStrictEqual(json.results.bindings.map((b) => b.x.value), ["http://example.com/bob", "http://example.com/carol"]);
r = await call("/update", { method: "POST", body: "PREFIX ex: <http://example.com/> DELETE { ?s ex:age ?a } INSERT { ?s ex:age ?b } WHERE { ?s ex:age ?a BIND(?a + 1 AS ?b) }" });
assert.strictEqual(r.status, 204, await r.text());
r = await call("/sparql", { method: "POST", body: "ASK { <http://example.com/alice> <http://example.com/age> 43 }" });
assert.strictEqual((await r.json()).boolean, true);
r = await call("/explain?query=" + encodeURIComponent("SELECT * WHERE { ?s ?p ?o }"));
assert((await r.text()).startsWith("-- oxilite: fully compiled"));
console.log("Rust D1 worker: ok");
await mf.dispose();
