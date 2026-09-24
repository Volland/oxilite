// The Datalog dialect through the Node binding: rules, recursion and materialization.
import { describe, expect, it } from "vitest";
import { Store } from "../lib/index.js";

const PREFIX = "@prefix ex: <http://example.org/> .\n";

function store(): Store {
  const s = new Store();
  s.load(
    `@prefix ex: <http://example.org/> .
     ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy . ex:cy ex:parent ex:dee .
     ex:ada ex:age 42 . ex:bob ex:age 17 .`,
    { format: "text/turtle" },
  );
  return s;
}

describe("datalog", () => {
  it("returns solutions as RDF/JS terms", () => {
    const r = store().datalog(`${PREFIX}p(?x, ?y) :- ex:parent(?x, ?y).\n?- p(?x, ?y).`);
    expect(r.columns).toEqual(["x", "y"]);
    expect(r.rows).toHaveLength(3);
    expect(r.rows[0][0]?.termType).toBe("NamedNode");
    expect(r.records[0].x?.value).toMatch(/^http:\/\/example\.org\//);
  });

  it("evaluates linear recursion", () => {
    const r = store().datalog(
      `${PREFIX}anc(?x, ?y) :- ex:parent(?x, ?y).
       anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).
       ?- anc(ex:ada, ?who).`,
    );
    expect(r.rows.map((row) => row[0]?.value).sort()).toEqual([
      "http://example.org/bob",
      "http://example.org/cy",
      "http://example.org/dee",
    ]);
    expect(r.rounds).toEqual([]);
  });

  it("reports the rounds an iterated component took", () => {
    const r = store().datalog(
      `${PREFIX}p(?x, ?y) :- ex:parent(?x, ?y).
       p(?x, ?z) :- p(?x, ?y), p(?y, ?z).
       ?- p(?x, ?y).`,
    );
    expect(r.rows).toHaveLength(6);
    expect(r.rounds).toHaveLength(1);
    expect(r.rounds[0]).toBeGreaterThan(0);
  });

  it("applies constraints", () => {
    const r = store().datalog(`${PREFIX}a(?p) :- ex:age(?p, ?n), ?n >= 18.\n?- a(?p).`);
    expect(r.rows.map((row) => row[0]?.value)).toEqual(["http://example.org/ada"]);
  });

  it("materializes rules where SPARQL can see them", () => {
    const s = store();
    const stats = s.datalogMaterialize(
      `${PREFIX}ex:ancestor(?x, ?y) :- ex:parent(?x, ?y).
       ex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z).`,
    );
    expect(stats.relations).toBe(1);
    expect(stats.inferred).toBe(6);
    expect(s.query("PREFIX ex: <http://example.org/> SELECT ?y WHERE { ex:ada ex:ancestor ?y }")).toHaveLength(0);
    const withInferred = s.query(
      "PREFIX ex: <http://example.org/> SELECT ?y WHERE { ex:ada ex:ancestor ?y }",
      { include_inferred: true },
    );
    expect(withInferred).toHaveLength(3);
  });

  it("explains the strategy", () => {
    const e = store().explainDatalog(
      `${PREFIX}anc(?x, ?y) :- ex:parent(?x, ?y).
       anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).
       ?- anc(?x, ?y).`,
    );
    expect(e).toContain("linear recursion");
    expect(e).toContain("WITH RECURSIVE");
  });

  it("rejects an unstratified program", () => {
    expect(() =>
      store().datalog(`${PREFIX}p(?x) :- ex:age(?x, _), not p(?x).\n?- p(?x).`),
    ).toThrow(/not stratified/);
  });
});
