// Checks a vitest JSON report against testsuite/allowlist.toml: every failing test must be an
// allow-listed divergence (`js:<describe…> <test>`), and allow-listed tests that pass are stale.
import { readFileSync } from "node:fs";

const report = JSON.parse(readFileSync(process.argv[2] ?? "test-results.json", "utf8"));
const allow = new Set(
  [...readFileSync(new URL("../../../testsuite/allowlist.toml", import.meta.url), "utf8").matchAll(/^id = "js:(.*)"$/gm)].map((m) => m[1]),
);
const tests = report.testResults.flatMap((f) => f.assertionResults);
let ok = report.numTotalTests > 0 && report.testResults.every((f) => f.status !== "failed" || f.assertionResults.length > 0);
for (const t of tests) {
  const id = [...t.ancestorTitles, t.title].join(" ");
  if (t.status === "failed" && !allow.has(id)) {
    console.error(`FAIL (not allow-listed): ${id}\n  ${t.failureMessages?.[0]?.split("\n")[0] ?? ""}`);
    ok = false;
  } else if (t.status === "failed") {
    console.log(`allow-listed divergence: ${id}`);
  } else if (t.status === "passed" && allow.has(id)) {
    console.error(`stale allow-list entry (test passes): js:${id}`);
    ok = false;
  }
}
console.log(`${tests.filter((t) => t.status === "passed").length}/${tests.length} tests passed`);
process.exit(ok ? 0 : 1);
