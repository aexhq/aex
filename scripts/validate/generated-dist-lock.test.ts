import { spawn } from "node:child_process";
import { access, copyFile, mkdir, readdir, rm, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "bun:test";

const sourceRepoRoot = resolve(import.meta.dirname, "../..");
const fixtureRepoRoot = join(tmpdir(), `aex-generated-dist-fixture-${process.pid}`);
const secondFixtureRepoRoot = join(tmpdir(), `aex-generated-dist-fixture-second-${process.pid}`);
const lockDir = join(fixtureRepoRoot, ".aex-generated-dist.lock");
const secondLockDir = join(secondFixtureRepoRoot, ".aex-generated-dist.lock");
const breakerDir = `${lockDir}.breaker`;
const probeRoot = join(tmpdir(), `aex-generated-dist-probe-${process.pid}`);
const sourceLockScript = join(sourceRepoRoot, "scripts", "with-generated-dist-lock.mjs");
const lockScript = join(fixtureRepoRoot, "scripts", "with-generated-dist-lock.mjs");
const secondLockScript = join(secondFixtureRepoRoot, "scripts", "with-generated-dist-lock.mjs");
const probeScript = join(sourceRepoRoot, "scripts", "validate", "fixtures", "generated-dist-lock-probe.mjs");

beforeEach(async () => {
  await mkdir(join(fixtureRepoRoot, "scripts"), { recursive: true });
  await mkdir(join(secondFixtureRepoRoot, "scripts"), { recursive: true });
  await copyFile(sourceLockScript, lockScript);
  await copyFile(sourceLockScript, secondLockScript);
});

afterEach(async () => {
  await rm(lockDir, { recursive: true, force: true });
  await rm(breakerDir, { recursive: true, force: true });
  await rm(probeRoot, { recursive: true, force: true });
  await rm(fixtureRepoRoot, { recursive: true, force: true });
  await rm(secondFixtureRepoRoot, { recursive: true, force: true });
});

describe("generated dist lock", () => {
  it("reclaims one dead stale owner without admitting concurrent commands", async () => {
    await mkdir(lockDir, { recursive: true });
    await writeFile(join(lockDir, "owner.json"), JSON.stringify({ ownerId: "dead", pid: 999_999_999 }), "utf8");
    const old = new Date(Date.now() - 60_000);
    await utimes(join(lockDir, "owner.json"), old, old);
    await mkdir(probeRoot, { recursive: true });

    await Promise.all(Array.from({ length: 8 }, () => runLockedProbe()));

    const entries = await readdir(probeRoot);
    expect(entries.filter((name) => name.startsWith("overlap-"))).toEqual([]);
  }, 20_000);

  it("lets independent output trees build concurrently without a shared temp lock", async () => {
    await mkdir(probeRoot, { recursive: true });

    const builds = Promise.all([
      runLockedProbe({ script: lockScript, cwd: fixtureRepoRoot, holdMs: 1_000 }),
      runLockedProbe({ script: secondLockScript, cwd: secondFixtureRepoRoot, holdMs: 1_000 })
    ]);
    await waitFor(async () => {
      try {
        return (await readdir(join(probeRoot, "active"))).length === 2;
      } catch {
        return false;
      }
    });
    const outputScopedLocks = await Promise.all([
      access(lockDir).then(() => true, () => false),
      access(secondLockDir).then(() => true, () => false)
    ]);
    await builds;

    const entries = await readdir(probeRoot);
    expect(outputScopedLocks).toEqual([true, true]);
    expect(entries.some((name) => name.startsWith("overlap-"))).toBe(true);
  }, 20_000);

  it("does not delete a replacement owner's lock during cleanup", async () => {
    await mkdir(probeRoot, { recursive: true });
    const done = runLockedProbe();
    await waitFor(async () => {
      try {
        return (await readdir(join(probeRoot, "active"))).length === 1;
      } catch {
        return false;
      }
    });
    await writeFile(
      join(lockDir, "owner.json"),
      JSON.stringify({ ownerId: "replacement-owner", pid: process.pid }),
      "utf8"
    );

    await done;

    // The replacement owner's lock directory must survive cleanup. Assert on
    // resolution only: node resolves `access()` with undefined while bun
    // resolves it with null, and the invariant is "still accessible".
    await expect(access(lockDir).then(() => true)).resolves.toBe(true);
  }, 20_000);
});

function runLockedProbe(
  options: { readonly script?: string; readonly cwd?: string; readonly holdMs?: number } = {}
): Promise<void> {
  const script = options.script ?? lockScript;
  const cwd = options.cwd ?? fixtureRepoRoot;
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(
      process.execPath,
      [script, process.execPath, probeScript, probeRoot, String(options.holdMs ?? 100)],
      {
        cwd,
        env: {
          ...process.env,
          AEX_GENERATED_DIST_LOCK_STALE_MS: "100",
          AEX_GENERATED_DIST_LOCK_TIMEOUT_MS: "10000"
        },
        stdio: "pipe"
      }
    );
    let stderr = "";
    child.stderr.on("data", (chunk) => { stderr += String(chunk); });
    child.on("error", rejectRun);
    child.on("close", (code) => {
      if (code === 0) resolveRun();
      else rejectRun(new Error(`locked probe exited ${code}: ${stderr}`));
    });
  });
}

async function waitFor(predicate: () => Promise<boolean>): Promise<void> {
  const deadline = Date.now() + 5_000;
  while (!(await predicate())) {
    if (Date.now() >= deadline) throw new Error("timed out waiting for locked probe");
    await new Promise((resolveWait) => setTimeout(resolveWait, 10));
  }
}
