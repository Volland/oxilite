// @oxilite/d1 against a real local D1 (Miniflare, the engine behind `wrangler dev`).
import assert from "node:assert";
import { Miniflare } from "miniflare";
import { afterAll, beforeEach, describe, it } from "vitest";
import { D1Store, DataFactory, OxiliteCollisionError, type Quad, type Term } from "../src/node.js";

const { namedNode, literal, quad, defaultGraph } = DataFactory;
const ex = namedNode("http://example.com");
const ex2 = namedNode("http://example.com/2");

const mf = new Miniflare({ modules: true, script: "export default {}", d1Databases: ["DB"] });
afterAll(() => mf.dispose());

let store: Awaited<ReturnType<typeof D1Store.open>>;

beforeEach(async () => {
  const db = await mf.getD1Database("DB");
  store = await D1Store.open(db);
  await store.clear();
});

describe("D1Store (Oxigraph JS API, async)", () => {
  // @lat: [[tests#D1#Store API on Miniflare D1]]
  it("add / has / delete / size / match", async () => {
    await store.add(quad(ex, ex, literal("o"), ex2));
    assert(await store.has(quad(ex, ex, literal("o"), ex2)));
    assert.strictEqual(await store.size(), 1);
    const m = await store.match(ex, null, null, null);
    assert.strictEqual(m.length, 1);
    assert(quad(ex, ex, literal("o"), ex2).equals(m[0]));
    await store.delete(quad(ex, ex, literal("o"), ex2));
    assert(!(await store.has(quad(ex, ex, literal("o"), ex2))));
  });

  it("SELECT / ASK / CONSTRUCT", async () => {
    await store.add(quad(ex, ex, ex));
    const sel = (await store.query("SELECT ?s WHERE { ?s ?p ?o }")) as Map<string, Term>[];
    assert.strictEqual(sel.length, 1);
    assert(ex.equals(sel[0]?.get("s")));
    assert.strictEqual(await store.query("ASK { ?s ?s ?s }"), true);
    const c = (await store.query("CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }")) as Quad[];
    assert(quad(ex, ex, ex).equals(c[0]));
  });

  it("query options: union graph, default graph, named graphs, results format", async () => {
    await store.addAll([quad(ex, ex, ex, ex), quad(ex, ex, ex, ex2)]);
    assert.strictEqual(((await store.query("SELECT * WHERE { ?s ?p ?o }", { use_default_graph_as_union: true })) as unknown[]).length, 1);
    assert.strictEqual(((await store.query("SELECT * WHERE { ?s ?p ?o }", { default_graph: ex })) as unknown[]).length, 1);
    assert.strictEqual(((await store.query("SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }", { named_graphs: [ex] })) as unknown[]).length, 1);
    assert.strictEqual(await store.query("ASK { GRAPH ?g { ?s ?p ?o } }", { results_format: "csv" }), "true");
    const json = await store.queryJson("SELECT ?g WHERE { GRAPH ?g { } } ORDER BY ?g");
    assert(json.includes('"http://example.com"'));
  });

  it("update: INSERT DATA / DELETE WHERE / DELETE-INSERT with a counter", async () => {
    await store.update("INSERT DATA { <http://example.com/c> <http://example.com/count> 41 }");
    await store.update(
      "DELETE { ?c <http://example.com/count> ?n } INSERT { ?c <http://example.com/count> ?m } WHERE { ?c <http://example.com/count> ?n BIND(?n + 1 AS ?m) }",
    );
    const r = (await store.query("SELECT ?n WHERE { ?c <http://example.com/count> ?n }")) as Map<string, Term>[];
    assert.strictEqual(r[0]?.get("n")?.value, "42");
    await store.update("DELETE WHERE { ?s ?p ?o }");
    assert.strictEqual(await store.size(), 0);
  });

  it("load and dump", async () => {
    await store.load("<http://example.com> <http://example.com> <> .", { format: "text/turtle", base_iri: "http://example.com" });
    await store.load("GRAPH <> { <http://example.com> <http://example.com> <> }", { format: "application/trig", base_iri: "http://example.com" });
    assert.strictEqual(
      await store.dump({ format: "application/n-triples", from_graph_name: defaultGraph() }),
      "<http://example.com> <http://example.com> <http://example.com> .\n",
    );
    assert(await store.has(quad(ex, ex, ex, ex)));
  });

  // @lat: [[tests#D1#Ids above 2^53 survive D1]]
  it("ids above 2^53 survive the JavaScript round-trip", async () => {
    // Every hashed id is a 60-bit integer: a lossy transport would corrupt them all.
    const terms = Array.from({ length: 50 }, (_, i) => namedNode(`http://example.com/n${i}`));
    await store.addAll(terms.map((t) => quad(t, ex, literal(`v${t.value}`))));
    const r = (await store.query("SELECT ?s ?o WHERE { ?s ?p ?o }")) as Map<string, Term>[];
    assert.strictEqual(r.length, 50);
    for (const row of r) assert.strictEqual(row.get("o")?.value, `v${row.get("s")?.value}`);
  });

  // @lat: [[tests#D1#Failed update leaves D1 unchanged]]
  it("a failing update is rolled back as one batch", async () => {
    await store.update("CREATE GRAPH <http://example.com/g>");
    await assert.rejects(
      store.update("INSERT DATA { <http://example.com/a> <http://example.com/b> 1 } ; CREATE GRAPH <http://example.com/g>"),
      /already exists/,
    );
    assert.strictEqual(await store.size(), 0);
  });

  // @lat: [[tests#D1#Large loads respect D1 limits]]
  it("bulk loads split into batches under D1's statement limit", async () => {
    const lines = Array.from(
      { length: 6000 },
      (_, i) => `<http://example.com/s${i}> <http://example.com/p> "a fairly long literal value number ${i} to fill statements" .`,
    );
    await store.bulkLoad(lines.join("\n"), { format: "application/n-triples" });
    assert.strictEqual(await store.size(), 6000);
    const plan = store.explain("SELECT * WHERE { ?s <http://example.com/p> ?o }");
    assert(plan.includes("statistics"), plan);
  });

  it("collision errors are typed", () => {
    assert(new OxiliteCollisionError("x") instanceof Error);
  });

  it("explains queries and updates", () => {
    assert(store.explain("SELECT * WHERE { ?s ?p ?o }").startsWith("-- oxilite: fully compiled"));
    assert(store.explainUpdate("DELETE WHERE { ?s ?p ?o }").includes("compiled to"));
  });

  it("schema SQL for migrations", () => {
    const sql = D1Store.schemaSql();
    assert(sql.includes("CREATE TABLE IF NOT EXISTS quads"));
    assert(!D1Store.schemaSql({ graphIndex: false }).includes("quads_gspo"));
  });
});
