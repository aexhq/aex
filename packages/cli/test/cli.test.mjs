import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";
import test from "node:test";

const exec = promisify(execFile);

test("help presents only the small session vocabulary", async () => {
  const { stdout, stderr } = await exec(process.execPath, ["dist/index.js", "--help"], {
    cwd: new URL("..", import.meta.url),
  });
  assert.equal(stderr, "");
  assert.match(stdout, /^Aex —/);
  assert.match(stdout, /session output/);
  assert.match(stdout, /https:\/\/api\.aex\.dev/);
  assert.doesNotMatch(stdout, /\bAEX\b/);
  assert.doesNotMatch(stdout, /workspace|microvm|hand|region/i);
});

test("login accepts a piped dashboard key and writes only that secret", async () => {
  const directory = await mkdtemp(join(tmpdir(), "aex-cli-"));
  const config = join(directory, "config.json");
  const apiKey = `aex_sk_${"A".repeat(40)}`;
  try {
    const child = spawn(process.execPath, ["dist/index.js", "login"], {
      cwd: new URL("..", import.meta.url),
      env: { ...process.env, AEX_CONFIG: config },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk) => (stdout += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk) => (stderr += chunk));
    child.stdin.end(`${apiKey}\n`);
    const exitCode = await new Promise((resolve, reject) => {
      child.once("error", reject);
      child.once("close", resolve);
    });

    assert.equal(exitCode, 0);
    assert.equal(stderr, "");
    assert.equal(stdout, "Saved.\n");
    assert.deepEqual(JSON.parse(await readFile(config, "utf8")), { apiKey });
    if (process.platform !== "win32") {
      assert.equal((await stat(config)).mode & 0o777, 0o600);
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("tools build discovers entries and emits a prepared deterministic module", async () => {
  const directory = await mkdtemp(join(process.cwd(), ".tool-build-"));
  try {
    await writeFile(join(directory, "package.json"), JSON.stringify({
      type: "module",
      aex: {
        tools: {
          entries: { "./lookup": "./lookup.ts" },
          outDir: "./dist",
        },
      },
    }));
    await writeFile(join(directory, "lookup.ts"), `
import { tool } from "@aexhq/sdk";
import { z } from "zod";
export default tool(z.object({ id: z.string() }), async function lookup({ id }) { return { id }; })
  .returns(z.object({ id: z.string() }))
  .needs({ workspace: true, recovery: "retained" });
`);
    const first = await exec(process.execPath, [resolve("dist/index.js"), "tools", "build"], { cwd: directory });
    assert.equal(first.stderr, "");
    assert.match(first.stdout, /^lookup -> [0-9a-f]{64} \([1-9][0-9]* bytes\)\n$/);
    const prepared = (await import(`${pathToFileURL(join(directory, "dist", "lookup.js"))}?first`)).default;
    assert.equal(prepared.kind, "aex.tool");
    assert.equal(prepared.name, "lookup");
    assert.match(prepared.artifact.digest, /^[0-9a-f]{64}$/);
    const manifest = JSON.parse(await readFile(join(directory, "dist", "lookup.artifact.json"), "utf8"));
    assert.equal(manifest.profile, "computer/v1");
    assert.equal(manifest.target, "linux-arm64");

    const second = await exec(process.execPath, [resolve("dist/index.js"), "tools", "build"], { cwd: directory });
    assert.equal(second.stdout, first.stdout);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
