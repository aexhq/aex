import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import type { CliIO } from "../src/internal.js";
import {
  parsePositiveLimit,
  pollingDelay,
  portableBasename
} from "../src/host/command-primitives.js";

const hostSource = (name: string): string =>
  readFileSync(new URL(`../src/host/${name}`, import.meta.url), "utf8");

describe("shared command primitive ownership", () => {
  it("keeps each cross-command primitive in one dependency-light owner", () => {
    const primitiveSource = hostSource("command-primitives.ts");
    expect(primitiveSource).not.toContain('from "./common.js"');

    for (const name of ["files.ts", "start-attachments.ts"]) {
      const source = hostSource(name);
      expect(source).toContain('from "./command-primitives.js"');
      expect(source).not.toMatch(/function baseName\s*\(/);
    }
    expect(hostSource("start-cmd.ts")).toContain('from "./start-attachments.js"');

    for (const name of ["billing.ts", "list-cmds.ts"]) {
      const source = hostSource(name);
      expect(source).toContain('from "./command-primitives.js"');
      expect(source).not.toMatch(/function parseLimit\s*\(/);
    }

    for (const name of ["auth-cmd.ts", "events.ts", "wait.ts"]) {
      const source = hostSource(name);
      expect(source).toContain('from "./command-primitives.js"');
      expect(source).not.toMatch(/function sleep\s*\(|const sleep\s*=/);
    }

    expect(hostSource("auth-cmd.ts")).toContain(
      "await pollingDelay(Math.max(0, intervalSec * 1000))"
    );
    expect(hostSource("events.ts")).toContain("await pollingDelay(2000)");
    expect(hostSource("wait.ts")).toContain("await pollingDelay(intervalMs)");
  });
});

describe("portableBasename", () => {
  it.each([
    ["", ""],
    ["plain.txt", "plain.txt"],
    ["/workspace/out/report.txt", "report.txt"],
    ["C:\\workspace\\out\\report.txt", "report.txt"],
    ["/workspace\\out/report.txt", "report.txt"],
    ["/workspace/out/report.txt///", "report.txt"],
    ["C:\\workspace\\out\\report.txt\\\\", "report.txt"],
    ["/", ""],
    ["\\", ""],
    ["C:\\", "C:"]
  ])("preserves the existing portable path result for %j", (input, expected) => {
    expect(portableBasename(input)).toBe(expected);
  });
});

describe("parsePositiveLimit", () => {
  function capture(raw: string | undefined) {
    let stderr = "";
    const io = { stderr: (chunk: string) => { stderr += chunk; } } as CliIO;
    return { result: parsePositiveLimit(io, raw), stderr: () => stderr };
  }

  it.each([
    [undefined, { ok: true, limit: undefined }],
    ["1", { ok: true, limit: 1 }],
    ["01", { ok: true, limit: 1 }],
    [" 2 ", { ok: true, limit: 2 }],
    ["1e2", { ok: true, limit: 100 }]
  ])("preserves successful Number coercion for %j", (raw, expected) => {
    const captured = capture(raw);
    expect(captured.result).toEqual(expected);
    expect(captured.stderr()).toBe("");
  });

  it.each(["", "0", "-1", "1.5", "many", "Infinity"])(
    "preserves the invalid result and exact diagnostic for %j",
    (raw) => {
      const captured = capture(raw);
      expect(captured.result).toEqual({ ok: false });
      expect(captured.stderr()).toBe(`--limit must be a positive integer (got: ${raw})\n`);
    }
  );
});

describe("pollingDelay", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("resolves only after the exact requested delay", async () => {
    vi.useFakeTimers();
    const setTimeoutSpy = vi.spyOn(globalThis, "setTimeout");
    let resolved = false;
    const pending = pollingDelay(25).then(() => { resolved = true; });

    expect(setTimeoutSpy).toHaveBeenCalledWith(expect.any(Function), 25);
    await vi.advanceTimersByTimeAsync(24);
    expect(resolved).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    await pending;
    expect(resolved).toBe(true);
  });

  it("passes negative delay values through to the host timer", async () => {
    vi.useFakeTimers();
    const setTimeoutSpy = vi.spyOn(globalThis, "setTimeout");
    const pending = pollingDelay(-25);

    expect(setTimeoutSpy).toHaveBeenCalledWith(expect.any(Function), -25);
    await vi.runAllTimersAsync();
    await pending;
  });
});
