// JSON-LD documents and Verifiable Credentials through @oxilite/node.
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { describe, it } from "vitest";
import { JsonLdError, NamedNode, Store } from "../dist/index.js";

const fixture = (name: string) =>
  readFileSync(new URL(`../../../crates/oxilite-vc/tests/fixtures/${name}`, import.meta.url), "utf8");

const person = `{
  "@context": {"name": "http://schema.org/name"},
  "@id": "urn:uuid:1234",
  "name": "Ada"
}`;

describe("@oxilite/node JSON-LD", () => {
  // @lat: [[tests#Node#JSON-LD documents round-trip and query]]
  it("stores documents verbatim, one graph each, queryable with SPARQL", () => {
    const store = new Store();
    const docs = store.jsonld();
    assert.strictEqual(docs.put(person), "urn:uuid:1234");
    const d = docs.get("urn:uuid:1234");
    assert.ok(d);
    assert.strictEqual(d.json, person);
    assert.strictEqual(d.graph.value, "urn:uuid:1234");
    assert.ok(d.storedAt instanceof Date);
    assert.strictEqual(store.query('ASK { GRAPH <urn:uuid:1234> { ?s <http://schema.org/name> "Ada" } }'), true);
    assert.strictEqual(docs.documentForGraph(new NamedNode("urn:uuid:1234"))?.key, "urn:uuid:1234");
    // Objects are serialized; options pick keys and graphs.
    const other = store.jsonld({ key: { pointer: "/name" }, graph: { template: "https://example.org/g/{key}" } });
    assert.strictEqual(other.put({ "@context": { name: "http://schema.org/name" }, name: "Alan" }), "Alan");
    assert.strictEqual(store.query("ASK { GRAPH <https://example.org/g/Alan> { ?s ?p ?o } }"), true);
    assert.strictEqual(docs.remove("urn:uuid:1234"), true);
    assert.strictEqual(docs.remove("urn:uuid:1234"), false);
    assert.strictEqual(store.query("ASK { GRAPH <urn:uuid:1234> { ?s ?p ?o } }"), false);
  });

  it("loads persisted contexts offline and reports JSON-LD errors with codes", () => {
    const store = new Store();
    const docs = store.jsonld();
    const doc = { "@context": "https://example.org/ctx.jsonld", "@id": "urn:c", name: "x" };
    assert.throws(() => docs.put(doc), (e: unknown) => e instanceof JsonLdError && e.code === "loading remote context failed");
    docs.putContext("https://example.org/ctx.jsonld", { "@context": { name: "http://schema.org/name" } });
    assert.deepStrictEqual(docs.contexts(), ["https://example.org/ctx.jsonld"]);
    assert.strictEqual(docs.put(doc), "urn:c");
    assert.throws(() => docs.put({ name: "no id" }), (e: unknown) => e instanceof JsonLdError && e.code === "missing-key");
    store.update("DELETE WHERE { GRAPH <urn:c> { ?s ?p ?o } }");
    assert.deepStrictEqual(docs.check().map((d) => d.key), ["urn:c"]);
    assert.strictEqual(docs.rebuild("urn:c"), true);
    assert.deepStrictEqual(docs.check(), []);
  });
});

describe("@oxilite/node Verifiable Credentials", () => {
  // @lat: [[tests#Node#Credentials are stored and found]]
  it("stores credentials under their id and finds them by metadata", () => {
    const store = new Store();
    const vcs = store.credentials();
    const degree = fixture("degree-v2.json");
    assert.strictEqual(vcs.put(degree), "http://university.example/credentials/3732");
    const stored = vcs.get("http://university.example/credentials/3732");
    assert.strictEqual(stored?.json, degree);
    assert.strictEqual(stored?.profile, "vc2");
    assert.strictEqual(stored?.validUntil?.toISOString(), "2040-01-01T00:00:00.000Z");
    const vp = vcs.putPresentation(fixture("presentation-v2.json"));
    assert.deepStrictEqual(vp.credentials, ["urn:uuid:vc-a", "urn:uuid:vc-b"]);
    assert.deepStrictEqual(
      vcs.find({ issuer: "did:example:academy", validAt: new Date("2024-01-01") }).map((d) => d.key),
      ["urn:uuid:vc-a", "urn:uuid:vc-b"],
    );
    assert.strictEqual(vcs.find({ type: "ExampleDegreeCredential" }).length, 1);
    assert.strictEqual(
      store.query("ASK { GRAPH <http://university.example/credentials/3732> { ?vc a <https://www.w3.org/2018/credentials#VerifiableCredential> } }"),
      true,
    );
    assert.throws(() => vcs.put({ "@context": ["https://example.org"], type: ["VerifiableCredential"] }), (e: unknown) => e instanceof JsonLdError && e.code === "invalid");
    const noId = vcs.put(fixture("no-id-v2.json"));
    assert.match(noId, /^urn:oxilite:doc:sha256:[0-9a-f]{64}$/);
    assert.strictEqual(vcs.documents.graphs("http://university.example/credentials/3732").length, 2);
  });
});
