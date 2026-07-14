import { execFileSync, spawnSync } from "node:child_process";
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
      encoding: "utf8"
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
});
