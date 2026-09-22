// Agent memory as an RDF graph: one Durable Object per agent, each with its own oxilite store
// on the object's embedded SQLite.
//
//   POST /agents/:id/remember   (body: Turtle)          → 204, facts added atomically
//   POST /agents/:id/sparql     (body: SPARQL query)    → SPARQL JSON results (?reasoning=rdfs|owl-ql)
//   POST /agents/:id/update     (body: SPARQL update)   → 204, one transaction
//   GET  /agents/:id/explain?query=…                    → the SQL the query compiles to
import { DurableObject } from "cloudflare:workers";
import { D1Store } from "@oxilite/d1";
import wasm from "@oxilite/d1/oxilite.wasm";
import { durableObjectDatabase } from "./do-sql";

type Store = Awaited<ReturnType<typeof D1Store.open>>;

interface Env {
  MEMORY: DurableObjectNamespace<GraphMemory>;
}

export class GraphMemory extends DurableObject<Env> {
  private store!: Store;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // Create the schema (idempotent) before the object handles any request.
    ctx.blockConcurrencyWhile(async () => {
      this.store = await D1Store.open(durableObjectDatabase(ctx.storage), { wasm, graphIndex: true });
    });
  }

  /** Adds facts, in Turtle, to one named graph: the provenance of the facts. */
  async remember(turtle: string, source: string): Promise<void> {
    await this.store.load(turtle, { format: "text/turtle", to_graph_name: { termType: "NamedNode", value: source } });
  }

  /** SPARQL JSON results; `reasoning` applies RDFS or OWL QL entailment at query time. */
  async query(sparql: string, reasoning?: "rdfs" | "owl-ql"): Promise<string> {
    if (!reasoning) return this.store.queryJson(sparql);
    return (await this.store.query(sparql, { reasoning, results_format: "application/sparql-results+json" })) as string;
  }

  async update(sparql: string): Promise<void> {
    await this.store.update(sparql);
  }

  explain(sparql: string): string {
    return this.store.explain(sparql);
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const m = url.pathname.match(/^\/agents\/([\w-]+)\/(remember|sparql|update|explain)$/);
    if (!m) return new Response("not found", { status: 404 });
    const [, agent, action] = m;
    // The same name always reaches the same object, wherever the request arrives.
    const memory = env.MEMORY.get(env.MEMORY.idFromName(agent));
    try {
      switch (action) {
        case "remember":
          await memory.remember(await req.text(), url.searchParams.get("source") ?? `urn:agent:${agent}:inbox`);
          return new Response(null, { status: 204 });
        case "sparql": {
          const reasoning = url.searchParams.get("reasoning");
          if (reasoning && reasoning !== "rdfs" && reasoning !== "owl-ql") throw new Error("reasoning must be rdfs or owl-ql");
          return new Response(await memory.query(await req.text(), reasoning ?? undefined), {
            headers: { "content-type": "application/sparql-results+json" },
          });
        }
        case "update":
          await memory.update(await req.text());
          return new Response(null, { status: 204 });
        default:
          return new Response(await memory.explain(url.searchParams.get("query") ?? ""));
      }
    } catch (e) {
      return new Response(e instanceof Error ? e.message : String(e), { status: 400 });
    }
  },
};
