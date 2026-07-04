import { describe, expect, it } from "vitest";
import { Aex, CredentialValidationError, RunConfigValidationError } from "../../src/index.js";

function recordingFetch(): { fetch: typeof fetch; calls: string[] } {
  const calls: string[] = [];
  const f: typeof fetch = async (input) => {
    calls.push(typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url);
    return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });
  };
  return { fetch: f, calls };
}

describe("Aex.openSession — removed field validation", () => {
  it("rejects the legacy runtimeSize field without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.openSession({
        runtimeSize: "shared-1x-4gb",
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" }
      } as never)
    ).rejects.toThrow(/runtimeSize is not a supported option; use runtime/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects the legacy secretEnv field without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        secretEnv: { SERPER_API_KEY: { ref: "serper" } }
      } as never)
    ).rejects.toThrow(/secretEnv is not a supported option; use environment\.secrets/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects the legacy nested secrets object without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        secrets: { apiKeys: { anthropic: "sk-x" } }
      } as never)
    ).rejects.toThrow(/secrets is not a supported option/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects a message field without an HTTP call (was silently dropped: empty session, no turn)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        message: "hello there"
      } as never)
    ).rejects.toThrow(/message is not a supported option; sessions are created without a first message/);

    expect(rec.calls).toHaveLength(0);
  });
});

describe("Aex.openSession — submit-boundary validation (Theme A, pre-network)", () => {
  it("rejects an invalid runtime token without an HTTP call (F11)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        runtime: "lite"
      } as never)
    ).rejects.toThrow(RunConfigValidationError);
    expect(rec.calls).toHaveLength(0);
  });

  it("rejects a malformed overrides.timeout without an HTTP call (F12)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        overrides: { timeout: "banana" }
      })
    ).rejects.toThrow(RunConfigValidationError);
    expect(rec.calls).toHaveLength(0);
  });

  it("rejects an out-of-range timeout (below the 1m floor) without an HTTP call (F12)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        overrides: { timeout: "10s" }
      })
    ).rejects.toThrow(RunConfigValidationError);
    expect(rec.calls).toHaveLength(0);
  });

  it("accepts a valid runtime + timeout (regression: does not over-reject)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "sk-x" },
      runtime: "shared-0.5x-4gb",
      overrides: { timeout: "30m" }
    });
    // A valid config DOES reach the network (create call).
    expect(rec.calls.length).toBeGreaterThan(0);
  });
});

describe("new Aex(...) — credential validation (F2)", () => {
  it("throws a typed CredentialValidationError (an AexError), not a bare Error, on a missing credential", () => {
    expect(() => new Aex({} as never)).toThrow(CredentialValidationError);
    try {
      new Aex({} as never);
    } catch (err) {
      // A caller catching the SDK error base must catch this too.
      expect(err).toBeInstanceOf(CredentialValidationError);
      expect((err as CredentialValidationError).code).toBe("CREDENTIAL_INVALID");
    }
  });
});
