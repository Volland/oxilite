// Exposes a local Cloudflare D1 database (Miniflare, the engine behind `wrangler dev`) over
// HTTP for the Rust compatibility harness: POST /execute {statements, mode} runs an oxilite
// request exactly like the @oxilite/d1 driver does (raw() for single reads, batch()
// otherwise); POST /reset drops every table.
import http from "node:http";
import { Miniflare } from "miniflare";

const port = Number(process.env.PORT ?? 8787);
const mf = new Miniflare({ modules: true, script: "export default {}", d1Databases: ["DB"] });
const db = await mf.getD1Database("DB");

async function execute(req) {
  if (req.statements.length === 0) return [];
  if (req.mode === "read" && req.statements.length === 1) {
    return [{ rows: await db.prepare(req.statements[0].sql).raw(), changes: 0 }];
  }
  const results = await db.batch(req.statements.map((s) => db.prepare(s.sql)));
  // rows_written (D1's billing unit, index entries included) feeds the write-cost report.
  return results.map((r) => ({
    rows: (r.results ?? []).map((row) => Object.values(row)),
    changes: r.meta?.changes ?? 0,
    rows_written: r.meta?.rows_written ?? 0,
  }));
}

async function reset() {
  const tables = await db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_cf_%'").raw();
  if (tables.length) await db.batch(tables.map(([t]) => db.prepare(`DROP TABLE IF EXISTS "${t}"`)));
}

http
  .createServer(async (req, res) => {
    let body = "";
    for await (const chunk of req) body += chunk;
    try {
      const out = req.url === "/reset" ? (await reset(), { ok: true }) : await execute(JSON.parse(body));
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(out));
    } catch (e) {
      res.writeHead(500, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: String(e?.message ?? e) }));
    }
  })
  .listen(port, () => console.log(`d1-sidecar listening on http://127.0.0.1:${port}`));
