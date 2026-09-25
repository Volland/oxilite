// @oxilite/d1 for Cloudflare Workers (and other WebAssembly hosts).
//
// In a Worker, import the wasm module and pass it once:
//   import wasm from "@oxilite/d1/oxilite.wasm";
//   const store = await D1Store.open(env.DB, { wasm });
import init, { Engine, initSync } from "../wasm/web/oxilite_wasm.js";
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

let ready = false;

function engineOptions(o: { graphIndex?: boolean; versioning?: Versioning; asOfIndex?: boolean; stampIndex?: boolean }): string {
  return JSON.stringify({
    graphIndex: o.graphIndex ?? true,
    versioning: o.versioning ?? "off",
    asOfIndex: o.asOfIndex ?? false,
    stampIndex: o.stampIndex ?? false,
  });
}

/** Initializes the WebAssembly core (a `WebAssembly.Module`, bytes, or a URL/Response). */
export async function initOxilite(wasm?: WebAssembly.Module | BufferSource | Response | URL | string): Promise<void> {
  if (ready) return;
  if (wasm instanceof WebAssembly.Module || ArrayBuffer.isView(wasm) || wasm instanceof ArrayBuffer) {
    initSync({ module: wasm as WebAssembly.Module | BufferSource });
  } else {
    await init(wasm === undefined ? undefined : { module_or_path: wasm });
  }
  ready = true;
}

export class D1Store {
  /** Opens (and, unless `migrated`, creates) the oxilite schema on a D1 binding. */
  static async open(
    db: D1DatabaseLike,
    options: D1StoreOptions & { wasm?: WebAssembly.Module | BufferSource | Response | URL | string } = {},
  ): Promise<Base> {
    await initOxilite(options.wasm);
    return Base.openWith(Engine as unknown as EngineConstructor, db, options);
  }

  /**
   * The schema as SQL (for `wrangler d1 migrations`); `jsonld` adds the JSON-LD document
   * tables (`true`, or the metadata indexes to create).
   */
  static async schemaSql(
    options: {
      graphIndex?: boolean;
      jsonld?: boolean | { issuer?: boolean; subject?: boolean; validUntil?: boolean };
      versioning?: Versioning;
      asOfIndex?: boolean;
      stampIndex?: boolean;
      wasm?: WebAssembly.Module | BufferSource;
    } = {},
  ): Promise<string> {
    await initOxilite(options.wasm);
    const e = new (Engine as unknown as EngineConstructor)(null, engineOptions(options));
    if (options.jsonld) return e.jsonldSchemaSql(JSON.stringify(options.jsonld === true ? {} : options.jsonld));
    return e.schemaSql();
  }

  /**
   * The SQL that changes the versioning level of an existing D1 database (for
   * `wrangler d1 migrations`). `state` describes the database when it was lowered before.
   */
  static async levelChangeSql(
    from: Versioning,
    to: Versioning,
    options: LevelChange & { stampColumn?: boolean; history?: "none" | "frozen"; wasm?: WebAssembly.Module | BufferSource } = {},
  ): Promise<string> {
    await initOxilite(options.wasm);
    const { stampColumn, history, wasm: _wasm, ...change } = options;
    const e = new (Engine as unknown as EngineConstructor)(null, null);
    return e.levelChangeSql(from, to, JSON.stringify(change), JSON.stringify({ stampColumn, history }));
  }
}
