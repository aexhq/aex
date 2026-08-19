import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import test from "node:test";

const exec = promisify(execFile);

test("help presents only the small session vocabulary", async () => {
  const { stdout, stderr } = await exec(process.execPath, ["dist/index.js", "--help"], {
    cwd: new URL("..", import.meta.url),
  });
  assert.equal(stderr, "");
  assert.match(stdout, /session output/);
  assert.match(stdout, /https:\/\/api\.aex\.dev/);
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
