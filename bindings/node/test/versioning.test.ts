// Versioning through the native addon: the store clock, the change log and time travel.
import assert from "node:assert";
import { describe, it } from "vitest";
import { Store, type Term } from "../dist/index.js";

const statuses = (store: Store, asOf?: string) =>
  (store.query("SELECT ?t ?s { ?t <http://example.com/status> ?s } ORDER BY ?t", { as_of: asOf }) as Map<string, Term>[]).map(
    (r) => `${r.get("t")?.value} ${r.get("s")?.value}`,
  );

describe("versioning (native)", () => {
  // @lat: [[tests#Versioning#Node binding keeps history]]
  it("records commits, time travels, and freezes", () => {
    const store = new Store({ versioning: "log" });
    store.withCommit({ author: "ada", message: "seed" }, (s) =>
      s.update('INSERT DATA { <http://example.com/t1> <http://example.com/status> "open" }'),
    );
    store.update('DELETE DATA { <http://example.com/t1> <http://example.com/status> "open" } ; INSERT DATA { <http://example.com/t1> <http://example.com/status> "done" }');
    assert.deepStrictEqual(statuses(store), ["http://example.com/t1 done"]);
    assert.deepStrictEqual(statuses(store, "HEAD~1"), ["http://example.com/t1 open"]);
    assert.strictEqual(store.history().find((c) => c.message === "seed")?.author, "ada");
    assert.strictEqual(store.diff("HEAD~1").length, 2);
    const datalog = store.datalog('?- <http://example.com/status>(?t, "open").', { asOf: "HEAD~1" });
    assert.strictEqual(datalog.rows.length, 1);
    assert.strictEqual(store.setVersioning("stamped").history, "frozen");
    assert.strictEqual(store.versioning().level, "stamped");
  });

  // @lat: [[tests#Versioning#Node binding queries the history]]
  it("queries the history graph, Cypher and Datalog history", () => {
    const store = new Store({ versioning: "log" });
    store.withCommit({ author: "ada" }, (s) => s.cypher("CREATE (:Ticket {name: 't1', status: 'open'})"));
    store.withCommit({ author: "bob" }, (s) => s.cypher("MATCH (t:Ticket) SET t.status = 'done'"));
    assert.strictEqual(store.cypher("MATCH (t:Ticket) RETURN t.status AS s", {}, { asOf: "HEAD~1" }).rows[0]?.[0], "open");
    const who = store.query(
      `PREFIX prov: <http://www.w3.org/ns/prov#> SELECT DISTINCT ?who WHERE { GRAPH <oxilite:history> { ?c prov:wasAssociatedWith ?who } } ORDER BY ?who`,
    ) as Map<string, Term>[];
    assert.deepStrictEqual(who.map((r) => r.get("who")?.value), ["ada", "bob"]);
    const removed = store.datalog("?- removed(?s, ?p, ?o, ?g, ?c).");
    assert.strictEqual(removed.rows.length, 1);
  });
});
