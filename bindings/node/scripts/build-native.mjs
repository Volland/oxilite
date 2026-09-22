// Builds the native addon with cargo and copies it to oxilite.node.
import { execFileSync } from "node:child_process";
import { copyFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../..", import.meta.url));
const profile = process.argv.includes("--debug") ? "debug" : "release";
execFileSync("cargo", ["build", "-p", "oxilite-node", ...(profile === "release" ? ["--release"] : [])], { cwd: root, stdio: "inherit" });
const file = { darwin: "liboxilite_node.dylib", win32: "oxilite_node.dll" }[process.platform] ?? "liboxilite_node.so";
copyFileSync(`${root}/target/${profile}/${file}`, fileURLToPath(new URL("../oxilite.node", import.meta.url)));
