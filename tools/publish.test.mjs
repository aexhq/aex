import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

function fixture(t, registry) {
  const directory = mkdtempSync(path.join(os.tmpdir(), "aex-release-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const file of ["publish.mjs", "npm-registry.mjs"]) copyFileSync(path.join(import.meta.dirname, file), path.join(directory, file));
  const packages = ["sdk", "cli"].map((name) => {
    const bytes = Buffer.from(name);
    writeFileSync(path.join(directory, `${name}.tgz`), bytes);
    return { name: `@aexhq/${name}`, version: "1.0.0", filename: `${name}.tgz`, integrity: `sha512-${createHash("sha512").update(bytes).digest("base64")}` };
  });
  const source = "a".repeat(40);
  writeFileSync(path.join(directory, "manifest.json"), JSON.stringify({ source, packages }));
  writeFileSync(path.join(directory, "registry.json"), JSON.stringify(registry(packages)));
  writeFileSync(path.join(directory, "npm.mjs"), `
    import { readFileSync, appendFileSync } from 'node:fs';
    const [command, spec, field] = process.argv.slice(2);
    if (command === 'view') {
      const registry = JSON.parse(readFileSync('registry.json'));
      const value = registry[spec]?.[field];
      if (value === undefined) { console.log(JSON.stringify({error:{code:'E404'}})); process.exit(1); }
      console.log(JSON.stringify(value));
    } else {
      appendFileSync('mutations.jsonl', JSON.stringify(process.argv.slice(2)) + '\\n');
    }
  `);
  return {
    directory,
    run: (operation) => spawnSync(process.execPath, [path.join(directory, "publish.mjs"), operation], {
      encoding: "utf8", env: { ...process.env, EXPECTED_COMMIT: source, npm_execpath: path.join(directory, "npm.mjs") },
    }),
    mutations: () => existsSync(path.join(directory, "mutations.jsonl")) ? readFileSync(path.join(directory, "mutations.jsonl"), "utf8").trim().split("\n").map(JSON.parse) : [],
  };
}

test("partial publication resumes using the original unpublished archive", (t) => {
  const release = fixture(t, ([sdk]) => ({ [`${sdk.name}@${sdk.version}`]: { "dist.integrity": sdk.integrity } }));
  const result = release.run("stage");
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(release.mutations().map((args) => [args[0], path.basename(args[1])]), [["publish", "cli.tgz"]]);
});

test("a version collision fails before publishing any package", (t) => {
  const release = fixture(t, ([, cli]) => ({ [`${cli.name}@${cli.version}`]: { "dist.integrity": "different" } }));
  assert.match(release.run("stage").stderr, /different registry integrity/);
  assert.deepEqual(release.mutations(), []);
});

test("verification retries have no registry mutations", (t) => {
  const release = fixture(t, (packages) => Object.fromEntries(packages.flatMap((item) => [
    [`${item.name}@${item.version}`, { "dist.integrity": item.integrity }],
    [`${item.name}@next`, { version: item.version }],
  ])));
  const result = release.run("verify");
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(release.mutations(), []);
});

test("changed archives are rejected before any registry mutation", (t) => {
  const release = fixture(t, () => ({}));
  writeFileSync(path.join(release.directory, "sdk.tgz"), "changed");
  assert.match(release.run("stage").stderr, /archive integrity mismatch/);
  assert.deepEqual(release.mutations(), []);
});
