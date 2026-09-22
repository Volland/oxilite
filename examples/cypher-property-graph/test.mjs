// Runs the property-graph walkthrough on @oxilite/node and on a local D1 (Miniflare).
import { Miniflare } from "miniflare";
import { Store } from "@oxilite/node";
import { D1Store } from "@oxilite/d1/node";
import { demo, fromRdf } from "./graph.mjs";

for (const run of [demo, fromRdf]) await run(new Store());
console.log("@oxilite/node: ok");

const mf = new Miniflare({ modules: true, script: "export default {}", d1Databases: ["DB"] });
const db = await mf.getD1Database("DB");
for (const run of [demo, fromRdf]) {
  const store = await D1Store.open(db);
  await store.clear();
  // D1Store.size is a method, not a getter.
  await run(Object.assign(Object.create(store), { get size() { return store.size(); } }));
}
console.log("@oxilite/d1 on Miniflare D1: ok");
await mf.dispose();
