// Versioning on a real local D1 (Miniflare): the store clock, the change log and time travel.
import assert from "node:assert";
import { Miniflare } from "miniflare";
import { afterAll, beforeEach, describe, it } from "vitest";
import { D1Store, type Term } from "../src/node.js";

const mf = new Miniflare({ modules: true, script: "export default {}", d1Databases: ["V"] });
afterAll(() => mf.dispose());

type Db = Awaited<ReturnType<typeof mf.getD1Database>>;
let db: Db;

beforeEach(async () => {
  db = await mf.getD1Database("V");
  const tables = (await db
    .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_cf_%'")
    .raw()) as [string][];
  if (tables.length) await db.batch(tables.map(([t]) => db.prepare(`DROP TABLE IF EXISTS "${t}"`)));
});

const statuses = async (store: Awaited<ReturnType<typeof D1Store.open>>, asOf?: string) => {
  const rows = (await store.query('SELECT ?t ?s { ?t <http://example.com/status> ?s } ORDER BY ?t', {
    as_of: asOf,
  })) as Map<string, Term>[];
  return rows.map((r) => `${r.get("t")?.value} ${r.get("s")?.value}`);
};

describe("versioning on D1", () => {
  // @lat: [[tests#Versioning#D1 driver keeps history]]
  it("records commits and answers as-of queries", async () => {
    const store = await D1Store.open(db, { versioning: "log" });
    await store.withCommit({ author: "ada", message: "seed" }, (s) =>
      s.update('INSERT DATA { <http://example.com/t1> <http://example.com/status> "open" . <http://example.com/t2> <http://example.com/status> "open" }'),
    );
    await store.update(
      'DELETE { <http://example.com/t1> <http://example.com/status> ?s } INSERT { <http://example.com/t1> <http://example.com/status> "done" } WHERE { <http://example.com/t1> <http://example.com/status> ?s }',
    );
    assert.deepStrictEqual(await statuses(store), ["http://example.com/t1 done", "http://example.com/t2 open"]);
    assert.deepStrictEqual(await statuses(store, "HEAD~1"), ["http://example.com/t1 open", "http://example.com/t2 open"]);

    const status = await store.versioning();
    assert.strictEqual(status.level, "log");
    assert.strictEqual(status.history, "live");
    const log = await store.history();
    const seed = log.find((c) => c.message === "seed");
    assert.strictEqual(seed?.author, "ada");
    assert.strictEqual(seed?.added, 2);

    const diff = await store.diff("HEAD~1");
    assert.deepStrictEqual(
      diff.map((c) => `${c.added ? "+" : "-"}${c.quad.object.value}`).sort(),
      ["+done", "-open"],
    );
    const compare = (await store.query(
      'SELECT ?t ?old { ?t <http://example.com/status> ?new SERVICE <oxilite:version/HEAD~1> { ?t <http://example.com/status> ?old } FILTER(?old != ?new) }',
    )) as Map<string, Term>[];
    assert.strictEqual(compare.length, 1);
    assert.strictEqual(compare[0]?.get("old")?.value, "open");
  });

  it("changes level with a migration and keeps the store clock", async () => {
    const plain = await D1Store.open(db);
    await plain.update('INSERT DATA { <http://example.com/t1> <http://example.com/status> "open" }');
    await db.exec(D1Store.levelChangeSql("off", "log"));
    const store = await D1Store.open(db, { migrated: true });
    assert.strictEqual((await store.versioning()).level, "log");
    await store.update('INSERT DATA { <http://example.com/t2> <http://example.com/status> "open" }');
    assert.deepStrictEqual(await statuses(store, "HEAD~1"), ["http://example.com/t1 open"]);
    const frozen = await store.setVersioning("stamped");
    assert.strictEqual(frozen.history, "frozen");
    const since = await store.changes(0);
    assert(since.length >= 2);
  });
});

describe("history queries on D1", () => {
  // @lat: [[tests#Versioning#D1 driver queries the history]]
  it("reads the history graph and Cypher at a version", async () => {
    const store = await D1Store.open(db, { versioning: "log" });
    await store.withCommit({ author: "ada", message: "grant" }, (s) =>
      s.update("INSERT DATA { <http://example.com/alice> <http://example.com/role> <http://example.com/admin> }"),
    );
    await store.withCommit({ author: "bob", message: "revoke" }, (s) =>
      s.update("DELETE DATA { <http://example.com/alice> <http://example.com/role> <http://example.com/admin> }"),
    );
    const who = (await store.query(
      `PREFIX prov: <http://www.w3.org/ns/prov#> PREFIX oxl: <https://oxilite.dev/ns#>
       SELECT ?who WHERE { GRAPH <oxilite:history> {
         ?c oxl:removed <<( <http://example.com/alice> <http://example.com/role> <http://example.com/admin> )>> ;
            prov:wasAssociatedWith ?who } }`,
    )) as Map<string, Term>[];
    assert.strictEqual(who[0]?.get("who")?.value, "bob");

    await store.cypher("CREATE (:Ticket {name: 't1', status: 'open'})");
    await store.cypher("MATCH (t:Ticket) SET t.status = 'done'");
    const before = await store.cypher("MATCH (t:Ticket) RETURN t.status AS s", {}, { asOf: "HEAD~1" });
    assert.strictEqual(before.rows[0]?.[0], "open");
    await assert.rejects(store.cypher("MATCH (t:Ticket) SET t.status = 'x'", {}, { asOf: "HEAD~1" }));
  });
});
