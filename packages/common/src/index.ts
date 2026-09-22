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
