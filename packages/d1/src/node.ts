// @oxilite/d1 for Node.js (tests, scripts, Miniflare): the WebAssembly core loads itself.
import { createRequire } from "node:module";
import { D1Store as Base, type D1DatabaseLike, type D1StoreOptions, type EngineConstructor } from "./driver.js";

export * from "@oxilite/common";
export {
  OxiliteCollisionError,
  D1Credentials,
  D1JsonLdDocuments,
  type D1DatabaseLike,
  type D1StoreOptions,
} from "./driver.js";

const require = createRequire(import.meta.url);
const { Engine } = require("../wasm/node/oxilite_wasm.js") as { Engine: EngineConstructor };

export class D1Store {
  static async open(db: D1DatabaseLike, options: D1StoreOptions = {}): Promise<Base> {
    return Base.openWith(Engine, db, options);
  }

  /** The schema as SQL; `jsonld` adds the JSON-LD document tables. */
  static schemaSql(
    options: { graphIndex?: boolean; jsonld?: boolean | { issuer?: boolean; subject?: boolean; validUntil?: boolean } } = {},
  ): string {
    const e = new Engine(null, JSON.stringify({ graphIndex: options.graphIndex ?? true }));
    if (options.jsonld) return e.jsonldSchemaSql(JSON.stringify(options.jsonld === true ? {} : options.jsonld));
    return e.schemaSql();
  }
}
