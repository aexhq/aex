import { describe, expect, it } from "vitest";
import { parseSessionSubmissionRequest } from "../src/internal.js";

/**
 * `submission.secretEnv` (value-free declarations, hashed) + `secrets.envSecrets`
 * (vaulted values, hash-excluded) — the wire contract for env-var secrets.
 *
 *   secretEnv[ENV] = { ref: "<handle>" }   → resolved from the workspace store
 *                                            server-side; NO value travels.
 *   secretEnv[ENV] = { ephemeral: true }   → paired with secrets.envSecrets[ENV]
 *                                            (per-session, deleted at terminal).
 *
 * Cross-validation rejects orphan values, and a `ref` never ships a value.
 */
const base = {
  workspaceId: "workspace-1",
  idempotencyKey: "idem-1",
  submission: {
    model: "claude-haiku-4-5",
    prompt: ["hi"],    assets: { files: [], skills: [], tools: [], instructions: [] },
      builtinTools: "default",
    mcpServers: []
  },
  secrets: { apiKeys: { anthropic: "sk-ant-test" } }
} as const;

describe("submission.secretEnv / secrets.envSecrets — contract", () => {
  it("accepts a workspace ref (value resolved server-side; no envSecrets entry)", () => {
    const parsed = parseSessionSubmissionRequest({
      ...base,
      submission: { ...base.submission, secretEnv: { SERPER_API_KEY: { ref: "serper" } } }
    });
    expect(parsed.submission.secretEnv).toEqual({ SERPER_API_KEY: { ref: "serper" } });
    expect(parsed.secrets.envSecrets).toBeUndefined();
  });

  it("accepts an ephemeral declaration paired with its vaulted value", () => {
    const parsed = parseSessionSubmissionRequest({
      ...base,
      submission: { ...base.submission, secretEnv: { SERPER_API_KEY: { ephemeral: true } } },
      secrets: { ...base.secrets, envSecrets: { SERPER_API_KEY: "sk-live-XYZ" } }
    });
    expect(parsed.submission.secretEnv).toEqual({ SERPER_API_KEY: { ephemeral: true } });
    expect(parsed.secrets.envSecrets).toEqual({ SERPER_API_KEY: "sk-live-XYZ" });
  });

  it("rejects an ephemeral declaration with no matching vaulted value", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, secretEnv: { SERPER_API_KEY: { ephemeral: true } } }
      })
    ).toThrow(/no matching secrets\.envSecrets/);
  });

  it("rejects a workspace ref that ALSO supplies a vaulted value", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, secretEnv: { SERPER_API_KEY: { ref: "serper" } } },
        secrets: { ...base.secrets, envSecrets: { SERPER_API_KEY: "sk-live-XYZ" } }
      })
    ).toThrow(/must not supply a value|envSecrets/);
  });

  it("rejects an orphan vaulted value with no secretEnv declaration", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        secrets: { ...base.secrets, envSecrets: { SERPER_API_KEY: "sk-live-XYZ" } }
      })
    ).toThrow(/no matching submission\.secretEnv/);
  });

  it("rejects an invalid env var name", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, secretEnv: { "bad-name": { ref: "serper" } } }
      })
    ).toThrow(/env/i);
  });

  it("rejects an invalid workspace handle", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, secretEnv: { SERPER_API_KEY: { ref: "bad handle!" } } }
      })
    ).toThrow(/handle/i);
  });

  it("rejects an entry that is neither ref nor ephemeral", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, secretEnv: { SERPER_API_KEY: { foo: "x" } } }
      })
    ).toThrow();
  });

  it("rejects ephemeral set to a non-true value", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, secretEnv: { SERPER_API_KEY: { ephemeral: false } } }
      })
    ).toThrow();
  });

  it("rejects a non-string vaulted value", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, secretEnv: { SERPER_API_KEY: { ephemeral: true } } },
        secrets: { ...base.secrets, envSecrets: { SERPER_API_KEY: 123 } }
      })
    ).toThrow();
  });
});
