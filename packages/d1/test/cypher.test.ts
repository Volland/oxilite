// Cypher through @oxilite/d1 against a local D1 (Miniflare).
import assert from "node:assert";
import { Miniflare } from "miniflare";
import { afterAll, beforeEach, describe, it } from "vitest";
import { D1Store, type CypherPath } from "../src/node.js";

const mf = new Miniflare({ modules: true, script: "export default {}", d1Databases: ["DB"] });
afterAll(() => mf.dispose());

let store: Awaited<ReturnType<typeof D1Store.open>>;

beforeEach(async () => {
  store = await D1Store.open(await mf.getD1Database("DB"));
  await store.clear();
});

describe("D1Store Cypher", () => {
  // @lat: [[tests#D1#Cypher runs on D1]]
  it("creates, matches, finds shortest paths and deletes in D1 batches", async () => {
    await store.cypher("UNWIND range(1, 5) AS i CREATE (:N {i: i})");
    await store.cypher("MATCH (a:N), (b:N) WHERE b.i = a.i + 1 CREATE (a)-[:NEXT]->(b)");
    const r = await store.cypher("MATCH p = shortestPath((a:N {i: 1})-[:NEXT*]->(b:N {i: 5})) RETURN p, length(p) AS len");
    assert.strictEqual(r.records[0]?.len, 4);
    assert.strictEqual((r.rows[0]?.[0] as CypherPath).nodes.length, 5);
    const counts = await store.cypher("MATCH (n:N) OPTIONAL MATCH (n)-[:NEXT]->(m) RETURN count(n) AS n, count(m) AS m");
    assert.deepStrictEqual(counts.records[0], { n: 5, m: 4 });
    await store.cypher("MATCH (n:N {i: 3}) DETACH DELETE n");
    const left = await store.cypher("MATCH ()-[r:NEXT]->() RETURN count(r) AS c");
    assert.strictEqual(left.records[0]?.c, 2);
    await assert.rejects(store.cypher("MATCH (n:N {i: 1}) DELETE n"), /relationships/);
  });
});
