import { describe, expect, it } from "vitest";
import {
  DEFAULT_MACHINE_SIZE,
  DEFAULT_RUN_TIMEOUT_MS,
  inngestTimeoutString,
  MACHINE_PRESETS,
  MACHINE_SIZES,
  machineResources,
  MachineSizes,
  MAX_RUN_TIMEOUT_MS,
  MIN_RUN_TIMEOUT_MS,
  parseDurationToMs,
  parseMachineSize,
  parseRunSubmissionRequest,
  parseRunTimeout,
  resolveRunTimeoutMs,
  RUN_GOOSE_KILL_GRACE_MS,
  RUN_GRACE_MS
} from "../src/index.js";

// Same canonical submission helper shape submission.test.ts uses, trimmed to
// what these tests need — a minimal Anthropic-managed request the parser
// accepts, with `machine`/`timeout` overridable.
function baseRequest(overrides: Record<string, unknown> = {}) {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider: "anthropic",
    submission: { model: "model-x", prompt: ["hello"], skills: [], agentsMd: [], files: [], mcpServers: [] },
    secrets: { anthropic: { apiKey: "sk-anthropic-test" } },
    ...overrides
  };
}

describe("machine presets — every offered combo is a valid Fly shared-CPU guest", () => {
  // Fly's rule: memory must be a multiple of 256MB, with
  // min = 256 * cpus and max = 2048 * cpus. If a preset ever violates this,
  // it would be rejected at machine-create time — exactly the error class
  // this closed set exists to eliminate.
  it.each(MACHINE_SIZES)("%s sits inside Fly's valid range", (size) => {
    const { cpus, memoryMb } = MACHINE_PRESETS[size];
    expect([1, 2, 4, 8]).toContain(cpus);
    expect(memoryMb % 256).toBe(0);
    expect(memoryMb).toBeGreaterThanOrEqual(256 * cpus);
    expect(memoryMb).toBeLessThanOrEqual(2048 * cpus);
  });

  it("has no duplicate (cpu, memory) pairs", () => {
    const seen = new Set(MACHINE_SIZES.map((s) => `${MACHINE_PRESETS[s].cpus}x${MACHINE_PRESETS[s].memoryMb}`));
    expect(seen.size).toBe(MACHINE_SIZES.length);
  });
});

describe("MachineSizes symbol const stays in lockstep with MACHINE_PRESETS", () => {
  it("maps every symbol to an existing preset token, and covers all of them", () => {
    const symbolValues = Object.values(MachineSizes);
    // Every symbol resolves to a real preset key…
    for (const token of symbolValues) {
      expect(MACHINE_SIZES).toContain(token);
    }
    // …and every preset has a symbol (no preset is unreachable via MachineSizes).
    expect([...symbolValues].sort()).toEqual([...MACHINE_SIZES].sort());
    // No two symbols point at the same token.
    expect(new Set(symbolValues).size).toBe(symbolValues.length);
  });
});

describe("default machine size is unchanged platform behaviour (1 cpu / 512MB)", () => {
  it("resolves to the pre-existing Goose runner size", () => {
    expect(DEFAULT_MACHINE_SIZE).toBe(MachineSizes.SHARED_1X_512MB);
    expect(machineResources(DEFAULT_MACHINE_SIZE)).toEqual({ cpus: 1, memoryMb: 512 });
  });
});

describe("parseMachineSize", () => {
  it("accepts an omitted field", () => {
    expect(parseMachineSize(undefined)).toBeUndefined();
  });

  it("accepts every preset token", () => {
    for (const size of MACHINE_SIZES) {
      expect(parseMachineSize(size)).toBe(size);
    }
  });

  it("rejects an unknown token with the full menu", () => {
    expect(() => parseMachineSize("shared-3x-7gb")).toThrow(/must be one of/);
    expect(() => parseMachineSize("shared-2x-3gb")).toThrow(/must be one of/); // valid-looking but not offered
    expect(() => parseMachineSize(512)).toThrow(/must be one of/);
  });
});

describe("parseDurationToMs", () => {
  it("parses each unit", () => {
    expect(parseDurationToMs("1h")).toBe(3_600_000);
    expect(parseDurationToMs("90m")).toBe(5_400_000);
    expect(parseDurationToMs("30s")).toBe(30_000);
    expect(parseDurationToMs("500ms")).toBe(500);
    expect(parseDurationToMs("1000")).toBe(1000); // bare ms
  });

  it("throws on malformed input", () => {
    expect(() => parseDurationToMs("soon")).toThrow(/invalid duration/);
    expect(() => parseDurationToMs("1d")).toThrow(/invalid duration/);
  });
});

describe("parseRunTimeout — bounded to [1m, 6h]", () => {
  it("accepts an omitted field", () => {
    expect(parseRunTimeout(undefined)).toBeUndefined();
  });

  it("parses an in-range duration string", () => {
    expect(parseRunTimeout("1h")).toBe(3_600_000);
    expect(parseRunTimeout("90m")).toBe(5_400_000);
  });

  it("rejects below the floor and above the ceiling", () => {
    expect(() => parseRunTimeout("30s")).toThrow(/at least/);
    expect(() => parseRunTimeout("7h")).toThrow(/at most/);
  });

  it("accepts the exact bounds", () => {
    expect(parseRunTimeout(`${MIN_RUN_TIMEOUT_MS}`)).toBe(MIN_RUN_TIMEOUT_MS);
    expect(parseRunTimeout(`${MAX_RUN_TIMEOUT_MS}`)).toBe(MAX_RUN_TIMEOUT_MS);
  });

  it("rejects a non-string", () => {
    expect(() => parseRunTimeout(3600)).toThrow(/duration string/);
  });
});

describe("resolveRunTimeoutMs + inngestTimeoutString", () => {
  it("applies the 1h default only when absent", () => {
    expect(resolveRunTimeoutMs(undefined)).toBe(DEFAULT_RUN_TIMEOUT_MS);
    expect(DEFAULT_RUN_TIMEOUT_MS).toBe(60 * 60 * 1000);
    expect(resolveRunTimeoutMs(123_000)).toBe(123_000);
  });

  it("formats ms as a second-granularity Inngest duration", () => {
    expect(inngestTimeoutString(3_600_000)).toBe("3600s");
    expect(inngestTimeoutString(1_500)).toBe("2s"); // rounds up, never 0
    expect(inngestTimeoutString(0)).toBe("1s");
  });
});

describe("graceful-termination grace constants", () => {
  it("orchestrator grace outlasts the runner's goose kill-grace", () => {
    // The orchestrator must wait longer than the runner's SIGINT→SIGKILL
    // window, or it would tear down the machine while the runner is still
    // uploading outputs after the kill. The difference is the upload budget.
    expect(RUN_GRACE_MS).toBeGreaterThan(RUN_GOOSE_KILL_GRACE_MS);
    expect(RUN_GOOSE_KILL_GRACE_MS).toBe(60 * 1000);
    expect(RUN_GRACE_MS).toBe(90 * 1000);
  });
});

describe("submission contract — machine + timeout round-trip", () => {
  it("normalises timeout string → timeoutMs and keeps machine token", () => {
    const parsed = parseRunSubmissionRequest(
      baseRequest({ machine: MachineSizes.SHARED_2X_2GB, timeout: "2h" })
    );
    expect(parsed.machine).toBe("shared-2x-2gb");
    expect(parsed.timeoutMs).toBe(7_200_000);
  });

  it("omits both when absent (default applied downstream, not in the parsed request)", () => {
    const parsed = parseRunSubmissionRequest(baseRequest());
    expect(parsed.machine).toBeUndefined();
    expect(parsed.timeoutMs).toBeUndefined();
  });

  it("rejects an invalid machine token at submit time", () => {
    expect(() => parseRunSubmissionRequest(baseRequest({ machine: "shared-9x-1tb" }))).toThrow(
      /must be one of/
    );
  });

  it("rejects an out-of-range timeout at submit time", () => {
    expect(() => parseRunSubmissionRequest(baseRequest({ timeout: "12h" }))).toThrow(/at most/);
  });
});
