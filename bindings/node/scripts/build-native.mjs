// Builds the native addon with cargo and copies it to oxilite.node.
import { execFileSync } from "node:child_process";
import { copyFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../..", import.meta.url));
const profile = process.argv.includes("--debug") ? "debug" : "release";
execFileSync("cargo", ["build", "-p", "oxilite-node", ...(profile === "release" ? ["--release"] : [])], { cwd: root, stdio: "inherit" });
const file = { darwin: "liboxilite_node.dylib", win32: "oxilite_node.dll" }[process.platform] ?? "liboxilite_node.so";
const built = `${root}/target/${profile}/${file}`;
copyFileSync(built, fileURLToPath(new URL("../oxilite.node", import.meta.url)));
// The published package carries one prebuilt binary per platform.
copyFileSync(built, fileURLToPath(new URL(`../oxilite.${process.platform}-${process.arch}.node`, import.meta.url)));
