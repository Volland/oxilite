// @oxilite/node beyond Oxigraph's API: files, a dlopen'ed SQLite, explain and bulk loads.
import assert from "node:assert";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "vitest";
import { DataFactory, Store, type Term } from "../dist/index.js";

const { namedNode, literal, quad } = DataFactory;
const ex = namedNode("http://example.com");

describe("@oxilite/node", () => {
  // @lat: [[tests#Node#File store persists across processes]]
  it("a file store persists across processes", () => {
    const path = join(mkdtempSync(join(tmpdir(), "oxilite-")), "data.sqlite");
    const script = `import { Store, DataFactory as F } from ${JSON.stringify(new URL("../dist/index.js", import.meta.url).href)};
      const s = new Store(${JSON.stringify(path)}); s.add(F.quad(F.namedNode("http://example.com"), F.namedNode("http://example.com"), F.literal("x")));`;
    execFileSync(process.execPath, ["--input-type=module", "-e", script]);
    const store = new Store(path);
    assert(store.has(quad(ex, ex, literal("x"))));
    assert.strictEqual(store.size, 1);
  });

  it("opens a store through a SQLite shared library", (ctx) => {
    // macOS system libraries live in the dyld shared cache: they load but are not files.
    const library =
      process.env.OXILITE_SQLITE_LIBRARY ??
      (process.platform === "darwin"
        ? "/usr/lib/libsqlite3.dylib"
        : ["/usr/lib/x86_64-linux-gnu/libsqlite3.so.0", "/usr/lib/aarch64-linux-gnu/libsqlite3.so.0"].find(existsSync));
    if (!library) return ctx.skip();
    const store = new Store({ library });
    store.load("<http://example.com> <http://example.com> 1, 2, 3 .", { format: "text/turtle" });
    const r = store.query("SELECT (SUM(?o) AS ?s) WHERE { ?x ?p ?o }") as Map<string, Term>[];
    assert.strictEqual(r[0]?.get("s")?.value, "6");
  });

  // @lat: [[tests#Node#Explain returns SQL]]
  it("explains queries and updates", () => {
    const store = new Store();
    const plan = store.explain("SELECT * WHERE { ?s ?p ?o }");
    assert(plan.includes("SELECT"), plan);
    assert(store.explainUpdate("DELETE WHERE { ?s ?p ?o }").includes("compiled to"));
  });

  it("bulk loads, optimizes and dumps", () => {
    const store = new Store({ graphIndex: false });
    const lines = Array.from({ length: 2000 }, (_, i) => `<http://example.com/s${i}> <http://example.com/p> "v${i}" .`);
    store.bulkLoad(lines.join("\n"), { format: "application/n-triples" });
    assert.strictEqual(store.size, 2000);
    store.optimize();
    assert.strictEqual(store.match(namedNode("http://example.com/s7")).length, 1);
    assert.strictEqual(store.dump({ format: "application/n-triples" }).split("\n").length, 2001);
    store.clear();
    assert.strictEqual(store.size, 0);
  });

  it("RDF 1.2 triple terms and directional language strings", () => {
    const store = new Store();
    store.update(`PREFIX ex: <http://example.com/>
      INSERT DATA { ex:a ex:says <<( ex:b ex:c "hi"@en--ltr )>> }`);
    const r = store.query("SELECT ?o WHERE { ?s ?p ?o }") as Map<string, Term>[];
    const t = r[0]?.get("o");
    assert.strictEqual(t?.termType, "Quad");
    assert(t?.termType === "Quad" && t.object.termType === "Literal" && t.object.direction === "ltr");
  });

  it("reports SPARQL errors as exceptions", () => {
    const store = new Store();
    assert.throws(() => store.query("SELECT WHERE"), /error|expected/i);
    store.update("CREATE GRAPH <http://example.com/g>");
    assert.throws(() => store.update("CREATE GRAPH <http://example.com/g>"), /already exists/);
  });

  // @lat: [[tests#Node#Reasoning options]]
  it("reasons per query and materializes OWL 2 RL", () => {
    const store = new Store();
    store.load(
      `@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . @prefix owl: <http://www.w3.org/2002/07/owl#> .
       ex:Dog rdfs:subClassOf ex:Animal . ex:rex a ex:Dog . ex:rex owl:sameAs ex:rexy . ex:rex ex:name "Rex" .`,
      { format: "text/turtle" },
    );
    const q = "SELECT ?x WHERE { ?x a <http://example.com/Animal> }";
    assert.strictEqual((store.query(q) as unknown[]).length, 0);
    assert.strictEqual((store.query(q, { reasoning: "rdfs" }) as unknown[]).length, 1);
    for (const engine of ["sql", "reasonable"] as const) {
      assert(store.materialize({ engine }) > 0);
      const r = store.query("SELECT ?n WHERE { <http://example.com/rexy> <http://example.com/name> ?n }", { include_inferred: true }) as Map<string, Term>[];
      assert.strictEqual(r[0]?.get("n")?.value, "Rex");
    }
    store.clearInferences();
    assert.strictEqual((store.query(q, { include_inferred: true }) as unknown[]).length, 0);
  });

  it("schema SQL", () => {
    assert(Store.schemaSql().includes("CREATE TABLE IF NOT EXISTS quads"));
  });
});
