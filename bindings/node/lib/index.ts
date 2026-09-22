// @oxilite/node: an Oxigraph-compatible SPARQL store on SQLite for Node.js.
//
// The API mirrors Oxigraph's JavaScript `Store` (query, update, load, dump, add, delete, has,
// match, size) with RDF/JS terms; the native addon (src/lib.rs) exchanges JSON terms with
// this wrapper, using the same conversions as @oxilite/d1.

import { createRequire } from "node:module";
import {
  type DumpOptions,
  type LoadData,
  type LoadOptions,
  type Output,
  type Quad,
  type QueryOptions,
  type QueryResult,
  type TermLike,
  loadDataToString,
  outputToResult,
  toJson,
} from "@oxilite/common";

export * from "@oxilite/common";

interface NativeStoreInstance {
  query(sparql: string, options?: string | null): string;
  explain(sparql: string): string;
  update(sparql: string, baseIri?: string | null): void;
  explainUpdate(sparql: string): string;
  load(data: string, format: string, baseIri: string | null, toGraph: string | null, bulk: boolean): void;
  add(quads: string): void;
  delete(quads: string): void;
  has(quad: string): boolean;
  match(s: string | null, p: string | null, o: string | null, g: string | null): string;
  size(): number;
  dump(format: string, fromGraph?: string | null): string;
  optimize(): void;
  materialize(reasonable?: boolean | null): number;
  clearInferences(): void;
  clear(): void;
  backup(path: string): void;
}
interface Native {
  NativeStore: new (path?: string | null, library?: string | null, options?: string | null) => NativeStoreInstance;
  schemaSql(options?: string | null): string;
}

const native = createRequire(import.meta.url)("../oxilite.node") as Native;

/** How to open a store. */
export interface StoreOptions {
  /** SQLite database file; in memory when absent. */
  path?: string;
  /** Path of a SQLite shared library to load instead of the bundled SQLite. */
  library?: string;
  /** Create the optional graph index (default true); only used when the schema is created. */
  graphIndex?: boolean;
}

const j = (t?: TermLike | null) => (t ? JSON.stringify(toJson(t)) : null);

/** An RDF dataset stored in SQLite, queryable with SPARQL. */
export class Store {
  private readonly native: NativeStoreInstance;

  /**
   * `new Store()` (in memory), `new Store(quads)` like Oxigraph, `new Store("data.sqlite")`, or
   * `new Store({ path, library, graphIndex })`.
   */
  constructor(init?: Iterable<TermLike> | string | StoreOptions, options: StoreOptions = {}) {
    const opts: StoreOptions =
      typeof init === "string" ? { ...options, path: init } : init && !isIterable(init) ? { ...init } : options;
    this.native = new native.NativeStore(
      opts.path ?? null,
      opts.library ?? null,
      JSON.stringify({ graphIndex: opts.graphIndex ?? true }),
    );
    if (init && typeof init !== "string" && isIterable(init)) this.addAll(init);
  }

  /** Number of quads. */
  get size(): number {
    return this.native.size();
  }

  /** SPARQL query: `Map[]` for SELECT, `boolean` for ASK, `Quad[]` for CONSTRUCT/DESCRIBE, or a string with `results_format`. */
  query(query: string, options: QueryOptions = {}): QueryResult {
    const js = {
      ...options,
      default_graph: options.default_graph
        ? Array.isArray(options.default_graph)
          ? options.default_graph.map(toJson)
          : toJson(options.default_graph)
        : undefined,
      named_graphs: options.named_graphs?.map(toJson),
    };
    return outputToResult(JSON.parse(this.native.query(query, JSON.stringify(js))) as Output);
  }

  /** The SQL a query compiles to, with join orders and warnings. */
  explain(query: string): string {
    return this.native.explain(query);
  }

  /** SPARQL update, applied atomically. */
  update(update: string, options: { base_iri?: string } = {}): void {
    this.native.update(update, options.base_iri ?? null);
  }

  /** How an update runs (SQL per operation). */
  explainUpdate(update: string): string {
    return this.native.explainUpdate(update);
  }

  /** Loads RDF atomically; `no_transaction` loads in chunks and refreshes statistics. */
  load(data: LoadData, options: LoadOptions): void;
  /** Oxigraph's older positional form. */
  load(data: LoadData, format: string, baseIri?: string | null, toGraphName?: TermLike | null): void;
  load(data: LoadData, options: LoadOptions | string, baseIri?: string | null, toGraphName?: TermLike | null): void {
    const o: LoadOptions =
      typeof options === "string"
        ? { format: options, base_iri: baseIri ?? undefined, to_graph_name: toGraphName ?? undefined }
        : options;
    this.native.load(loadDataToString(data), o.format, o.base_iri ?? null, j(o.to_graph_name), o.no_transaction ?? false);
  }

  /** Loads RDF in chunks (not atomic) and refreshes planner statistics. */
  bulkLoad(data: LoadData, options: LoadOptions): void {
    this.load(data, { ...options, no_transaction: true });
  }

  /** Serializes the dataset, or one graph with `from_graph_name`. */
  dump(options: DumpOptions): string;
  dump(format: string, fromGraphName?: TermLike | null): string;
  dump(options: DumpOptions | string, fromGraphName?: TermLike | null): string {
    const o: DumpOptions = typeof options === "string" ? { format: options, from_graph_name: fromGraphName ?? undefined } : options;
    return this.native.dump(o.format, j(o.from_graph_name));
  }

  add(quad: TermLike): void {
    this.native.add(JSON.stringify([toJson(quad)]));
  }

  /** Inserts many quads atomically. */
  addAll(quads: Iterable<TermLike>): void {
    this.native.add(JSON.stringify(Array.from(quads, toJson)));
  }

  delete(quad: TermLike): void {
    this.native.delete(JSON.stringify([toJson(quad)]));
  }

  has(quad: TermLike): boolean {
    return this.native.has(JSON.stringify(toJson(quad)));
  }

  match(subject?: TermLike | null, predicate?: TermLike | null, object?: TermLike | null, graph?: TermLike | null): Quad[] {
    return outputToResult(JSON.parse(this.native.match(j(subject), j(predicate), j(object), j(graph))) as Output) as Quad[];
  }

  /**
   * Computes the OWL 2 RL closure into a separate inference table (replacing earlier
   * inferences); query it with `include_inferred: true`. `engine: "reasonable"` computes it
   * in memory with the `reasonable` reasoner (faster, same results). Returns the number of
   * inferred triples.
   */
  materialize(options: { engine?: "sql" | "reasonable" } = {}): number {
    return this.native.materialize(options.engine === "reasonable");
  }

  /** Removes every materialized inference. */
  clearInferences(): void {
    this.native.clearInferences();
  }

  /** Refreshes planner statistics (run after large imports). */
  optimize(): void {
    this.native.optimize();
  }

  clear(): void {
    this.native.clear();
  }

  /** Writes a consistent copy of the database to `path`. */
  backup(path: string): void {
    this.native.backup(path);
  }

  /** The schema as SQL. */
  static schemaSql(options: { graphIndex?: boolean } = {}): string {
    return native.schemaSql(JSON.stringify({ graphIndex: options.graphIndex ?? true }));
  }
}

function isIterable(x: unknown): x is Iterable<TermLike> {
  return typeof x === "object" && x !== null && Symbol.iterator in x;
}
