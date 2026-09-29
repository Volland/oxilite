// Synalog through @oxilite/d1 against a local D1 (Miniflare). The frontend is an opt-in feature
// of the WebAssembly core, so this runs only on a core built with `--features synalog`.
import assert from "node:assert";
import { createRequire } from "node:module";
import { Miniflare } from "miniflare";
import { afterAll, beforeEach, describe, it } from "vitest";
import { D1Store } from "../src/node.js";

const require = createRequire(import.meta.url);
const { Engine } = require("../wasm/node/oxilite_wasm.js") as { Engine: { prototype: object } };
const built = "synalog" in Engine.prototype;

const mf = new Miniflare({ modules: true, script: "export default {}", d1Databases: ["DB"] });
afterAll(() => mf.dispose());

let store: Awaited<ReturnType<typeof D1Store.open>>;

beforeEach(async () => {
  store = await D1Store.open(await mf.getD1Database("DB"));
  await store.clear();
});

describe("D1Store Synalog", () => {
  // @lat: [[tests#D1#Synalog runs on D1]]
  it.skipIf(!built)("runs recursion and negation over declared tables in D1", async () => {
    await store.update(`PREFIX ex: <http://example.org/>
      INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy .
                    ex:ada a ex:Person . ex:bob a ex:Person . ex:cy a ex:Person . ex:ada ex:age 42 }`);
    const program = `# @table parent <http://example.org/parent>
      # @class person <http://example.org/Person>
      @Recursive(Ancestor, 5);
      @OrderBy(Ancestor, "x", "y");
      Ancestor(x:, y:) distinct :- parent(subject: x, object: y);
      Ancestor(x:, y:) distinct :- Ancestor(x:, y: m), parent(subject: m, object: y);
      Childless(p:) distinct :- person(subject: p), ~parent(subject: p);
      Old(p:, age:) :- triples(subject: p, predicate: "http://example.org/age", object: age), age > 18;`;
    const anc = await store.synalog(program, "Ancestor");
    assert.deepStrictEqual(anc.columns, ["x", "y"]);
    assert.strictEqual(anc.rows.length, 3);
    const childless = await store.synalog(program, "Childless");
    assert.deepStrictEqual(childless.rows, [["http://example.org/cy"]]);
    const old = await store.synalog(program, "Old");
    assert.deepStrictEqual(old.records, [{ p: "http://example.org/ada", age: 42 }]);
    const page = await store.synalog(program, "Ancestor", { limit: 1, offset: 2 });
    assert.deepStrictEqual(page.rows, [["http://example.org/bob", "http://example.org/cy"]]);
  });
});
