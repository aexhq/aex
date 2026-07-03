import { describe, expect, it } from "vitest";
import {
  DEFAULT_RUN_TIMEOUT_MS,
  DEFAULT_RUNTIME_SIZE,
  MAX_RUN_TIMEOUT_MS,
  MIN_RUN_TIMEOUT_MS,
  RUNTIME_SIZE_PRESETS,
  RUNTIME_SIZES,
  RUN_PROCESS_KILL_GRACE_MS,
  RUN_TERMINAL_GRACE_MS,
  RuntimeSizes,
  Models,
  orchestrationTimeoutString,
  parseDurationToMs,
  parseRunTimeout,
  parseRuntimeSize,
  parseRunSubmissionRequest,
  resolveRunTimeoutMs,
  runtimeResources
} from "../src/index.js";

function baseRequest(overrides: Record<string, unknown> = {}) {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider: "anthropic",
    submission: { model: Models.CLAUDE_HAIKU_4_5, prompt: ["hello"], agentsMd: [], files: [], mcpServers: [] },
    secrets: { apiKeys: { anthropic: "sk-anthropic-test" } },
    ...overrides
  };
}

describe("runtime size presets", () => {
  it("exposes exactly the six managed runtime preset tokens", () => {
    expect([...RUNTIME_SIZES]).toEqual([
      "shared-0.06x-256mb",
      "shared-0.25x-1gb",
      "shared-0.5x-4gb",
      "shared-1x-6gb",
      "shared-2x-8gb",
      "shared-4x-12gb"
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

describe("default runtime size", () => {
  it("resolves to the current default runtime resource preset", () => {
    expect(DEFAULT_RUNTIME_SIZE).toBe(RuntimeSizes.SHARED_0_25X_1GB);
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

describe("parseRunTimeout", () => {
  it("accepts an omitted field", () => {
    expect(parseRunTimeout(undefined)).toBeUndefined();
  });

  it("parses an in-range duration string", () => {
    expect(parseRunTimeout("1h")).toBe(3_600_000);
    expect(parseRunTimeout("90m")).toBe(5_400_000);
  });

  it("rejects below the floor and above the ceiling", () => {
    expect(() => parseRunTimeout("30s")).toThrow(/at least/);
    expect(() => parseRunTimeout("9h")).toThrow(/at most/);
  });

  it("accepts the exact bounds", () => {
    expect(parseRunTimeout(`${MIN_RUN_TIMEOUT_MS}`)).toBe(MIN_RUN_TIMEOUT_MS);
    expect(parseRunTimeout(`${MAX_RUN_TIMEOUT_MS}`)).toBe(MAX_RUN_TIMEOUT_MS);
  });

  it("rejects a non-string", () => {
    expect(() => parseRunTimeout(3600)).toThrow(/duration string/);
  });
});

describe("resolveRunTimeoutMs + orchestrationTimeoutString", () => {
  it("applies the 8h default only when absent", () => {
    expect(resolveRunTimeoutMs(undefined)).toBe(DEFAULT_RUN_TIMEOUT_MS);
    expect(DEFAULT_RUN_TIMEOUT_MS).toBe(8 * 60 * 60 * 1000);
    expect(resolveRunTimeoutMs(123_000)).toBe(123_000);
  });

  it("formats ms as a second-granularity duration", () => {
    expect(orchestrationTimeoutString(3_600_000)).toBe("3600s");
    expect(orchestrationTimeoutString(1_500)).toBe("2s");
    expect(orchestrationTimeoutString(0)).toBe("1s");
  });
});

describe("graceful termination constants", () => {
  it("orchestrator grace outlasts runtime process kill grace", () => {
    expect(RUN_TERMINAL_GRACE_MS).toBeGreaterThan(RUN_PROCESS_KILL_GRACE_MS);
    expect(RUN_PROCESS_KILL_GRACE_MS).toBe(60 * 1000);
    expect(RUN_TERMINAL_GRACE_MS).toBe(90 * 1000);
  });
});

describe("submission contract — runtimeSize + timeout round-trip", () => {
  it("normalises timeout string to timeoutMs and keeps runtime size token", () => {
    const parsed = parseRunSubmissionRequest(
      baseRequest({ runtimeSize: RuntimeSizes.SHARED_2X_8GB, timeout: "2h" })
    );
    expect(parsed.runtimeSize).toBe("shared-2x-8gb");
    expect(parsed.timeoutMs).toBe(7_200_000);
  });

  it("omits both when absent", () => {
    const parsed = parseRunSubmissionRequest(baseRequest());
    expect(parsed.runtimeSize).toBeUndefined();
    expect(parsed.timeoutMs).toBeUndefined();
  });

  it("rejects the old machine field spelling (a runtimeSize string) — machine is now a {spot} object", () => {
    expect(() => parseRunSubmissionRequest(baseRequest({ machine: "shared-2x-8gb" }))).toThrow(
      /machine must be an object/
    );
  });

  it("rejects an invalid runtimeSize token at submit time", () => {
    expect(() => parseRunSubmissionRequest(baseRequest({ runtimeSize: "shared-9x-1tb" }))).toThrow(
      /runtimeSize must be one of/
    );
  });

  it("rejects an out-of-range timeout at submit time", () => {
    expect(() => parseRunSubmissionRequest(baseRequest({ timeout: "12h" }))).toThrow(/at most/);
  });
});
