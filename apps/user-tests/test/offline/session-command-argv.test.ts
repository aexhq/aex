import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { getBunCommand, runCommand } from "../_fixtures/install.js";

describe("runCommand argv spawning", () => {
  it("passes an argv element containing spaces as one child argument", async () => {
    const dir = mkdtempSync(join(tmpdir(), "aex-session-command-argv-"));
    try {
      const script = join(dir, "argv.mjs");
      writeFileSync(script, 'process.stdout.write(JSON.stringify(process.argv.slice(2)));');

      const result = await runCommand(getBunCommand(), [script, "prompt with spaces", "seeded case filter"], {
        timeoutMs: 10_000
      });

      expect(result.exitCode, result.stderr).toBe(0);
      expect(JSON.parse(result.stdout)).toEqual(["prompt with spaces", "seeded case filter"]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  if (process.platform === "win32") {
    it("passes an argv element containing spaces through a .cmd shim fallback", async () => {
      const dir = mkdtempSync(join(tmpdir(), "aex-session-command-cmd-"));
      try {
        const script = join(dir, "argv.cmd");
        writeFileSync(script, "@echo off\r\necho first=%~1\r\necho second=%~2\r\n");

        const result = await runCommand(script, ["prompt with spaces", "seeded case filter"], {
          timeoutMs: 10_000
        });

        expect(result.exitCode, result.stderr).toBe(0);
        expect(result.stdout.replace(/\r/g, "")).toBe("first=prompt with spaces\nsecond=seeded case filter\n");
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    });
  }
});
