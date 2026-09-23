import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { registryValue, waitFor } from "./npm-registry.mjs";

const directory = import.meta.dirname;
const npmCli = [
  process.env.npm_execpath,
  path.join(path.dirname(process.execPath), "node_modules/npm/bin/npm-cli.js"),
  path.resolve(path.dirname(process.execPath), "../lib/node_modules/npm/bin/npm-cli.js"),
].find((candidate) => candidate !== undefined && existsSync(candidate));
if (npmCli === undefined) throw new Error("could not locate npm-cli.js for the active Node runtime");
const run = (args, stdio = "pipe") => execFileSync(process.execPath, [npmCli, ...args], {
  cwd: directory, encoding: "utf8", stdio,
})?.trim() ?? "";
const manifest = JSON.parse(readFileSync(path.join(directory, "manifest.json"), "utf8"));
const expected = process.env.EXPECTED_COMMIT ?? "";
if (!/^[0-9a-f]{40}$/u.test(expected) || manifest.source !== expected) {
  throw new Error("the release archive source does not match EXPECTED_COMMIT");
}
for (const item of manifest.packages) {
  const integrity = `sha512-${createHash("sha512").update(readFileSync(path.join(directory, item.filename))).digest("base64")}`;
  if (integrity !== item.integrity) throw new Error(`${item.name}: archive integrity mismatch`);
}
const registryIntegrity = (item) => {
  const spec = `${item.name}@${item.version}`;
  const integrity = registryValue(run, spec, "dist.integrity");
  if (integrity !== undefined && integrity !== item.integrity) {
    throw new Error(`${spec} is immutable and already has a different registry integrity`);
  }
  return integrity;
};

const operation = process.argv[2];
if (operation === "stage") {
  // Check every version before publishing any part of the release.
  const existing = manifest.packages.map(registryIntegrity);
  for (const [index, item] of manifest.packages.entries()) {
    if (existing[index] === undefined) {
      run(["publish", path.join(directory, item.filename), "--access", "public", "--tag", "next"], "inherit");
    }
    console.log(`submitted ${item.name}@${item.version} (${item.integrity})`);
  }
} else if (operation === "verify") {
  for (const item of manifest.packages) {
    await waitFor(() => registryIntegrity(item), item.integrity, `${item.name}@${item.version} integrity`);
    await waitFor(() => registryValue(run, `${item.name}@next`, "version"), item.version, `${item.name}@next`);
    console.log(`verified ${item.name}@${item.version} (${item.integrity})`);
  }
} else {
  throw new Error("usage: publish.mjs stage|verify");
}
