// A SPARQL endpoint on Cloudflare D1 in TypeScript, with @oxilite/d1.
//
// GET /sparql?query=… or POST /sparql (body: the query) → SPARQL JSON results;
// POST /update (body: a SPARQL update) → 204; POST /load?format=text/turtle → 204.
import { D1Store, type D1DatabaseLike } from "@oxilite/d1";
import wasm from "@oxilite/d1/oxilite.wasm";

interface Env {
  DB: D1DatabaseLike;
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    // The schema is applied by `wrangler d1 migrations apply` (npx oxilite-d1 schema).
    const store = await D1Store.open(env.DB, { wasm, migrated: true });
    const url = new URL(req.url);
    try {
      switch (`${req.method} ${url.pathname}`) {
        case "GET /sparql":
        case "POST /sparql": {
          const query = req.method === "GET" ? (url.searchParams.get("query") ?? "") : await req.text();
          return new Response(await store.queryJson(query), {
            headers: { "content-type": "application/sparql-results+json" },
          });
        }
        case "POST /update":
          await store.update(await req.text());
          return new Response(null, { status: 204 });
        case "POST /load":
          await store.bulkLoad(await req.text(), { format: url.searchParams.get("format") ?? "text/turtle" });
          return new Response(null, { status: 204 });
        case "GET /explain":
          return new Response(store.explain(url.searchParams.get("query") ?? ""));
        default:
          return new Response("not found", { status: 404 });
      }
    } catch (e) {
      return new Response(e instanceof Error ? e.message : String(e), { status: 400 });
    }
  },
};
