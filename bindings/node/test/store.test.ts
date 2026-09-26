// @oxilite/node beyond Oxigraph's API: files, a dlopen'ed SQLite, explain and bulk loads.
import assert from "node:assert";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "vitest";
import { DataFactory, Store, type Term } from "../dist/index.js";

const { namedNode, literal, quad, defaultGraph } = DataFactory;
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

  // @lat: [[tests#Node#Schema registry]]
  it("registers ontology and shapes graphs", () => {
    const store = new Store();
    const good = namedNode("http://example.com/onto/good");
    const bad = namedNode("http://example.com/onto/bad");
    const shapes = namedNode("http://example.com/shapes");
    store.load(
      `@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
       ex:Dog rdfs:subClassOf ex:Animal .`,
      { format: "text/turtle", to_graph_name: good },
    );
    store.load(
      `@prefix ex: <http://example.com/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
       ex:Dog rdfs:subClassOf ex:Plant .`,
      { format: "text/turtle", to_graph_name: bad },
    );
    store.load(
      `@prefix ex: <http://example.com/> . @prefix sh: <http://www.w3.org/ns/shacl#> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
       ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
         sh:property [ sh:path ex:age ; sh:datatype xsd:integer ; sh:maxCount 1 ] .`,
      { format: "text/turtle", to_graph_name: shapes },
    );
    store.add(quad(namedNode("http://example.com/rex"), namedNode("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"), namedNode("http://example.com/Dog")));
    const plants = "SELECT ?x WHERE { ?x a <http://example.com/Plant> }";
    assert.strictEqual((store.query(plants, { reasoning: "rdfs" }) as unknown[]).length, 1, "every graph counts before registering");

    store.registerSchemaGraph(good, "ontology", { version: "v1", imports: ["http://example.com/onto/base"] });
    store.registerSchemaGraph("http://example.com/shapes", "shacl");
    assert.strictEqual((store.query(plants, { reasoning: "rdfs" }) as unknown[]).length, 0, "only the registered ontology counts");
    const entries = store.schemaGraphs();
    assert.deepStrictEqual(
      entries.map((e) => [e.graph.value, e.role, e.active]),
      [
        ["http://example.com/onto/good", "ontology", true],
        ["http://example.com/shapes", "shacl", true],
      ],
    );
    assert.strictEqual(entries[0]?.version, "v1");
    assert.deepStrictEqual(entries[0]?.imports, ["http://example.com/onto/base"]);
    assert.match(entries[0]!.loadedAt ?? "", /^\d{4}-\d{2}-\d{2}T/);
    assert.deepStrictEqual(entries[0]?.appliesTo, []);

    const shape = store.shapeIndex().find((e) => e.path === "http://example.com/age");
    assert.strictEqual(shape?.target, "http://example.com/Person");
    assert.strictEqual(shape?.datatype, "http://www.w3.org/2001/XMLSchema#integer");
    assert.strictEqual(shape?.maxCount, 1);

    const count = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";
    const n = (o: object) => Number((store.query(count, o) as Map<string, Term>[])[0]?.get("n")?.value);
    assert.strictEqual(n({ include_schema_graphs: false }), 1, "only the bad ontology is neither schema nor registry");
    assert(n({}) > 1);

    // Mapped to one graph: the ontology no longer entails for rex in the default graph.
    store.registerSchemaGraph(good, "ontology", { appliesTo: ["http://example.com/elsewhere"] });
    assert.deepStrictEqual(store.schemaGraphs()[0]?.appliesTo, ["http://example.com/elsewhere"]);
    const animals = "SELECT ?x WHERE { ?x a <http://example.com/Animal> }";
    assert.strictEqual((store.query(animals, { reasoning: "rdfs" }) as unknown[]).length, 0);
    store.registerSchemaGraph(good, "ontology", { appliesTo: [defaultGraph()] });
    assert.strictEqual((store.query(animals, { reasoning: "rdfs" }) as unknown[]).length, 1);
    // The registry is RDF: SPARQL sees it.
    assert.strictEqual(
      store.query("ASK { GRAPH <oxilite:schema> { <http://example.com/onto/good> a <https://oxilite.dev/ns#OntologyGraph> } }"),
      true,
    );
    assert(store.setSchemaGraphActive(good, false));
    assert.strictEqual(store.schemaGraphs()[0]?.active, false);
    assert(store.unregisterSchemaGraph(good));
    assert(!store.unregisterSchemaGraph(good));
    assert(store.dropSchemaGraph(shapes) > 0);
    assert.deepStrictEqual(store.schemaGraphs(), []);
    assert.strictEqual(store.shapeIndex().length, 0);
    assert.throws(() => store.registerSchemaGraph(good, "taxonomy" as "shacl"), /unknown schema role/);
  });

  // @lat: [[tests#Node#System graphs]]
  it("starts a blank store with the system graphs on request", () => {
    const store = new Store({ systemGraphs: true });
    assert.strictEqual(store.query("ASK { GRAPH <oxilite:vocabulary> { <https://oxilite.dev/ns#appliesTo> ?p ?o } }"), true);
    assert.strictEqual(store.installSystemGraphs(), false);
    assert.deepStrictEqual(store.schemaGraphs(), []);
    const plain = new Store();
    assert.strictEqual(plain.size, 0, "a default store is empty, as in Oxigraph");
    assert.strictEqual(plain.installSystemGraphs(), true);
    assert(plain.size > 0);
  });

  it("schema SQL", () => {
    assert(Store.schemaSql().includes("CREATE TABLE IF NOT EXISTS quads"));
  });
});
