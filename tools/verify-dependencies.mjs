import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const npmCli = [
  process.env.npm_execpath,
  path.join(path.dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js"),
  path.resolve(path.dirname(process.execPath), "../lib/node_modules/npm/bin/npm-cli.js"),
].find((candidate) => candidate !== undefined && existsSync(candidate));
if (npmCli === undefined) throw new Error("could not locate npm-cli.js for the active Node runtime");
const manifest = JSON.parse(readFileSync(path.join(import.meta.dirname, "manifest.json"), "utf8"));
const releaseNames = new Set(manifest.packages.map(({ name }) => name));
const required = new Map();
for (const item of manifest.packages) {
  for (const [name, version] of Object.entries(item.dependencies)) {
    if (name.startsWith("@aexhq/") && !releaseNames.has(name)) required.set(name, version);
  }
}
const tag = process.argv[2] ?? "next";
for (const [name, version] of required) {
  const actual = JSON.parse(
    execFileSync(process.execPath, [npmCli, "view", `${name}@${tag}`, "version", "--json"], {
      encoding: "utf8",
    }),
  );
  if (actual !== version) {
    throw new Error(`${name}@${tag} is ${actual}; Aex requires the staged ${name}@${version}`);
  }
}
