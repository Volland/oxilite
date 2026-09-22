// @oxilite/node: an Oxigraph-compatible SPARQL store on SQLite for Node.js.
//
// The API mirrors Oxigraph's JavaScript `Store` (query, update, load, dump, add, delete, has,
// match, size) with RDF/JS terms; the native addon (src/lib.rs) exchanges JSON terms with
// this wrapper, using the same conversions as @oxilite/d1.

import { createRequire } from "node:module";
import {
  type CypherOptions,
  type CypherOutput,
  type CypherResult,
  type CypherValue,
  type CredentialOptions,
  type DocumentFilter,
  type Drift,
  type JsonLdOptions,
  type PresentationKeys,
  type StoredDocument,
  type Term,
  cypherResult,
  documentText,
  filterJson,
  fromJson,
  jsonLdError,
  storedDocument,
  type DumpOptions,
  type LoadData,
  type LoadOptions,
  type Output,
  type Quad,
  type QueryOptions,
  type QueryResult,
  type TermJson,
  type TermLike,
  loadDataToString,
  outputToResult,
  toJson,
} from "@oxilite/common";

export * from "@oxilite/common";

interface NativeStoreInstance {
  query(sparql: string, options?: string | null): string;
  explain(sparql: string): string;
  cypher(query: string, params?: string | null, options?: string | null): string;
  explainCypher(query: string, params?: string | null, options?: string | null): string;
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
  jsonld(op: string, args: string, options?: string | null): string;
}
interface Native {
  NativeStore: new (path?: string | null, library?: string | null, options?: string | null) => NativeStoreInstance;
  schemaSql(options?: string | null): string;
}

/** The prebuilt addon for this platform (`oxilite.<platform>-<arch>.node`), or a local build. */
function loadNative(): Native {
  const require = createRequire(import.meta.url);
  const target = `${process.platform}-${process.arch}`;
  for (const file of [`../oxilite.${target}.node`, "../oxilite.node"]) {
    try {
      return require(file) as Native;
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code !== "MODULE_NOT_FOUND") throw e;
    }
  }
  throw new Error(
    `@oxilite/node has no prebuilt binary for ${target}; build it from source with \`npm run build:native\` in a checkout of https://github.com/Volland/oxilite`,
  );
}

const native = loadNative();

/** How to open a store. */
export interface StoreOptions {
  /** SQLite database file; in memory when absent. */
  path?: string;
  /** Path of a SQLite shared library to load instead of the bundled SQLite. */
  library?: string;
  /** Create the optional graph index (default true); only used when the schema is created. */
  graphIndex?: boolean;
  /** Create the FTS5 full-text index over string literals (for `oxl:textMatch`). */
  textIndex?: boolean;
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
      JSON.stringify({ graphIndex: opts.graphIndex ?? true, textIndex: opts.textIndex ?? false }),
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

  /**
   * A Cypher statement over the property-graph view of the dataset: nodes are IRIs, labels
   * `rdf:type`, properties literal triples, relationships triples (with properties on an RDF
   * 1.2 reifier). A writing statement is applied atomically.
   */
  cypher(query: string, params: Record<string, CypherValue> = {}, options: CypherOptions = {}): CypherResult {
    const out = JSON.parse(this.native.cypher(query, JSON.stringify(params), JSON.stringify(options))) as CypherOutput;
    return cypherResult(out);
  }

  /** How a Cypher statement runs: its SPARQL, the SQL it compiles to, and what runs in Rust. */
  explainCypher(query: string, params: Record<string, CypherValue> = {}, options: CypherOptions = {}): string {
    return this.native.explainCypher(query, JSON.stringify(params), JSON.stringify(options));
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

  /**
   * JSON-LD documents: each stored verbatim under a key (by default its `@id`), its RDF in a
   * named graph (by default the key) that SPARQL queries like any other graph.
   */
  jsonld(options: JsonLdOptions = {}): JsonLdDocuments {
    return new JsonLdDocuments((op, args) => this.call(op, args, options));
  }

  /**
   * Verifiable Credentials (VCDM 1.1 and 2.0): stored verbatim under their `id`, RDF in the
   * graph of the same IRI, issuer/subject/type/validity indexed. Proofs are not verified.
   */
  credentials(options: CredentialOptions = {}): Credentials {
    return new Credentials((op, args) => this.call(op, args, { ...options, credentials: true }));
  }

  private call(op: string, args: object, options: object): unknown {
    try {
      return JSON.parse(this.native.jsonld(op, JSON.stringify(args), JSON.stringify(options)));
    } catch (e) {
      throw jsonLdError(e);
    }
  }

  /** The schema as SQL. */
  static schemaSql(options: { graphIndex?: boolean } = {}): string {
    return native.schemaSql(JSON.stringify({ graphIndex: options.graphIndex ?? true }));
  }
}

function isIterable(x: unknown): x is Iterable<TermLike> {
  return typeof x === "object" && x !== null && Symbol.iterator in x;
}

type Call = (op: string, args: object) => unknown;

/** JSON-LD documents of a store (see `Store.jsonld`). Every write is atomic. */
export class JsonLdDocuments {
  constructor(private readonly call: Call) {}

  /** Stores a document (replacing one with the same key) and returns its key. JSON text is stored byte for byte. */
  put(document: string | object, key?: string): string {
    return this.putAll([{ document, key }])[0];
  }

  /** Stores several documents in one atomic write; returns their keys. */
  putAll(documents: { document: string | object; key?: string }[]): string[] {
    const out = this.call("put", {
      documents: documents.map((d) => ({ json: documentText(d.document), key: d.key })),
    }) as { keys: string[] };
    return out.keys;
  }

  /** The stored document, or null. */
  get(key: string): StoredDocument | null {
    return storedDocument(this.call("get", { key }) as Record<string, unknown> | null);
  }

  /** Removes a document and the graphs it owns; false when it was not stored. */
  remove(key: string): boolean {
    return this.call("remove", { key }) as boolean;
  }

  /** Documents ordered by key (keyset paging with `after`). */
  list(options: { after?: string; limit?: number } = {}): StoredDocument[] {
    return (this.call("list", options) as Record<string, unknown>[]).map((d) => storedDocument(d) as StoredDocument);
  }

  /** Documents matching metadata filters. */
  find(filter: DocumentFilter = {}): StoredDocument[] {
    return (this.call("find", filterJson(filter)) as Record<string, unknown>[]).map((d) => storedDocument(d) as StoredDocument);
  }

  /** The graphs a document owns (its own graph and the graphs it defines, such as proofs). */
  graphs(key: string): Term[] {
    return (this.call("graphs", { key }) as TermJson[]).map(fromJson);
  }

  /** The document behind a graph, e.g. a `?g` bound by SPARQL. */
  documentForGraph(graph: TermLike): StoredDocument | null {
    return storedDocument(this.call("documentForGraph", { graph: toJson(graph) }) as Record<string, unknown> | null);
  }

  /** Persists a context, so documents that reference `iri` convert offline. */
  putContext(iri: string, context: object | string): void {
    this.call("putContext", { iri, context });
  }

  removeContext(iri: string): void {
    this.call("removeContext", { iri });
  }

  /** IRIs of the persisted contexts. */
  contexts(): string[] {
    return this.call("contexts", {}) as string[];
  }

  /** Regenerates a document's graphs from its stored JSON; false when it is not stored. */
  rebuild(key: string): boolean {
    return this.call("rebuild", { key }) as boolean;
  }

  /** Documents whose graphs no longer match their JSON (e.g. after a SPARQL UPDATE). */
  check(): Drift[] {
    return this.call("check", {}) as Drift[];
  }
}

/** Verifiable Credentials of a store (see `Store.credentials`). */
export class Credentials {
  /** The document operations (graphs, contexts, check, rebuild…) with the bundled credential contexts. */
  readonly documents: JsonLdDocuments;

  constructor(private readonly call: Call) {
    this.documents = new JsonLdDocuments(call);
  }

  /** Checks and stores a credential; returns its key (its `id`, or a content hash without one). */
  put(credential: string | object, key?: string): string {
    return this.call("putCredential", { json: documentText(credential), key }) as string;
  }

  /** Stores a presentation and each credential it embeds, atomically. */
  putPresentation(presentation: string | object): PresentationKeys {
    return this.call("putPresentation", { json: documentText(presentation) }) as PresentationKeys;
  }

  get(key: string): StoredDocument | null {
    return this.documents.get(key);
  }

  remove(key: string): boolean {
    return this.documents.remove(key);
  }

  /** Credentials by issuer, subject, type, validity instant and profile (indexed; no SPARQL). */
  find(filter: DocumentFilter = {}): StoredDocument[] {
    return (this.call("find", filterJson(filter)) as Record<string, unknown>[]).map((d) => storedDocument(d) as StoredDocument);
  }
}
