#!/usr/bin/env node
// Usage: npx oxilite-d1 schema [--no-graph-index] > migrations/0001_oxilite.sql
import { D1Store } from "../dist/node.js";

const [command, ...args] = process.argv.slice(2);
if (command === "schema") {
  process.stdout.write(D1Store.schemaSql({ graphIndex: !args.includes("--no-graph-index") }));
} else {
  console.error("usage: oxilite-d1 schema [--no-graph-index]");
  process.exit(1);
}
