import { describe, expect, it } from "vitest";
import {
  DEFAULT_SESSION_TIMEOUT_MS,
  DEFAULT_RUNTIME_SIZE,
  MAX_SESSION_TIMEOUT_MS,
  MIN_SESSION_TIMEOUT_MS,
  RUNTIME_SIZE_PRESETS,
  RUNTIME_SIZES,
  SESSION_PROCESS_KILL_GRACE_MS,
  SESSION_TERMINAL_GRACE_MS,
  RuntimeSizes,
  Models,
  orchestrationTimeoutString,
  parseDurationToMs,
  parseSessionTimeout,
  parseRuntimeSize,
  resolveSessionTimeoutMs,
  runtimeResources
} from "../src/index.js";
import { parseSessionSubmissionRequest } from "../src/internal.js";

function baseRequest(overrides: Record<string, unknown> = {}) {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider: "anthropic",
    submission: { model: Models.CLAUDE_HAIKU_4_5, prompt: ["hello"], assets: { files: [], skills: [], tools: [], instructions: [] },
      builtinTools: "default", mcpServers: [] },
    secrets: { apiKeys: { anthropic: "sk-anthropic-test" } },
    ...overrides
  };
}

describe("runtime size presets", () => {
  it("exposes exactly the five managed runtime preset tokens", () => {
    expect([...RUNTIME_SIZES]).toEqual([
      "0.25cpu-1gb",
      "0.5cpu-4gb",
      "1cpu-6gb",
      "2cpu-8gb",
      "4cpu-12gb"
    ]);
  });

  it.each(RUNTIME_SIZES)("%s is a valid product preset", (size) => {
    const { cpus, memoryMb } = RUNTIME_SIZE_PRESETS[size];
    expect(cpus).toBeGreaterThan(0);
    expect(memoryMb).toBeGreaterThan(0);
  });

  it("has no duplicate resource pairs", () => {
    const seen = new Set(RUNTIME_SIZES.map((s) => `${RUNTIME_SIZE_PRESETS[s].cpus}x${RUNTIME_SIZE_PRESETS[s].memoryMb}`));
    expect(seen.size).toBe(RUNTIME_SIZES.length);
  });
});

describe("RuntimeSizes symbol const stays in lockstep with presets", () => {
  it("maps every symbol to an existing preset token, and covers all of them", () => {
    const symbolValues = Object.values(RuntimeSizes);
    for (const token of symbolValues) {
      expect(RUNTIME_SIZES).toContain(token);
    }
    expect([...symbolValues].sort()).toEqual([...RUNTIME_SIZES].sort());
    expect(new Set(symbolValues).size).toBe(symbolValues.length);
  });
});

describe("default sessiontime size", () => {
  it("resolves to the current default sessiontime resource preset", () => {
    expect(DEFAULT_RUNTIME_SIZE).toBe(RuntimeSizes.CPU_0_25_1GB);
    expect(runtimeResources(DEFAULT_RUNTIME_SIZE)).toEqual({ cpus: 0.25, memoryMb: 1024 });
  });
});

describe("parseRuntimeSize", () => {
  it("accepts an omitted field", () => {
    expect(parseRuntimeSize(undefined)).toBeUndefined();
  });

  it("accepts every preset token", () => {
    for (const size of RUNTIME_SIZES) {
      expect(parseRuntimeSize(size)).toBe(size);
    }
  });

  it("rejects an unknown token with the full menu", () => {
    expect(() => parseRuntimeSize("shared-3x-7gb")).toThrow(/runtimeSize must be one of/);
    expect(() => parseRuntimeSize("shared-2x-3gb")).toThrow(/runtimeSize must be one of/);
    expect(() => parseRuntimeSize(512)).toThrow(/runtimeSize must be one of/);
  });
});

describe("parseDurationToMs", () => {
  it("parses each unit", () => {
    expect(parseDurationToMs("1h")).toBe(3_600_000);
    expect(parseDurationToMs("90m")).toBe(5_400_000);
    expect(parseDurationToMs("30s")).toBe(30_000);
    expect(parseDurationToMs("500ms")).toBe(500);
    expect(parseDurationToMs("1000")).toBe(1000);
  });

  it("throws on malformed input", () => {
    expect(() => parseDurationToMs("soon")).toThrow(/invalid duration/);
    expect(() => parseDurationToMs("1d")).toThrow(/invalid duration/);
  });
});

describe("parseSessionTimeout", () => {
  it("accepts an omitted field", () => {
    expect(parseSessionTimeout(undefined)).toBeUndefined();
  });

  it("parses an in-range duration string", () => {
    expect(parseSessionTimeout("1h")).toBe(3_600_000);
    expect(parseSessionTimeout("90m")).toBe(5_400_000);
  });

  it("rejects below the floor and above the ceiling", () => {
    expect(() => parseSessionTimeout("30s")).toThrow(/at least/);
    expect(() => parseSessionTimeout("9h")).toThrow(/at most/);
  });

  it("accepts the exact bounds", () => {
    expect(parseSessionTimeout(`${MIN_SESSION_TIMEOUT_MS}`)).toBe(MIN_SESSION_TIMEOUT_MS);
    expect(parseSessionTimeout(`${MAX_SESSION_TIMEOUT_MS}`)).toBe(MAX_SESSION_TIMEOUT_MS);
  });

  it("rejects a non-string", () => {
    expect(() => parseSessionTimeout(3600)).toThrow(/duration string/);
  });
});

describe("resolveSessionTimeoutMs + orchestrationTimeoutString", () => {
  it("pins the independent omission default and accepted-input maximum", () => {
    expect(DEFAULT_SESSION_TIMEOUT_MS).toBe(28_800_000);
    expect(MAX_SESSION_TIMEOUT_MS).toBe(28_800_000);
    expect(resolveSessionTimeoutMs(undefined)).toBe(DEFAULT_SESSION_TIMEOUT_MS);
    expect(resolveSessionTimeoutMs(123_000)).toBe(123_000);
  });

  it("accepts the exact maximum and rejects one millisecond above it", () => {
    expect(parseSessionTimeout(`${MAX_SESSION_TIMEOUT_MS}`)).toBe(MAX_SESSION_TIMEOUT_MS);
    expect(() => parseSessionTimeout(`${MAX_SESSION_TIMEOUT_MS + 1}`)).toThrow(/at most/);
  });

  it("formats ms as a second-granularity duration", () => {
    expect(orchestrationTimeoutString(3_600_000)).toBe("3600s");
    expect(orchestrationTimeoutString(1_500)).toBe("2s");
    expect(orchestrationTimeoutString(0)).toBe("1s");
  });
});

describe("graceful termination constants", () => {
  it("pins each grace policy and keeps cleanup later than force-kill", () => {
    expect(SESSION_PROCESS_KILL_GRACE_MS).toBe(60_000);
    expect(SESSION_TERMINAL_GRACE_MS).toBe(90_000);
    expect(SESSION_TERMINAL_GRACE_MS).toBeGreaterThan(SESSION_PROCESS_KILL_GRACE_MS);
  });
});

describe("submission contract — runtimeSize + timeout round-trip", () => {
  it("normalises timeout string to timeoutMs and keeps runtime size token", () => {
    const parsed = parseSessionSubmissionRequest(
      baseRequest({ runtimeSize: RuntimeSizes.CPU_2_8GB, timeout: "2h" })
    );
    expect(parsed.runtimeSize).toBe("2cpu-8gb");
    expect(parsed.timeoutMs).toBe(7_200_000);
  });

  it("omits both when absent", () => {
    const parsed = parseSessionSubmissionRequest(baseRequest());
    expect(parsed.runtimeSize).toBeUndefined();
    expect(parsed.timeoutMs).toBeUndefined();
  });

  it("rejects the old machine field spelling (a runtimeSize string) — machine is now a {spot} object", () => {
    expect(() => parseSessionSubmissionRequest(baseRequest({ machine: "2cpu-8gb" }))).toThrow(
      /machine must be an object/
    );
  });

  it("rejects an invalid runtimeSize token at submit time", () => {
    expect(() => parseSessionSubmissionRequest(baseRequest({ runtimeSize: "shared-9x-1tb" }))).toThrow(
      /runtimeSize must be one of/
    );
  });

  it("rejects an out-of-range timeout at submit time", () => {
    expect(() => parseSessionSubmissionRequest(baseRequest({ timeout: "12h" }))).toThrow(/at most/);
  });
});
