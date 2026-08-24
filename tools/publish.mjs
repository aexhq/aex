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
const directory = import.meta.dirname;
const manifest = JSON.parse(readFileSync(path.join(directory, "manifest.json"), "utf8"));
if (
  !/^[0-9a-f]{40}$/u.test(process.env.EXPECTED_COMMIT ?? "") ||
  manifest.source !== process.env.EXPECTED_COMMIT
) {
  throw new Error("the release archive source does not match EXPECTED_COMMIT");
}

const run = (args, stdio = "pipe") => {
  const output = execFileSync(process.execPath, [npmCli, ...args], {
    encoding: "utf8",
    stdio,
  });
  return typeof output === "string" ? output.trim() : "";
};
const registryValue = (spec, field) => {
  try {
    return JSON.parse(run(["view", spec, field, "--json"]));
  } catch {
    return undefined;
  }
};
const waitFor = async (read, expected, description) => {
  for (let attempt = 0; attempt < 12; attempt += 1) {
    if (read() === expected) return;
    await new Promise((resolve) => setTimeout(resolve, 5_000));
  }
  throw new Error(`${description} did not become ${JSON.stringify(expected)} within 60 seconds`);
};
const assertRegistryObject = (item) => {
  const spec = `${item.name}@${item.version}`;
  const integrity = registryValue(spec, "dist.integrity");
  if (integrity !== item.integrity) {
    throw new Error(
      integrity === undefined
        ? `${spec} is not visible on the public registry`
        : `${spec} exists with integrity ${integrity}, not this release's ${item.integrity}`,
    );
  }
};

const operation = process.argv[2];
if (operation === "bootstrap") {
  if (!process.env.NODE_AUTH_TOKEN) {
    throw new Error("the protected npm-production environment has no NPM_DIST_TAG_TOKEN");
  }
  const missing = manifest.packages.filter((item) => {
    const versions = registryValue(item.name, "versions");
    return (
      versions === undefined ||
      (item.version !== "0.0.0" &&
        Array.isArray(versions) &&
        versions.length === 1 &&
        versions[0] === "0.0.0")
    );
  });
  if (missing.length === 0) {
    throw new Error("every package in this release has a non-placeholder version; use stage");
  }
  for (const item of missing) {
    const spec = `${item.name}@${item.version}`;
    if (registryValue(spec, "dist.integrity") !== undefined) {
      throw new Error(`${spec} exists even though ${item.name} was not visible`);
    }
  }
  for (const item of missing) {
    const spec = `${item.name}@${item.version}`;
    run(
      [
        "publish",
        path.join(directory, item.filename),
        "--access",
        "public",
        "--tag",
        "next",
        "--provenance",
      ],
      "inherit",
    );
    process.stdout.write(`bootstrapped ${spec} (${item.integrity})\n`);
  }
} else if (operation === "stage") {
  const existingIntegrities = new Map();
  // Fail before mutating the registry if any immutable version in the release set collides.
  // This keeps a corrected dispatch from having to reason about an avoidable partial stage.
  for (const item of manifest.packages) {
    const spec = `${item.name}@${item.version}`;
    const existing = registryValue(spec, "dist.integrity");
    if (existing !== undefined && existing !== item.integrity) {
      throw new Error(`${spec} is immutable and already has a different registry integrity`);
    }
    existingIntegrities.set(spec, existing);
  }
  for (const item of manifest.packages) {
    const spec = `${item.name}@${item.version}`;
    const existing = existingIntegrities.get(spec);
    if (existing === undefined) {
      run(
        [
          "publish",
          path.join(directory, item.filename),
          "--access",
          "public",
          "--tag",
          "next",
          "--provenance",
        ],
        "inherit",
      );
      await waitFor(
        () => registryValue(spec, "dist.integrity"),
        item.integrity,
        `${spec} integrity`,
      );
    }
    await waitFor(
      () => registryValue(`${item.name}@next`, "version"),
      item.version,
      `${item.name}@next`,
    );
    process.stdout.write(`staged ${spec} (${item.integrity})\n`);
  }
} else if (operation === "promote") {
  if (!process.env.NODE_AUTH_TOKEN) {
    throw new Error("the protected npm-production environment has no NPM_DIST_TAG_TOKEN");
  }
  // Verify the complete release set before moving any public tag. Promotion never rebuilds and
  // can be retried safely if the registry fails between individual dist-tag mutations.
  for (const item of manifest.packages) {
    const spec = `${item.name}@${item.version}`;
    assertRegistryObject(item);
    const staged = registryValue(`${item.name}@next`, "version");
    if (staged !== item.version) {
      throw new Error(`${item.name}@next is ${staged ?? "absent"}; refusing to promote ${spec}`);
    }
  }
  for (const item of manifest.packages) {
    const spec = `${item.name}@${item.version}`;
    run(["dist-tag", "add", spec, "latest"], "inherit");
    await waitFor(
      () => registryValue(`${item.name}@latest`, "version"),
      item.version,
      `${item.name}@latest`,
    );
    process.stdout.write(`promoted ${spec} without republishing\n`);
  }
} else {
  throw new Error("usage: publish.mjs bootstrap|stage|promote");
}
