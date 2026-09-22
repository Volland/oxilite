#!/usr/bin/env node
// Usage: npx oxilite-d1 schema [--no-graph-index] [--jsonld] [--no-metadata-indexes] > migrations/0001_oxilite.sql
import { D1Store } from "../dist/node.js";

const [command, ...args] = process.argv.slice(2);
if (command === "schema") {
  const jsonld = args.includes("--jsonld")
    ? args.includes("--no-metadata-indexes")
      ? { issuer: false, subject: false, validUntil: false }
      : true
    : false;
  process.stdout.write(D1Store.schemaSql({ graphIndex: !args.includes("--no-graph-index"), jsonld }));
} else {
  console.error("usage: oxilite-d1 schema [--no-graph-index] [--jsonld] [--no-metadata-indexes]");
  process.exit(1);
}
