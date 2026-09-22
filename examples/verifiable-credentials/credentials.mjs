// The "Verifiable Credentials in SQLite" walkthrough (site/articles/verifiable-credentials-jsonld.html).
// Works on @oxilite/node (sync) and @oxilite/d1 (async): every call is awaited.
import assert from "node:assert";
import { readFileSync } from "node:fs";

const fixture = (name) =>
  readFileSync(new URL(`../../crates/oxilite-vc/tests/fixtures/${name}`, import.meta.url), "utf8");

export const degree = fixture("degree-v2.json");
export const presentation = fixture("presentation-v2.json");

const EX = "https://www.w3.org/ns/credentials/examples#";

export async function demo(store) {
  const vcs = store.credentials();

  // 1. Store a credential: key and graph are its id.
  const id = await vcs.put(degree);
  assert.strictEqual(id, "http://university.example/credentials/3732");

  // 2. The exact bytes come back; metadata is extracted; the proof has its own graph.
  const stored = await vcs.get(id);
  assert.strictEqual(stored.json, degree);
  assert.strictEqual(stored.profile, "vc2");
  assert.strictEqual(stored.issuer, "https://university.example/issuers/565049");
  assert.strictEqual(stored.validUntil.toISOString(), "2040-01-01T00:00:00.000Z");
  const graphs = await vcs.documents.graphs(id);
  assert.deepStrictEqual(graphs.map((g) => g.termType).sort(), ["BlankNode", "NamedNode"]);
  assert.strictEqual(await store.query(`ASK { GRAPH <${id}> { ?s <https://w3id.org/security#proofValue> ?v } }`), false);

  // 3. SPARQL across credentials, one graph per credential.
  const names = await store.query(`
    SELECT ?cred ?name WHERE {
      GRAPH ?cred { ?s <${EX}degree> ?d . ?d <https://schema.org/name> ?name }
    }`);
  assert.deepStrictEqual(names.map((r) => [r.get("cred").value, r.get("name").value]), [[id, "Bachelor of Science and Arts"]]);

  // 4. Presentations: embedded credentials are stored on their own too.
  const vp = await vcs.putPresentation(presentation);
  assert.deepStrictEqual(vp.credentials, ["urn:uuid:vc-a", "urn:uuid:vc-b"]);
  const own = await store.query(`SELECT ?cred WHERE { GRAPH ?cred { ?s <${EX}achievement> "Go master" } FILTER(isIRI(?cred)) }`);
  assert.deepStrictEqual(own.map((r) => r.get("cred").value), ["urn:uuid:vc-b"]);

  // 5. Metadata lookups without SPARQL.
  const valid = await vcs.find({ issuer: "did:example:academy", validAt: new Date("2024-01-01") });
  assert.deepStrictEqual(valid.map((d) => d.key), ["urn:uuid:vc-a", "urn:uuid:vc-b"]);

  // 6. Any JSON-LD document, with a context persisted in the store.
  const docs = store.jsonld();
  await docs.putContext("https://example.org/contexts/recipe.jsonld", {
    "@context": { "@vocab": "https://schema.org/", ingredients: "recipeIngredient" },
  });
  const recipe = await docs.put(
    `{"@context": "https://example.org/contexts/recipe.jsonld", "@id": "https://example.org/recipes/pancakes", "@type": "Recipe", "name": "Pancakes", "ingredients": ["flour", "milk", "eggs"]}`,
  );
  const [n] = await store.query(`SELECT (COUNT(*) AS ?n) WHERE { GRAPH <${recipe}> { ?s <https://schema.org/recipeIngredient> ?i } }`);
  assert.strictEqual(n.get("n").value, "3");

  // 7. Graphs edited by SPARQL UPDATE are found and rebuilt from the stored JSON.
  await store.update(`DELETE WHERE { GRAPH <${id}> { ?d <https://schema.org/name> ?name } }`);
  assert.deepStrictEqual(await vcs.documents.check(), [{ key: id, missing: 1, extra: 0 }]);
  assert.strictEqual(await vcs.documents.rebuild(id), true);
  assert.deepStrictEqual(await vcs.documents.check(), []);

  // 8. Invalid credentials are rejected, and nothing is written.
  await assert.rejects(
    async () => vcs.put({ "@context": ["https://www.w3.org/ns/credentials/v2"], type: ["VerifiableCredential"], credentialSubject: { id: "did:x" } }),
    (e) => e.code === "invalid" && /issuer/.test(e.message),
  );
}
