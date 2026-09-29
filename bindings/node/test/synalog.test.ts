// Synalog through the Node binding: the store as tables, declared tables, recursion, options.
import { describe, expect, it } from "vitest";
import { Store } from "../lib/index.js";

const PARENT = "# @table parent <http://example.org/parent>\n";

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

describe("synalog", () => {
  // @lat: [[tests#Node#Synalog runs over the store]]
  it("runs recursion over a declared table and decodes numbers", () => {
    const s = store();
    const r = s.synalog(
      `${PARENT}@Recursive(Ancestor, 10);
       @OrderBy(Ancestor, "y");
       Ancestor(x:, y:) distinct :- parent(subject: x, object: y);
       Ancestor(x:, y:) distinct :- Ancestor(x:, y: m), parent(subject: m, object: y);
       FromAda(y:) :- Ancestor(x: "http://example.org/ada", y:);`,
      "FromAda",
    );
    expect(r.columns).toEqual(["y"]);
    expect(r.rows.map((row) => row[0])).toEqual([
      "http://example.org/bob",
      "http://example.org/cy",
      "http://example.org/dee",
    ]);
    const ages = s.synalog(
      `Old(p:, age:) :- triples(subject: p, predicate: "http://example.org/age", object: age), age > 18;`,
      "Old",
    );
    expect(ages.records).toEqual([{ p: "http://example.org/ada", age: 42 }]);
  });

  it("takes tables and pagination from the options", () => {
    const r = store().synalog(
      `@OrderBy(P, "x");\nP(x:) :- par(subject: x);`,
      "P",
      { tables: [{ name: "par", predicate: "http://example.org/parent" }], limit: 1, offset: 1 },
    );
    expect(r.rows).toEqual([["http://example.org/bob"]]);
    expect(store().synalogSql(`${PARENT}P(x:) :- parent(subject: x);`, "P")).toContain("NOT MATERIALIZED");
  });

  it("rejects what the store cannot run", () => {
    expect(() =>
      store().synalog(`O(p? ArgMax= p -> a) distinct :- triples(subject: p, object: a);`, "O"),
    ).toThrow(/ArgMax/);
  });
});
