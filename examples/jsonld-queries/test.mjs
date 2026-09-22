// Runs the JSON-LD query walkthrough on @oxilite/node and on a local D1 (Miniflare).
import { Miniflare } from "miniflare";
import { Store } from "@oxilite/node";
import { D1Store } from "@oxilite/d1/node";
import { demo } from "./wallet.mjs";

await demo(new Store({ textIndex: true }));
console.log("@oxilite/node: ok");

const mf = new Miniflare({ modules: true, script: "export default {}", d1Databases: ["DB"] });
await demo(await D1Store.open(await mf.getD1Database("DB"), { textIndex: true }));
console.log("@oxilite/d1 on Miniflare D1: ok");
await mf.dispose();
