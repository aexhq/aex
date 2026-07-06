import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
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
const runUserVitestUrl = pathToFileURL(resolve(repoRoot, "apps/user-tests/scripts/run-user-vitest.mjs")).href;

function callRunUserVitestHelper(expression: string): SpawnInvocation {
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
  ) as SpawnInvocation;
}

describe("run-user-vitest argv spawning", () => {
  it("keeps --testNamePattern values with spaces in one argv element", () => {
    const command = process.platform === "win32" ? "C:\\tools\\bun.exe" : "/usr/local/bin/bun";
    const pattern = "seeded file/navigation case covers read write edit";
    const invocation = callRunUserVitestHelper(
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
    const invocation = callRunUserVitestHelper(
      `mod.buildSpawnInvocation(${JSON.stringify(command)}, ${JSON.stringify(["--testNamePattern", pattern])}, ` +
        `${JSON.stringify({ ComSpec: "C:\\Windows\\System32\\cmd.exe" })})`
    );

    if (process.platform === "win32") {
      expect(invocation.command).toBe("C:\\Windows\\System32\\cmd.exe");
      expect(invocation.args).toEqual([
        "/d",
        "/s",
        "/c",
        `"\"${command}\" \"--testNamePattern\" \"${pattern}\""`
      ]);
      expect(invocation.options).toEqual({ shell: false, windowsVerbatimArguments: true });
      return;
    }

    expect(invocation.command).toBe(command);
    expect(invocation.args).toEqual(["--testNamePattern", pattern]);
    expect(invocation.options).toEqual({ shell: false });
  });
});
