// @oxilite/d1 for Node.js (tests, scripts, Miniflare): the WebAssembly core loads itself.
import { createRequire } from "node:module";
import {
  D1Store as Base,
  type D1DatabaseLike,
  type D1StoreOptions,
  type EngineConstructor,
} from "./driver.js";
import type { LevelChange, Versioning } from "@oxilite/common";

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
    options: {
      graphIndex?: boolean;
      jsonld?: boolean | { issuer?: boolean; subject?: boolean; validUntil?: boolean };
      versioning?: Versioning;
      asOfIndex?: boolean;
      stampIndex?: boolean;
    } = {},
  ): string {
    const e = new Engine(
      null,
      JSON.stringify({
        graphIndex: options.graphIndex ?? true,
        versioning: options.versioning ?? "off",
        asOfIndex: options.asOfIndex ?? false,
        stampIndex: options.stampIndex ?? false,
      }),
    );
    if (options.jsonld) return e.jsonldSchemaSql(JSON.stringify(options.jsonld === true ? {} : options.jsonld));
    return e.schemaSql();
  }

  /**
   * The SQL that changes the versioning level of an existing D1 database (for
   * `wrangler d1 migrations`). `stampColumn` / `history` describe a database lowered before.
   */
  static levelChangeSql(
    from: Versioning,
    to: Versioning,
    options: LevelChange & { stampColumn?: boolean; history?: "none" | "frozen" } = {},
  ): string {
    const { stampColumn, history, ...change } = options;
    const e = new Engine(null, null);
    return e.levelChangeSql(from, to, JSON.stringify(change), JSON.stringify({ stampColumn, history }));
  }
}
