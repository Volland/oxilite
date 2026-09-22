// Cypher through @oxilite/node: the property-graph view of the same dataset SPARQL sees.
import assert from "node:assert";
import { describe, it } from "vitest";
import { type CypherNode, type CypherRelationship, Store } from "../dist/index.js";

const ex = { base: "http://example.com/" };

describe("@oxilite/node Cypher", () => {
  // @lat: [[tests#Node#Cypher reads and writes the same dataset]]
  it("writes with Cypher and reads with SPARQL, and the other way round", () => {
    const store = new Store();
    const created = store.cypher("CREATE (a:Person {name: $name})-[:KNOWS {since: 2020}]->(b:Person {name: 'Alan'}) RETURN a", { name: "Ada" }, ex);
    assert.strictEqual(created.stats.nodesCreated, 2);
    assert.strictEqual((created.rows[0]?.[0] as CypherNode).labels[0], "Person");
    const ask = store.query('ASK { ?p a <http://example.com/Person> ; <http://example.com/name> "Ada" }');
    assert.strictEqual(ask, true);
    store.update('INSERT DATA { <http://example.com/carol> a <http://example.com/Person> ; <http://example.com/name> "Carol" }');
    const r = store.cypher("MATCH (p:Person) RETURN p.name AS name ORDER BY name", {}, ex);
    assert.deepStrictEqual(r.records.map((x) => x.name), ["Ada", "Alan", "Carol"]);
    const k = store.cypher("MATCH ()-[k:KNOWS]->() RETURN k", {}, ex).rows[0]?.[0] as CypherRelationship;
    assert.strictEqual(k.relType, "KNOWS");
    assert.strictEqual(k.properties.since, 2020);
    assert.match(store.explainCypher("MATCH (p:Person) RETURN p", {}, ex), /SELECT/);
  });

  it("reports Cypher errors", () => {
    const store = new Store();
    assert.throws(() => store.cypher("MATCH (n) RETURN m"), /UndefinedVariable/);
  });
});
