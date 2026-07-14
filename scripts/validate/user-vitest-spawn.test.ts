import { execFileSync, spawn, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";

interface SpawnInvocation {
  readonly command: string;
  readonly args: string[];
  readonly options: {
    readonly shell: false;
    readonly windowsVerbatimArguments?: true;
  };
}

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const runUserVitestUrl = pathToFileURL(resolve(repoRoot, "apps/user-tests/scripts/user-vitest.mjs")).href;

function callUserVitestValue<T>(expression: string): T {
  const code = [
    `const mod = await import(${JSON.stringify(runUserVitestUrl)});`,
    `const result = ${expression};`,
    "process.stdout.write(JSON.stringify(result));"
  ].join("\n");
  return JSON.parse(
    execFileSync(process.execPath, ["--input-type=module", "--eval", code], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"]
    })
  ) as T;
}

function callUserVitestHelper(expression: string): SpawnInvocation {
  return callUserVitestValue<SpawnInvocation>(expression);
}

describe("user-vitest argv spawning", () => {
  it("keeps --testNamePattern values with spaces in one argv element", () => {
    const command = process.platform === "win32" ? "C:\\tools\\bun.exe" : "/usr/local/bin/bun";
    const pattern = "seeded file/navigation case covers read write edit";
    const invocation = callUserVitestHelper(
      `mod.buildUserVitestSpawnInvocation(${JSON.stringify(
        ["--config", "vitest.tool-fuzz.config.ts", "--testNamePattern", pattern]
      )}, ${JSON.stringify(command)})`
    );

    expect(invocation.command).toBe(command);
    expect(invocation.args).toEqual([
      "run",
      "vitest",
      "run",
      "--config",
      "vitest.tool-fuzz.config.ts",
      "--testNamePattern",
      pattern
    ]);
    expect(invocation.options).toEqual({ shell: false });
  });

  it("limits Windows shell fallback to command shims", () => {
    const pattern = "seeded custom-tool case keeps one pattern arg";
    const command = process.platform === "win32" ? "C:\\tools\\vitest.cmd" : "/usr/local/bin/vitest.cmd";
    const invocation = callUserVitestHelper(
      `mod.buildSpawnInvocation(${JSON.stringify(command)}, ${JSON.stringify(["--testNamePattern", pattern])}, ` +
        `${JSON.stringify({ ComSpec: "C:\\Windows\\System32\\cmd.exe" })})`
    );

    const expected =
      process.platform === "win32"
        ? {
            command: "C:\\Windows\\System32\\cmd.exe",
            args: [
              "/d",
              "/s",
              "/c",
              `"\"${command}\" \"--testNamePattern\" \"${pattern}\""`
            ],
            options: { shell: false, windowsVerbatimArguments: true }
          }
        : {
            command,
            args: ["--testNamePattern", pattern],
            options: { shell: false }
          };

    expect(invocation).toEqual(expected);
  });

  it("removes and reports fixture residue owned by a completed user-test run", () => {
    const parentDir = mkdtempSync(join(tmpdir(), "aex-user-vitest-cleanup-test-"));
    const runRoot = mkdtempSync(join(parentDir, "aex-user-test-run-"));
    const retainedInstall = join(runRoot, "install-retained");
    mkdirSync(retainedInstall);

    try {
      const result = callUserVitestValue<{ readonly message: string }>(
        `await (async () => { try { mod.cleanupUserTestTempRoot(${JSON.stringify(runRoot)}); ` +
          `return { message: "unexpected success" }; } catch (error) { return { message: error.message }; } })()`
      );
      expect(result.message).toMatch(
        /fixture cleanup left 1 entr(?:y|ies)[\s\S]*install-retained[\s\S]*removed the owned run root/
      );
      expect(existsSync(runRoot)).toBe(false);
    } finally {
      rmSync(parentDir, { recursive: true, force: true });
    }
  });

  it("removes expected fixture residue during graceful cancellation", () => {
    const parentDir = mkdtempSync(join(tmpdir(), "aex-user-vitest-cancel-cleanup-test-"));
    const runRoot = mkdtempSync(join(parentDir, "aex-user-test-run-"));
    mkdirSync(join(runRoot, "install-retained"));

    try {
      const result = callUserVitestValue<{ readonly cleaned: boolean }>(
        `(() => { mod.cleanupUserTestTempRoot(${JSON.stringify(runRoot)}, { allowRetainedEntries: true }); ` +
          `return { cleaned: true }; })()`
      );
      expect(result).toEqual({ cleaned: true });
      expect(existsSync(runRoot)).toBe(false);
    } finally {
      rmSync(parentDir, { recursive: true, force: true });
    }
  });

  it("exits nonzero when a Vitest worker leaves an install tree behind", () => {
    const dir = mkdtempSync(join(tmpdir(), "aex-user-vitest-residue-test-"));
    const fakeTarball = join(dir, "aexhq-sdk-0.0.0.tgz");
    const fakeBun = join(dir, process.platform === "win32" ? "fake-bun.cmd" : "fake-bun");
    writeFileSync(fakeTarball, "fixture");
    writeFileSync(
      fakeBun,
      process.platform === "win32"
        ? [
            "@echo off",
            "if defined AEX_USER_TEST_TEMP_ROOT mkdir \"%AEX_USER_TEST_TEMP_ROOT%\\install-retained\"",
            "exit /b 0"
          ].join("\r\n")
        : [
            "#!/bin/sh",
            'if [ -n "$AEX_USER_TEST_TEMP_ROOT" ]; then mkdir "$AEX_USER_TEST_TEMP_ROOT/install-retained"; fi',
            "exit 0"
          ].join("\n")
    );
    if (process.platform !== "win32") chmodSync(fakeBun, 0o755);

    try {
      const result = spawnSync(process.execPath, [fileURLToPath(runUserVitestUrl), "--config", "fake.config.ts"], {
        cwd: repoRoot,
        env: {
          ...process.env,
          AEX_USER_TEST_BUN: fakeBun,
          AEX_USER_TEST_TARBALL: fakeTarball,
          AEX_USER_TEST_VERSION: ""
        },
        encoding: "utf8"
      });

      expect(result.status).toBe(1);
      expect(result.stderr).toMatch(
        /fixture cleanup left 1 entr(?:y|ies)[\s\S]*install-retained[\s\S]*removed the owned run root/
      );
      const runRoot = result.stderr.match(/in ([^\r\n]+aex-user-test-run-[^:]+):/)?.[1];
      expect(runRoot).toBeTruthy();
      expect(existsSync(runRoot!)).toBe(false);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  if (process.platform !== "win32") {
    it.each(["SIGINT", "SIGTERM"] as const)(
      "forwards %s, awaits child close, and removes retained install trees",
      async (signal) => {
        const dir = mkdtempSync(join(tmpdir(), "aex-user-vitest-signal-test-"));
        const fakeTarball = join(dir, "aexhq-sdk-0.0.0.tgz");
        const fakeBun = join(dir, "fake-bun");
        const readyFile = join(dir, "vitest-child-ready");
        writeFileSync(fakeTarball, "fixture");
        writeFileSync(
          fakeBun,
          [
            "#!/bin/sh",
            'if [ -z "$AEX_USER_TEST_TEMP_ROOT" ]; then exit 0; fi',
            "trap 'exit 0' INT TERM",
            'mkdir "$AEX_USER_TEST_TEMP_ROOT/install-retained"',
            'printf ready > "$AEX_USER_TEST_SIGNAL_READY"',
            "while :; do sleep 1; done"
          ].join("\n")
        );
        chmodSync(fakeBun, 0o755);

        const child = spawn(process.execPath, [fileURLToPath(runUserVitestUrl), "--config", "fake.config.ts"], {
          cwd: repoRoot,
          env: {
            ...process.env,
            AEX_USER_TEST_BUN: fakeBun,
            AEX_USER_TEST_SIGNAL_READY: readyFile,
            AEX_USER_TEST_TARBALL: fakeTarball,
            AEX_USER_TEST_VERSION: ""
          },
          stdio: ["ignore", "pipe", "pipe"]
        });
        let stderr = "";
        child.stderr.on("data", (chunk) => {
          stderr += chunk.toString();
        });
        const close = new Promise<{ readonly code: number | null; readonly signal: NodeJS.Signals | null }>(
          (resolvePromise, reject) => {
            child.once("error", reject);
            child.once("close", (code, closeSignal) => resolvePromise({ code, signal: closeSignal }));
          }
        );

        try {
          await waitForPath(readyFile, 5_000);
          expect(child.kill(signal)).toBe(true);
          const outcome = await within(close, 5_000, `user-vitest did not exit after ${signal}`);

          expect(outcome).toEqual({ code: 1, signal: null });
          expect(stderr).toContain(`received ${signal}; forwarding it to the Vitest child and waiting for close`);
          expect(stderr).toContain(`exiting after graceful ${signal} cancellation`);
          expect(stderr).toMatch(
            /graceful cancellation retained 1 entr(?:y|ies)[\s\S]*install-retained[\s\S]*removed the owned run root/
          );
          const runRoot = stderr.match(/in ([^\r\n]+aex-user-test-run-[^:]+):/)?.[1];
          expect(runRoot).toBeTruthy();
          expect(existsSync(runRoot!)).toBe(false);
        } finally {
          if (child.exitCode === null && child.signalCode === null) {
            // Test-only timeout containment; graceful-signal behavior is asserted above.
            child.kill("SIGKILL");
          }
          rmSync(dir, { recursive: true, force: true });
        }
      }
    );
  }
});

async function waitForPath(path: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (existsSync(path)) return;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 20));
  }
  throw new Error(`timed out waiting for ${path}`);
}

async function within<T>(promise: Promise<T>, timeoutMs: number, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_resolvePromise, reject) => {
        timer = setTimeout(() => reject(new Error(message)), timeoutMs);
      })
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}
