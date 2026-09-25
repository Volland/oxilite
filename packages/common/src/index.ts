// RDF/JS-compatible terms and conversions shared by @oxilite/node and @oxilite/d1.
//
// The JSON forms exchanged with the oxilite core are defined in
// crates/oxilite-core/src/json.rs.

/** Any RDF/JS term (from any library: @rdfjs/data-model, N3.js, oxigraph, …). */
export interface TermLike {
  termType: string;
  value: string;
  language?: string;
  direction?: string;
  datatype?: TermLike;
  subject?: TermLike;
  predicate?: TermLike;
  object?: TermLike;
  graph?: TermLike;
}

abstract class BaseTerm {
  abstract readonly termType: string;
  abstract readonly value: string;
  equals(other: TermLike | null | undefined): boolean {
    return other != null && termEquals(this as unknown as TermLike, other);
  }
}

export class NamedNode extends BaseTerm {
  readonly termType = "NamedNode" as const;
  constructor(readonly value: string) {
    super();
  }
  toString(): string {
    return `<${this.value}>`;
  }
}

export class BlankNode extends BaseTerm {
  readonly termType = "BlankNode" as const;
  constructor(readonly value: string) {
    super();
  }
  toString(): string {
    return `_:${this.value}`;
  }
}

const XSD_STRING = "http://www.w3.org/2001/XMLSchema#string";
const RDF_LANG_STRING = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const RDF_DIR_LANG_STRING = "http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString";

function escape(s: string): string {
  return s.replace(/[\\"\n\r]/g, (c) => ({ "\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r" })[c] as string);
}

export class Literal extends BaseTerm {
  readonly termType = "Literal" as const;
  readonly datatype: NamedNode;
  constructor(
    readonly value: string,
    readonly language: string = "",
    datatype?: NamedNode,
    readonly direction: "" | "ltr" | "rtl" = "",
  ) {
    super();
    this.datatype =
      datatype ?? new NamedNode(language ? (direction ? RDF_DIR_LANG_STRING : RDF_LANG_STRING) : XSD_STRING);
  }
  toString(): string {
    const v = `"${escape(this.value)}"`;
    if (this.language) return `${v}@${this.language}${this.direction ? `--${this.direction}` : ""}`;
    if (this.datatype.value === XSD_STRING) return v;
    return `${v}^^${this.datatype.toString()}`;
  }
}

export class DefaultGraph extends BaseTerm {
  readonly termType = "DefaultGraph" as const;
  readonly value = "" as const;
  toString(): string {
    return "DEFAULT";
  }
}

export class Variable extends BaseTerm {
  readonly termType = "Variable" as const;
  constructor(readonly value: string) {
    super();
  }
  toString(): string {
    return `?${this.value}`;
  }
}

export type Subject = NamedNode | BlankNode | Quad | Variable;
export type Object_ = NamedNode | BlankNode | Literal | Quad | Variable;
export type Graph = NamedNode | BlankNode | DefaultGraph | Variable;
export type Term = NamedNode | BlankNode | Literal | DefaultGraph | Variable | Quad;

export class Quad extends BaseTerm {
  readonly termType = "Quad" as const;
  readonly value = "" as const;
  constructor(
    readonly subject: Subject,
    readonly predicate: NamedNode | Variable,
    readonly object: Object_,
    readonly graph: Graph = new DefaultGraph(),
  ) {
    super();
  }
  toString(): string {
    const inner = (t: Term) => (t instanceof Quad ? `<<( ${t.subject} ${t.predicate} ${t.object} )>>` : t.toString());
    const g = this.graph.termType === "DefaultGraph" ? "" : ` ${this.graph}`;
    return `${inner(this.subject)} ${this.predicate} ${inner(this.object)}${g}`;
  }
}

function termEquals(a: TermLike, b: TermLike): boolean {
  if (a.termType !== b.termType || a.value !== b.value) return false;
  if (a.termType === "Literal") {
    return (
      (a.language ?? "") === (b.language ?? "") &&
      (a.direction ?? "") === (b.direction ?? "") &&
      (a.datatype?.value ?? XSD_STRING) === (b.datatype?.value ?? XSD_STRING)
    );
  }
  if (a.termType === "Quad") {
    const parts = ["subject", "predicate", "object", "graph"] as const;
    return parts.every((p) => {
      const x = a[p];
      const y = b[p];
      if (!x || !y) return (x?.termType ?? "DefaultGraph") === (y?.termType ?? "DefaultGraph");
      return termEquals(x, y);
    });
  }
  return true;
}

/** RDF/JS DataFactory. */
export const DataFactory = {
  namedNode: (value: string) => new NamedNode(value),
  blankNode: (value?: string) => new BlankNode(value ?? `b${Math.random().toString(36).slice(2)}`),
  literal: (value: string, languageOrDatatype?: string | TermLike) => {
    if (typeof languageOrDatatype === "string") {
      const [lang, dir] = languageOrDatatype.split("--");
      return new Literal(value, lang, undefined, (dir as "ltr" | "rtl" | undefined) ?? "");
    }
    if (languageOrDatatype) return new Literal(value, "", new NamedNode(languageOrDatatype.value));
    return new Literal(value);
  },
  defaultGraph: () => new DefaultGraph(),
  variable: (value: string) => new Variable(value),
  quad: (subject: Subject, predicate: NamedNode | Variable, object: Object_, graph?: Graph) =>
    new Quad(subject, predicate, object, graph ?? new DefaultGraph()),
  triple: (subject: Subject, predicate: NamedNode | Variable, object: Object_) =>
    new Quad(subject, predicate, object, new DefaultGraph()),
  fromTerm: (t: TermLike) => fromJson(toJson(t)),
  fromQuad: (q: TermLike) => fromJson(toJson(q)) as Quad,
};

export const namedNode = DataFactory.namedNode;
export const blankNode = DataFactory.blankNode;
export const literal = DataFactory.literal;
export const defaultGraph = DataFactory.defaultGraph;
export const variable = DataFactory.variable;
export const quad = DataFactory.quad;
export const triple = DataFactory.triple;

/** JSON term as produced by the oxilite core. */
export type TermJson = {
  termType: string;
  value: string;
  language?: string;
  direction?: string;
  datatype?: TermJson;
  subject?: TermJson;
  predicate?: TermJson;
  object?: TermJson;
  graph?: TermJson;
};

/** Converts any RDF/JS term to the core's JSON form. */
export function toJson(t: TermLike): TermJson {
  switch (t.termType) {
    case "Literal":
      return {
        termType: "Literal",
        value: t.value,
        language: t.language ?? "",
        direction: t.direction ?? "",
        datatype: { termType: "NamedNode", value: t.datatype?.value ?? XSD_STRING },
      };
    case "Quad":
      return {
        termType: "Quad",
        value: "",
        subject: toJson(t.subject as TermLike),
        predicate: toJson(t.predicate as TermLike),
        object: toJson(t.object as TermLike),
        graph: t.graph ? toJson(t.graph) : { termType: "DefaultGraph", value: "" },
      };
    default:
      return { termType: t.termType, value: t.value };
  }
}

/** Builds a term from the core's JSON form. */
export function fromJson(j: TermJson): Term {
  switch (j.termType) {
    case "NamedNode":
      return new NamedNode(j.value);
    case "BlankNode":
      return new BlankNode(j.value);
    case "Literal":
      return new Literal(
        j.value,
        j.language ?? "",
        j.datatype ? new NamedNode(j.datatype.value) : undefined,
        (j.direction as "" | "ltr" | "rtl" | undefined) ?? "",
      );
    case "DefaultGraph":
      return new DefaultGraph();
    case "Quad":
      return new Quad(
        fromJson(j.subject as TermJson) as Subject,
        fromJson(j.predicate as TermJson) as NamedNode,
        fromJson(j.object as TermJson) as Object_,
        j.graph ? (fromJson(j.graph) as Graph) : new DefaultGraph(),
      );
    default:
      throw new Error(`Unsupported termType ${j.termType}`);
  }
}

/** Output of the core (see `oxilite_core::json::output_to_json`). */
export type Output =
  | { kind: "solutions"; variables: string[]; rows: (TermJson | null)[][] }
  | { kind: "boolean"; value: boolean }
  | { kind: "quads"; quads: TermJson[] }
  | { kind: "number"; value: number }
  | { kind: "text"; value: string }
  | { kind: "ok" };

/** SELECT solutions (like Oxigraph's JS API: one Map per solution), ASK boolean, or quads. */
export type QueryResult = Map<string, Term>[] | boolean | Quad[] | string;

export function outputToResult(out: Output): QueryResult {
  switch (out.kind) {
    case "solutions":
      return out.rows.map((row) => {
        const m = new Map<string, Term>();
        row.forEach((t, i) => {
          if (t) m.set(out.variables[i] as string, fromJson(t));
        });
        return m;
      });
    case "boolean":
      return out.value;
    case "quads":
      return out.quads.map((q) => fromJson(q) as Quad);
    case "text":
      return out.value;
    default:
      throw new Error(`Unexpected output ${out.kind}`);
  }
}

/** Query options accepted by both packages (Oxigraph's JS option names). */
export interface QueryOptions {
  base_iri?: string;
  use_default_graph_as_union?: boolean;
  default_graph?: TermLike | TermLike[];
  named_graphs?: TermLike[];
  results_format?: string;
  /** Query-time entailment (oxilite extension): `"none"` (default), `"rdfs"` or `"owl-ql"`. */
  reasoning?: "none" | "rdfs" | "owl-ql";
  /** Also match inferences stored by `materialize()` (oxilite extension). */
  include_inferred?: boolean;
  /**
   * Read the store as it was at this version (oxilite extension, versioning `log`):
   * `"HEAD~2"`, `"#42"` (a tick) or `"@2026-09-01T12:00:00Z"`.
   */
  as_of?: string;
}

/** Load options (Oxigraph's JS option names). */
export interface LoadOptions {
  format: string;
  base_iri?: string;
  to_graph_name?: TermLike;
  unchecked?: boolean;
  no_transaction?: boolean;
}

/** Dump options (Oxigraph's JS option names). */
export interface DumpOptions {
  format: string;
  from_graph_name?: TermLike;
}

/** Content of `load()`: a string, bytes, or a list of them (like Oxigraph's JS API). */
export type LoadData = string | Uint8Array | (string | Uint8Array)[];

export function loadDataToString(data: LoadData): string {
  const one = (d: string | Uint8Array) => (typeof d === "string" ? d : new TextDecoder().decode(d));
  return Array.isArray(data) ? data.map(one).join("\n") : one(data);
}

// ----- Cypher (property-graph view) -----

/** A node of the property-graph view of the dataset. */
export interface CypherNode {
  type: "node";
  /** The node's IRI (or `_:label` for a blank node). */
  id: string;
  labels: string[];
  properties: Record<string, CypherValue>;
}

/** A relationship: an RDF triple, identified by its reifier when it has one. */
export interface CypherRelationship {
  type: "relationship";
  id: string;
  relType: string;
  start: string;
  end: string;
  properties: Record<string, CypherValue>;
}

export interface CypherPath {
  type: "path";
  nodes: CypherNode[];
  relationships: CypherRelationship[];
}

/** A temporal value in its ISO 8601 form (`value`). */
export interface CypherTemporal {
  type: "date" | "datetime" | "localdatetime" | "time" | "localtime" | "duration";
  value: string;
}

export type CypherValue =
  | null
  | boolean
  | number
  | string
  | CypherValue[]
  | CypherNode
  | CypherRelationship
  | CypherPath
  | CypherTemporal
  | { [key: string]: CypherValue };

/** What a statement changed. */
export interface CypherStats {
  nodesCreated: number;
  nodesDeleted: number;
  relationshipsCreated: number;
  relationshipsDeleted: number;
  propertiesSet: number;
  labelsAdded: number;
  labelsRemoved: number;
}

export interface CypherResult {
  columns: string[];
  rows: CypherValue[][];
  /** Rows as objects keyed by column. */
  records: Record<string, CypherValue>[];
  stats: CypherStats;
}

/** Options of the Datalog frontend (see `oxilite_datalog::json`). */
export interface DatalogOptions {
  /** Match every graph rather than only the default graph. */
  useDefaultGraphAsUnion?: boolean;
  /** Also match materialized inferences. */
  includeInferred?: boolean;
  /** Rounds a component evaluated by iteration may take (default 100). */
  maxIterations?: number;
  /** Run the program on this version of the store (like an `@version` directive). */
  asOf?: string;
}

/** What the native side returns for a Datalog program. */
export interface DatalogOutput {
  kind: "datalog";
  columns: string[];
  rows: (TermJson | null)[][];
  /** Rounds each iterated component took; empty when nothing had to be iterated. */
  rounds: number[];
}

/** The solutions of a Datalog program, as RDF/JS terms. */
export interface DatalogResult {
  columns: string[];
  rows: (Term | null)[][];
  /** Rows as objects keyed by the goal's variables. */
  records: Record<string, Term | null>[];
  rounds: number[];
}

/** What a materialization did. */
export interface DatalogMaterializeResult {
  kind: "datalogMaterialize";
  /** Triples in the inference table afterwards. */
  inferred: number;
  /** Relations whose conclusions were stored. */
  relations: number;
}

export function datalogResult(out: DatalogOutput): DatalogResult {
  const rows = out.rows.map((row) => row.map((c) => (c === null ? null : fromJson(c))));
  return {
    columns: out.columns,
    rows,
    records: rows.map((row) => Object.fromEntries(out.columns.map((c, i) => [c, row[i] ?? null]))),
    rounds: out.rounds,
  };
}

/** Options of the Cypher frontend (see `oxilite_cypher::json`). */
export interface CypherOptions {
  /** Namespace of labels, relationship types and keys without a prefix (default `urn:oxilite:pg:`). */
  base?: string;
  /** Prefixes usable in names: `` :`schema:Person` ``. */
  prefixes?: Record<string, string>;
  /** Explicit name → IRI mappings. */
  names?: Record<string, string>;
  /** How a property with several RDF values reads: a list (default), its first value, or an error. */
  multiValue?: "list" | "first" | "error";
  /** Maximum hops of an unbounded variable-length relationship that binds a variable. */
  varLengthCap?: number;
  /** Maximum depth of an unbounded shortest-path search. */
  shortestPathCap?: number;
  /** Check writes against the SHACL shapes of the dataset (default true). */
  shapes?: boolean;
  /**
   * Match the store as it was at this version (versioning `log`): `"HEAD~1"`, `"#42"` or
   * `"@2026-09-01T12:00:00Z"`. Writing statements are refused with a version.
   */
  asOf?: string;
  /** Give created nodes an `rdf:type rdfs:Resource` triple (default true). */
  nodeMarker?: boolean;
  /** Entailment for matching: `"rdfs"` or `"owl-ql"` make labels follow class hierarchies. */
  reasoning?: "none" | "rdfs" | "owl-ql";
  useDefaultGraphAsUnion?: boolean;
}

/** Output of a Cypher job (`kind: "cypher"`). */
export interface CypherOutput {
  kind: "cypher";
  columns: string[];
  rows: CypherValue[][];
  stats: CypherStats;
}

export function cypherResult(out: CypherOutput): CypherResult {
  return {
    columns: out.columns,
    rows: out.rows,
    records: out.rows.map((row) => Object.fromEntries(out.columns.map((c, i) => [c, row[i] ?? null]))),
    stats: out.stats,
  };
}

// ---------------------------------------------------------------------------------------------
// JSON-LD documents and Verifiable Credentials (see `oxilite_jsonld::json`).

/** Where a document's key comes from. */
export type KeyStrategy = "id" | "contentHash" | "explicit" | { pointer: string };

/** Which graph a document's triples go to: the key itself (default), an IRI template with `{key}`, one fixed graph, or the default graph. */
export type GraphStrategy = "key" | "default" | { template: string } | { fixed: string };

/** Options of a JSON-LD document handle. */
export interface JsonLdOptions {
  /** Default `"id"`: the top-level `@id` / `id`. */
  key?: KeyStrategy;
  /** When the key strategy finds nothing: fail (default for documents) or use `urn:oxilite:doc:sha256:<hex>` (default for credentials). */
  onMissingKey?: "reject" | "contentHash";
  /** Default `"key"`: one named graph per document, named after its key. */
  graph?: GraphStrategy;
  /** Base IRI for relative IRIs in documents. */
  baseIri?: string;
  rdfDirection?: "i18n-datatype" | "compound-literal";
  processingMode?: "json-ld-1.0" | "json-ld-1.1";
  /** Contexts available in memory: IRI → context document. */
  contexts?: Record<string, object | string>;
  /** Fetch unknown remote contexts over HTTP (Node only; default false: no network access). */
  network?: boolean;
  /** Persist fetched contexts in the store, so later loads work offline. */
  cacheFetched?: boolean;
  /** Metadata indexes created with the tables (all default true). */
  indexes?: { issuer?: boolean; subject?: boolean; validUntil?: boolean };
}

/** Options of a credentials handle. */
export interface CredentialOptions extends JsonLdOptions {
  /** Also store each credential embedded in a presentation on its own (default true). */
  embedCredentials?: boolean;
}

/** A stored document (or credential). */
export interface StoredDocument {
  key: string;
  /** The graph its default-graph triples were written to. */
  graph: Term;
  /** The JSON exactly as it was stored. */
  json: string;
  /** Hex SHA-256 of `json`. */
  sha256: string;
  /** `jsonld`, or `vc1` / `vc2` / `vp1` / `vp2` for credentials and presentations. */
  profile: string;
  issuer: string | null;
  subject: string | null;
  types: string[];
  validFrom: Date | null;
  validUntil: Date | null;
  /** Keys of the credentials a presentation embeds. */
  refs: string[];
  storedAt: Date;
}

/** Metadata filter of `find`. */
export interface DocumentFilter {
  issuer?: string;
  subject?: string;
  /** One value of the `type` array. */
  type?: string;
  /** Valid at this instant (validFrom ≤ t < validUntil; open ends allowed). */
  validAt?: Date | number;
  profile?: string;
  /** Keyset paging: keys after this one. */
  after?: string;
  limit?: number;
}

/** A document whose graphs differ from a fresh conversion of its JSON. */
export interface Drift {
  key: string;
  missing: number;
  extra: number;
}

/** Keys written by `putPresentation`. */
export interface PresentationKeys {
  key: string;
  credentials: string[];
}

/** A JSON-LD or credential error; `code` is the JSON-LD error code (e.g. `invalid local context`) or `missing-key`, `invalid-graph-name`, `graph-owned`, `document-too-large`, `invalid`, `json`, `store`. */
export class JsonLdError extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(message);
    this.name = "JsonLdError";
    this.code = code;
  }
}

/** Rethrows an error of the core's JSON-LD operations as a `JsonLdError`. */
export function jsonLdError(e: unknown): unknown {
  const message = e instanceof Error ? e.message : String(e);
  const at = message.indexOf("oxilite-jsonld:");
  if (at < 0) return e;
  try {
    const { code, message: m } = JSON.parse(message.slice(at + "oxilite-jsonld:".length)) as { code: string; message: string };
    return new JsonLdError(code, m);
  } catch {
    return e;
  }
}

/** A stored document from the core's JSON (epoch seconds become `Date`s). */
export function storedDocument(j: Record<string, unknown> | null): StoredDocument | null {
  if (!j) return null;
  const date = (v: unknown) => (typeof v === "number" ? new Date(v * 1000) : null);
  return {
    key: j.key as string,
    graph: fromJson(j.graph as TermJson),
    json: j.json as string,
    sha256: j.sha256 as string,
    profile: j.profile as string,
    issuer: (j.issuer as string | null) ?? null,
    subject: (j.subject as string | null) ?? null,
    types: (j.types as string[]) ?? [],
    validFrom: date(j.validFrom),
    validUntil: date(j.validUntil),
    refs: (j.refs as string[]) ?? [],
    storedAt: date(j.storedAt) as Date,
  };
}

/** A filter in the core's JSON form. */
export function filterJson(f: DocumentFilter): Record<string, unknown> {
  const t = f.validAt;
  return { ...f, validAt: t === undefined ? undefined : (t instanceof Date ? t.getTime() : t) / 1000 };
}

/** A document argument: JSON text is stored verbatim, objects are serialized. */
export function documentText(doc: string | object): string {
  return typeof doc === "string" ? doc : JSON.stringify(doc);
}

// ------------------------------------------------------------------------------ versioning

/** How much history a store keeps. */
export type Versioning = "off" | "stamped" | "log";

/** The versioning level of a store and where its clock and history stand. */
export interface VersionStatus {
  level: Versioning;
  history: "none" | "live" | "frozen";
  stampColumn: boolean;
  stampIndex: boolean;
  asOfIndex: boolean;
  /** The latest tick. */
  head: number | null;
  /** Wall time of the latest tick, in seconds since the epoch. */
  headTime: number | null;
  /** Where the recorded history (re)starts. */
  genesis: number | null;
  /** The latest freeze, while the history is frozen. */
  frozenAt: number | null;
  commits: number | null;
}

/** One entry of the history: a commit or a level change. */
export interface CommitRecord {
  tick: number;
  /** Seconds since the epoch. */
  time: number;
  kind: "write" | "genesis" | "freeze" | "resume" | "dropped" | "level" | "purge";
  author: string | null;
  message: string | null;
  added: number | null;
  removed: number | null;
}

/** A quad added or removed at a tick. */
export interface Change {
  tick: number;
  added: boolean;
  quad: Quad;
}

/** Options of a level change. */
export interface LevelChange {
  asOfIndex?: boolean;
  stampIndex?: boolean;
  /** Allow a downgrade to delete the history, the ticks and the stamp column. */
  allowLoss?: boolean;
  author?: string;
  message?: string;
}

/** Author and message recorded on the commits of later writes. */
export interface CommitInfo {
  author?: string;
  message?: string;
}

/** Changes from their JSON form (`{tick, added, quad}`). */
export function toChanges(v: unknown): Change[] {
  return (v as { tick: number; added: boolean; quad: TermJson }[]).map((c) => ({
    tick: c.tick,
    added: c.added,
    quad: fromJson(c.quad) as Quad,
  }));
}
