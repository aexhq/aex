#!/usr/bin/env bun
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..");
const tmpRoot = resolve(tmpdir());
const lockId = createHash("sha256").update(repoRoot).digest("hex").slice(0, 16);
const lockDir = resolve(tmpRoot, `aex-generated-dist-${lockId}.lock`);
const ownerPath = join(lockDir, "owner.json");
const heldEnv = "AEX_GENERATED_DIST_LOCK_HELD";
const timeoutMs = Number(process.env.AEX_GENERATED_DIST_LOCK_TIMEOUT_MS ?? 20 * 60_000);
const staleMs = Number(process.env.AEX_GENERATED_DIST_LOCK_STALE_MS ?? 10 * 60_000);
const pollMs = 250;
const token = createHash("sha256").update(lockDir).digest("hex");

const tmpRel = relative(tmpRoot, lockDir);
if (tmpRel.startsWith("..") || isAbsolute(tmpRel)) {
  throw new Error(`refusing to use generated-dist lock outside temp dir: ${lockDir}`);
}

const args = process.argv.slice(2);
if (args[0] === "--") args.shift();

if (args.length === 0) {
  console.error("usage: bun scripts/with-generated-dist-lock.mjs [--] <command> [...args]");
  process.exit(2);
}

if (process.env[heldEnv] === token) {
  process.exitCode = await runCommand(args, process.env);
} else {
  await acquireLock();
  const heartbeat = setInterval(() => {
    void writeOwnerFile();
  }, 30_000);
  heartbeat.unref?.();
  try {
    process.exitCode = await runCommand(args, { ...process.env, [heldEnv]: token });
  } finally {
    clearInterval(heartbeat);
    await rm(lockDir, { recursive: true, force: true });
  }
}

async function acquireLock() {
  const startedAt = Date.now();
  while (true) {
    try {
      await mkdir(lockDir);
      await writeOwnerFile();
      return;
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
      if (await isStaleLock()) {
        await rm(lockDir, { recursive: true, force: true });
        continue;
      }
      if (Date.now() - startedAt > timeoutMs) {
        throw new Error(`timed out waiting for generated-dist lock at ${lockDir}`);
      }
      await sleep(pollMs);
    }
  }
}

async function isStaleLock() {
  try {
    const lockStat = await stat(ownerPath).catch(() => stat(lockDir));
    return Date.now() - lockStat.mtimeMs > staleMs;
  } catch {
    return false;
  }
}

async function writeOwnerFile() {
  await writeFile(
    ownerPath,
    JSON.stringify(
      {
        pid: process.pid,
        repoRoot,
        updatedAt: new Date().toISOString()
      },
      null,
      2
    ),
    "utf8"
  );
}

function sleep(ms) {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

function runCommand(commandLine, env) {
  const [command, ...commandArgs] = commandLine;
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, commandArgs, {
      cwd: process.cwd(),
      env,
      stdio: "inherit"
    });
    child.on("error", rejectRun);
    child.on("close", (code, signal) => {
      if (signal) {
        console.error(`generated-dist lock command terminated by ${signal}`);
        resolveRun(1);
        return;
      }
      resolveRun(code ?? 1);
    });
  });
}
