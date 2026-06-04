import { describe, expect, it } from "vitest";
import { parseRunSubmissionRequest } from "../src/index.js";

// Minimal valid submission envelope. Tests below only override the
// `submission.environment` slice to exercise the new envVars parser.
const baseSubmission = {
  workspaceId: "workspace-1",
  idempotencyKey: "idem-1",
  submission: {
    model: "claude-haiku-4-5",
    prompt: ["say hello"],
    skills: [],
    agentsMd: [],
    files: [],
    mcpServers: []
  },
  secrets: {
    anthropic: { apiKey: "sk-ant-test" }
  }
} as const;

function submit(environment: Record<string, unknown>): ReturnType<typeof parseRunSubmissionRequest> {
  return parseRunSubmissionRequest({
    ...baseSubmission,
    submission: { ...baseSubmission.submission, environment }
  });
}

describe("submission.environment.envVars — additive surface", () => {
  it("accepts a well-formed envVars map and preserves insertion order", () => {
    const parsed = submit({
      envVars: {
        BROLL_STORE: "/mnt/session/broll/store",
        BROLL_OUTPUTS: "/mnt/session/outputs"
      }
    });
    expect(parsed.submission.environment?.envVars).toEqual({
      BROLL_STORE: "/mnt/session/broll/store",
      BROLL_OUTPUTS: "/mnt/session/outputs"
    });
    expect(Object.keys(parsed.submission.environment!.envVars!)).toEqual([
      "BROLL_STORE",
      "BROLL_OUTPUTS"
    ]);
  });

  it("composes with networking and packages on the same environment object", () => {
    const parsed = submit({
      networking: { mode: "open" },
      envVars: { BROLL_STORE: "/mnt/session/broll/store" }
    });
    expect(parsed.submission.environment?.networking?.mode).toBe("open");
    expect(parsed.submission.environment?.envVars).toEqual({
      BROLL_STORE: "/mnt/session/broll/store"
    });
  });

  it("treats an empty envVars object as not supplied", () => {
    const parsed = submit({ envVars: {} });
    expect(parsed.submission.environment).toBeUndefined();
  });

  it("rejects lowercase or non-POSIX keys", () => {
    expect(() => submit({ envVars: { brollStore: "/x" } })).toThrow(
      /envVars\.brollStore key must match/
    );
    expect(() => submit({ envVars: { "1BAD": "/x" } })).toThrow(
      /envVars\.1BAD key must match/
    );
    expect(() => submit({ envVars: { "BAD-KEY": "/x" } })).toThrow(
      /envVars\.BAD-KEY key must match/
    );
  });

  it("rejects keys using the reserved AEX_ prefix", () => {
    expect(() => submit({ envVars: { AEX_OUTPUTS: "/elsewhere" } })).toThrow(
      /uses reserved prefix "AEX_"/
    );
  });

  it("rejects non-string values", () => {
    expect(() => submit({ envVars: { K: 1 as unknown as string } })).toThrow(
      /envVars\.K must be a string/
    );
    expect(() => submit({ envVars: { K: null as unknown as string } })).toThrow(
      /envVars\.K must be a string/
    );
  });

  it("rejects NUL bytes in values", () => {
    expect(() => submit({ envVars: { K: "no\0nul" } })).toThrow(
      /envVars\.K must not contain NUL bytes/
    );
  });

  it("rejects a value larger than the per-value cap", () => {
    const huge = "x".repeat(5000);
    expect(() => submit({ envVars: { K: huge } })).toThrow(/maximum is 4096/);
  });

  it("rejects more than ENV_VARS_MAX_ENTRIES entries", () => {
    const bag: Record<string, string> = {};
    for (let i = 0; i <= 64; i++) {
      bag[`K${i}`] = "v";
    }
    expect(() => submit({ envVars: bag })).toThrow(/maximum is 64/);
  });

  it("rejects a total payload larger than the overall cap", () => {
    // 32 keys * 2KiB each = 64KiB — overshoots the 64 KiB total cap
    // once key bytes are included.
    const bag: Record<string, string> = {};
    const big = "x".repeat(2048);
    for (let i = 0; i < 32; i++) {
      bag[`LONGKEYNAME${i}`] = big;
    }
    expect(() => submit({ envVars: bag })).toThrow(/total byte size exceeds maximum/);
  });

  it("rejects unknown sibling fields with a message listing all permitted", () => {
    expect(() =>
      submit({
        envVars: { K: "v" },
        bogus: 1
      })
    ).toThrow(/permitted: networking, packages, envVars/);
  });
});
