import { execFileSync, spawn, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { describe, expect, it } from "bun:test";

interface SpawnInvocation {
  readonly command: string;
  readonly args: string[];
  readonly options: {
    readonly shell: false;
    readonly windowsVerbatimArguments?: true;
  };
}

interface LaneInvocation {
  readonly bunTestArgs: string[];
  readonly reportPath: string | null;
  readonly gateArgs: string[] | null;
  readonly suite: "sweep" | "smoke" | null;
}

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const runnerUrl = pathToFileURL(resolve(repoRoot, "apps/user-tests/scripts/user-bun-test.mjs")).href;

function callRunnerValue<T>(expression: string): T {
  const code = [
    `const mod = await import(${JSON.stringify(runnerUrl)});`,
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

function callRunnerHelper(expression: string): SpawnInvocation {
  return callRunnerValue<SpawnInvocation>(expression);
}

function laneInvocation(argv: readonly string[], envLike: Record<string, string>): LaneInvocation {
  return callRunnerValue<LaneInvocation>(
    `mod.prepareLaneInvocation(${JSON.stringify(argv)}, ${JSON.stringify(envLike)})`
  );
}

function laneInvocationError(argv: readonly string[], envLike: Record<string, string>): string {
  return callRunnerValue<{ readonly message: string }>(
    `(() => { try { mod.prepareLaneInvocation(${JSON.stringify(argv)}, ${JSON.stringify(envLike)}); ` +
      `return { message: "unexpected success" }; } catch (error) { return { message: error.message }; } })()`
  ).message;
}

describe("user-bun-test argv spawning", () => {
  it("spawns `bun test` with lane args passed through verbatim, keeping -t values in one argv element", () => {
    const command = process.platform === "win32" ? "C:\\tools\\bun.exe" : "/usr/local/bin/bun";
    const pattern = "seeded file/navigation case covers read write edit";
    const invocation = callRunnerHelper(
      `mod.buildUserBunTestSpawnInvocation(${JSON.stringify(
        ["--isolate", "--timeout=1200000", "-t", pattern, "test/live/live-sdk-tool-capability-fuzz.test.ts"]
      )}, ${JSON.stringify(command)})`
    );

    expect(invocation.command).toBe(command);
    expect(invocation.args).toEqual([
      "test",
      "--isolate",
      "--timeout=1200000",
      "-t",
      pattern,
      "test/live/live-sdk-tool-capability-fuzz.test.ts"
    ]);
    expect(invocation.options).toEqual({ shell: false });
  });

  it("limits Windows shell fallback to command shims", () => {
    const pattern = "seeded custom-tool case keeps one pattern arg";
    const command = process.platform === "win32" ? "C:\\tools\\bun.cmd" : "/usr/local/bin/bun.cmd";
    const invocation = callRunnerHelper(
      `mod.buildSpawnInvocation(${JSON.stringify(command)}, ${JSON.stringify(["-t", pattern])}, ` +
        `${JSON.stringify({ ComSpec: "C:\\Windows\\System32\\cmd.exe" })})`
    );

    const expected: SpawnInvocation =
      process.platform === "win32"
        ? {
            command: "C:\\Windows\\System32\\cmd.exe",
            args: [
              "/d",
              "/s",
              "/c",
              `"\"${command}\" \"-t\" \"${pattern}\""`
            ],
            options: { shell: false, windowsVerbatimArguments: true }
          }
        : {
            command,
            args: ["-t", pattern],
            options: { shell: false }
          };

    expect(invocation).toEqual(expected);
  });
});

describe("user-bun-test lane argument processing", () => {
  it("injects the junit reporter for --report and runs the no-skips gate on that report", () => {
    const lane = laneInvocation(
      ["--report=.tmp/junit-test-user-heavy.xml", "--isolate", "--timeout=900000", "test/live/live-sdk-heavy-session.test.ts"],
      {}
    );

    expect(lane.bunTestArgs).toEqual([
      "--isolate",
      "--timeout=900000",
      "test/live/live-sdk-heavy-session.test.ts",
      "--reporter=junit",
      "--reporter-outfile=.tmp/junit-test-user-heavy.xml"
    ]);
    expect(lane.reportPath).toBe(".tmp/junit-test-user-heavy.xml");
    expect(lane.gateArgs).toEqual([".tmp/junit-test-user-heavy.xml"]);
    expect(lane.suite).toBeNull();
  });

  it("mirrors a bun -t filter into the gate's --name-pattern (bun marks non-selected tests skipped)", () => {
    const pattern = "seeded web case";
    const lane = laneInvocation(
      ["--report=.tmp/junit-test-user-tool-fuzz.xml", "--isolate", "test/live/live-sdk-tool-capability-fuzz.test.ts", "-t", pattern],
      {}
    );

    expect(lane.bunTestArgs).toContain("-t");
    expect(lane.bunTestArgs).toContain(pattern);
    expect(lane.gateArgs).toEqual(["--name-pattern", pattern, ".tmp/junit-test-user-tool-fuzz.xml"]);
  });

  it("resolves --parallel-env from the environment with its fallback, mirroring the vitest worker-count contract", () => {
    const fallback = laneInvocation(["--parallel-env=AEX_USER_TEST_MAX_WORKERS:2", "test/offline"], {});
    expect(fallback.bunTestArgs).toEqual(["--parallel=2", "test/offline"]);

    const fromEnv = laneInvocation(
      ["--parallel-env=AEX_USER_TEST_MAX_WORKERS:2", "test/offline"],
      { AEX_USER_TEST_MAX_WORKERS: "4" }
    );
    expect(fromEnv.bunTestArgs).toEqual(["--parallel=4", "test/offline"]);

    expect(
      laneInvocationError(["--parallel-env=AEX_USER_TEST_MAX_WORKERS:2", "test/offline"], {
        AEX_USER_TEST_MAX_WORKERS: "zero"
      })
    ).toMatch(/AEX_USER_TEST_MAX_WORKERS must be a positive integer/);
    expect(
      laneInvocationError(["--parallel-env=AEX_USER_TEST_MAX_WORKERS:2", "test/offline"], {
        AEX_USER_TEST_MAX_WORKERS: "0"
      })
    ).toMatch(/AEX_USER_TEST_MAX_WORKERS must be a positive integer/);
  });

  it("refuses to run without an explicit file selection (bare `bun test` would collect every live suite)", () => {
    expect(laneInvocationError(["--isolate", "--timeout=180000"], {})).toMatch(/explicit file selection/);
  });

  it("treats values of known value-taking flags as flag values, not file selections", () => {
    expect(laneInvocationError(["--isolate", "-t", "some test name"], {})).toMatch(/explicit file selection/);
    expect(laneInvocationError(["--reporter", "junit", "--reporter-outfile", "out.xml"], {})).toMatch(
      /explicit file selection/
    );
  });

  it("accepts a suite selector instead of files and rejects mixing the two", () => {
    const sweep = laneInvocation(["--sweep", "--isolate"], {});
    expect(sweep.suite).toBe("sweep");
    const smoke = laneInvocation(["--smoke", "--isolate"], {});
    expect(smoke.suite).toBe("smoke");
    expect(laneInvocationError(["--sweep", "--smoke"], {})).toMatch(/only one of/);
    expect(laneInvocationError(["--sweep", "test/offline"], {})).toMatch(/either a suite selector or explicit files/);
  });

  it("collects the default sweep exactly as the retired vitest.config.ts glob did", () => {
    const files = callRunnerValue<string[]>("mod.collectDefaultSweepFiles()");

    // Spot invariants of the vitest include/exclude pair this replaces:
    // include test/**/*.test.ts; exclude the four dedicated-lane files,
    // providers/**, e2e/** and node_modules/**.
    expect(files.length).toBeGreaterThan(30);
    expect(new Set(files).size).toBe(files.length);
    expect(files).toContain("test/_fixtures/install.test.ts");
    expect(files).toContain("test/offline/install.test.ts");
    expect(files).toContain("test/live/live-sdk-deepseek.test.ts");
    for (const file of files) {
      expect(file).toMatch(/^test\/.*\.test\.ts$/);
      expect(file).not.toMatch(/^test\/e2e\//);
      expect(file).not.toMatch(/^test\/live\/providers\//);
      expect(file).not.toContain("node_modules");
    }
    expect(files).not.toContain("test/live/edge-admission-gates.user.test.ts");
    expect(files).not.toContain("test/live/live-sdk-heavy-session.test.ts");
    expect(files).not.toContain("test/live/live-api-fuzz.test.ts");
    expect(files).not.toContain("test/live/live-sdk-tool-capability-fuzz.test.ts");
  });

  it("gates `--parallel` behind the crashed-worker-safe bun floor", () => {
    const check = (args: readonly string[], version: string | null): string =>
      callRunnerValue<{ readonly message: string }>(
        `(() => { try { mod.assertParallelBunFloor(${JSON.stringify(args)}, ${JSON.stringify(version)} ?? undefined); ` +
          `return { message: "ok" }; } catch (error) { return { message: error.message }; } })()`
      ).message;

    expect(check(["--parallel=2", "test/offline"], "1.3.14")).toBe("ok");
    expect(check(["--parallel=2", "test/offline"], "1.4.0")).toBe("ok");
    expect(check(["--parallel=2", "test/offline"], "1.3.12")).toMatch(/requires bun >= 1\.3\.14/);
    expect(check(["--parallel=2", "test/offline"], null)).toMatch(/requires bun >= 1\.3\.14/);
    expect(check(["test/offline"], null)).toBe("ok");
  });
});

describe("user-bun-test temp-root ownership", () => {
  it("removes and reports fixture residue owned by a completed user-test run", () => {
    const parentDir = mkdtempSync(join(tmpdir(), "aex-user-bun-test-cleanup-test-"));
    const runRoot = mkdtempSync(join(parentDir, "aex-user-test-run-"));
    const retainedInstall = join(runRoot, "install-retained");
    mkdirSync(retainedInstall);

    try {
      const result = callRunnerValue<{ readonly message: string }>(
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
    const parentDir = mkdtempSync(join(tmpdir(), "aex-user-bun-test-cancel-cleanup-test-"));
    const runRoot = mkdtempSync(join(parentDir, "aex-user-test-run-"));
    mkdirSync(join(runRoot, "install-retained"));

    try {
      const result = callRunnerValue<{ readonly cleaned: boolean }>(
        `(() => { mod.cleanupUserTestTempRoot(${JSON.stringify(runRoot)}, { allowRetainedEntries: true }); ` +
          `return { cleaned: true }; })()`
      );
      expect(result).toEqual({ cleaned: true });
      expect(existsSync(runRoot)).toBe(false);
    } finally {
      rmSync(parentDir, { recursive: true, force: true });
    }
  });

  it("exits nonzero when a bun test worker leaves an install tree behind", () => {
    const dir = mkdtempSync(join(tmpdir(), "aex-user-bun-test-residue-test-"));
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
      const result = spawnSync(process.execPath, [fileURLToPath(runnerUrl), "fake-target.test.ts"], {
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
        const dir = mkdtempSync(join(tmpdir(), "aex-user-bun-test-signal-test-"));
        const fakeTarball = join(dir, "aexhq-sdk-0.0.0.tgz");
        const fakeBun = join(dir, "fake-bun");
        const readyFile = join(dir, "bun-test-child-ready");
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

        const child = spawn(process.execPath, [fileURLToPath(runnerUrl), "fake-target.test.ts"], {
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
          const outcome = await within(close, 5_000, `user-bun-test did not exit after ${signal}`);

          expect(outcome).toEqual({ code: 1, signal: null });
          expect(stderr).toContain(`received ${signal}; forwarding it to the bun test child and waiting for close`);
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
