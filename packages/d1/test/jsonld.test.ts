// JSON-LD documents and Verifiable Credentials through @oxilite/d1 on a real local D1
// (Miniflare): every write is one D1 batch, contexts load offline.
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { Miniflare } from "miniflare";
import { afterAll, beforeEach, describe, it } from "vitest";
import { D1Store, JsonLdError, NamedNode } from "../src/node.js";

const fixture = (name: string) =>
  readFileSync(new URL(`../../../crates/oxilite-vc/tests/fixtures/${name}`, import.meta.url), "utf8");

const mf = new Miniflare({ modules: true, script: "export default {}", d1Databases: ["DB"] });
afterAll(() => mf.dispose());

let store: Awaited<ReturnType<typeof D1Store.open>>;

beforeEach(async () => {
  const db = await mf.getD1Database("DB");
  const tables = await db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_cf_%'").raw();
  if (tables.length) await db.batch(tables.map(([t]) => db.prepare(`DROP TABLE IF EXISTS "${t}"`)));
  store = await D1Store.open(db);
});

describe("@oxilite/d1 JSON-LD", () => {
  // @lat: [[tests#D1#JSON-LD documents on D1]]
  it("stores, queries, replaces and removes documents", async () => {
    const docs = store.jsonld();
    const v1 = '{"@context": {"p": "http://example.org/p"}, "@id": "urn:d", "p": "1"}';
    assert.strictEqual(await docs.put(v1), "urn:d");
    assert.strictEqual((await docs.get("urn:d"))?.json, v1);
    await docs.put({ "@context": { p: "http://example.org/p" }, "@id": "urn:d", p: "2" });
    assert.strictEqual(await store.query('ASK { GRAPH <urn:d> { <urn:d> <http://example.org/p> "2" } }'), true);
    assert.strictEqual(await store.query('ASK { GRAPH ?g { ?s ?p "1" } }'), false);
    assert.strictEqual((await docs.documentForGraph(new NamedNode("urn:d")))?.key, "urn:d");
    await assert.rejects(docs.put({ "@context": "https://example.org/ctx", "@id": "urn:c" }), (e: unknown) => e instanceof JsonLdError && e.code === "loading remote context failed");
    await docs.putContext("https://example.org/ctx", { "@context": { name: "http://schema.org/name" } });
    assert.strictEqual(await docs.put({ "@context": "https://example.org/ctx", "@id": "urn:c", name: "c" }), "urn:c");
    assert.strictEqual(await docs.remove("urn:d"), true);
    assert.strictEqual(await store.query("ASK { GRAPH <urn:d> { ?s ?p ?o } }"), false);
    assert.deepStrictEqual(await docs.check(), []);
  });

  it("includes the JSON-LD tables in the migration schema", () => {
    const sql = D1Store.schemaSql({ jsonld: { issuer: false } });
    assert.match(sql, /CREATE TABLE IF NOT EXISTS jsonld_documents/);
    assert.doesNotMatch(sql, /jsonld_issuer/);
    assert.match(sql, /jsonld_subject/);
  });
});

describe("@oxilite/d1 Verifiable Credentials", () => {
  // @lat: [[tests#D1#Credentials on D1]]
  it("stores credentials with bundled contexts and finds them", async () => {
    const vcs = store.credentials();
    const degree = fixture("degree-v2.json");
    assert.strictEqual(await vcs.put(degree), "http://university.example/credentials/3732");
    assert.strictEqual((await vcs.get("http://university.example/credentials/3732"))?.json, degree);
    const vp = await vcs.putPresentation(fixture("presentation-v2.json"));
    assert.deepStrictEqual(vp.credentials, ["urn:uuid:vc-a", "urn:uuid:vc-b"]);
    const found = await vcs.find({ issuer: "did:example:academy", validAt: new Date("2024-01-01") });
    assert.deepStrictEqual(found.map((d) => d.key), ["urn:uuid:vc-a", "urn:uuid:vc-b"]);
    assert.strictEqual(
      await store.query("ASK { GRAPH <http://university.example/credentials/3732> { ?vc a <https://www.w3.org/2018/credentials#VerifiableCredential> } }"),
      true,
    );
    assert.strictEqual((await vcs.documents.graphs("http://university.example/credentials/3732")).length, 2);
    assert.strictEqual(await vcs.remove("http://university.example/credentials/3732"), true);
    await assert.rejects(vcs.put({ "@context": ["https://example.org"], type: ["VerifiableCredential"] }), (e: unknown) => e instanceof JsonLdError && e.code === "invalid");
  });
});
