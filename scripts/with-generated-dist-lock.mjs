#!/usr/bin/env bun
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..");
// Keep the mutex on the same filesystem as the dist trees it protects. CI
// runners can share /tmp while mounting independent checkouts at the same
// logical path; a temp path hash would serialize those unrelated outputs.
const lockDir = resolve(repoRoot, ".aex-generated-dist.lock");
const ownerPath = join(lockDir, "owner.json");
const breakerDir = `${lockDir}.breaker`;
const heldEnv = "AEX_GENERATED_DIST_LOCK_HELD";
const timeoutMs = Number(process.env.AEX_GENERATED_DIST_LOCK_TIMEOUT_MS ?? 20 * 60_000);
const staleMs = Number(process.env.AEX_GENERATED_DIST_LOCK_STALE_MS ?? 10 * 60_000);
const pollMs = 250;
const token = createHash("sha256").update(lockDir).digest("hex");
const ownerId = randomUUID();

const repoRel = relative(repoRoot, lockDir);
if (repoRel.startsWith("..") || isAbsolute(repoRel)) {
  throw new Error(`refusing to use generated-dist lock outside repository: ${lockDir}`);
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
    void refreshOwnedLock().catch(() => {});
  }, 30_000);
  heartbeat.unref?.();
  try {
    process.exitCode = await runCommand(args, { ...process.env, [heldEnv]: token });
  } finally {
    clearInterval(heartbeat);
    await releaseOwnedLock();
  }
}

async function acquireLock() {
  const startedAt = Date.now();
  while (true) {
    try {
      await mkdir(lockDir);
      try {
        await writeOwnerFile();
      } catch (error) {
        await rm(lockDir, { recursive: true, force: true });
        throw error;
      }
      return;
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
      if (await breakStaleLock()) {
        continue;
      }
      if (Date.now() - startedAt > timeoutMs) {
        throw new Error(`timed out waiting for generated-dist lock at ${lockDir}`);
      }
      await sleep(pollMs);
    }
  }
}

async function breakStaleLock() {
  try {
    await mkdir(breakerDir);
  } catch (error) {
    if (error?.code === "EEXIST") return false;
    throw error;
  }
  try {
    if (!(await isStaleLock())) return false;
    await rm(lockDir, { recursive: true, force: true });
    return true;
  } finally {
    await rm(breakerDir, { recursive: true, force: true });
  }
}

async function isStaleLock() {
  try {
    const lockStat = await stat(ownerPath).catch(() => stat(lockDir));
    if (Date.now() - lockStat.mtimeMs <= staleMs) return false;
    const owner = await readOwner();
    return owner?.pid === undefined || !isProcessAlive(owner.pid);
  } catch {
    return false;
  }
}

function isProcessAlive(pid) {
  if (!Number.isSafeInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

async function readOwner() {
  try {
    const parsed = JSON.parse(await readFile(ownerPath, "utf8"));
    if (!parsed || typeof parsed !== "object") return undefined;
    return {
      ownerId: typeof parsed.ownerId === "string" ? parsed.ownerId : undefined,
      pid: Number.isSafeInteger(parsed.pid) ? parsed.pid : undefined
    };
  } catch {
    return undefined;
  }
}

async function refreshOwnedLock() {
  const owner = await readOwner();
  if (owner?.ownerId !== ownerId) return;
  await writeOwnerFile();
}

async function releaseOwnedLock() {
  const owner = await readOwner();
  if (owner?.ownerId !== ownerId) return;
  await rm(lockDir, { recursive: true, force: true });
}

async function writeOwnerFile() {
  await writeFile(
    ownerPath,
    JSON.stringify(
      {
        ownerId,
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
