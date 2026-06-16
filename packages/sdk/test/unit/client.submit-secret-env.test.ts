/**
 * submit() splits `secretEnv: Record<envName, Secret>` exactly like it splits
 * proxyEndpoints: value-free declarations into `submission.secretEnv` (hashed)
 * and ephemeral values into `secrets.envSecrets` (vaulted, hash-excluded).
 */
import { describe, expect, it, vi } from "vitest";
import { AgentExecutor, Secret } from "../../src/index.js";

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
  readonly body: unknown;
}

function makeStubFetch(): { fetch: typeof fetch; calls: CapturedRequest[] } {
  const calls: CapturedRequest[] = [];
  const stub: typeof fetch = vi.fn(async (input, init) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    let body: unknown = init?.body;
    if (typeof body === "string") {
      try {
        body = JSON.parse(body);
      } catch {
        /* leave as string */
      }
    }
    calls.push({ url, method: (init?.method ?? "GET").toString(), body });
    return new Response(JSON.stringify({ id: "run_test", status: "queued" }), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  });
  return { fetch: stub, calls };
}

function submitWith(secretEnv: Record<string, Secret>) {
  const { fetch, calls } = makeStubFetch();
  const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
  return { client, calls, run: () => client.submit({ model: "claude-haiku-4-5", prompt: "p", secrets: { apiKey: "sk-x" }, secretEnv }) };
}

describe("submit() secretEnv split", () => {
  it("workspace ref → submission.secretEnv {ref}; no value travels", async () => {
    const { calls, run } = submitWith({ SERPER_API_KEY: Secret.ref("serper") });
    await run();
    const body = calls[0]!.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    const secrets = body.secrets as Record<string, unknown>;
    expect(submission.secretEnv).toEqual({ SERPER_API_KEY: { ref: "serper" } });
    expect("envSecrets" in secrets).toBe(false);
  });

  it("ephemeral value → submission.secretEnv {ephemeral} + secrets.envSecrets value", async () => {
    const { calls, run } = submitWith({ SERPER_API_KEY: Secret.value("sk-live-XYZ") });
    await run();
    const body = calls[0]!.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    const secrets = body.secrets as Record<string, unknown>;
    expect(submission.secretEnv).toEqual({ SERPER_API_KEY: { ephemeral: true } });
    expect(secrets.envSecrets).toEqual({ SERPER_API_KEY: "sk-live-XYZ" });
  });

  it("the ephemeral value is NEVER in the (hashed) submission half", async () => {
    const { calls, run } = submitWith({ SERPER_API_KEY: Secret.value("sk-live-XYZ") });
    await run();
    const body = calls[0]!.body as Record<string, unknown>;
    expect(JSON.stringify(body.submission)).not.toContain("sk-live-XYZ");
    expect(JSON.stringify(body.secrets)).toContain("sk-live-XYZ");
  });

  it("mixes refs and ephemeral values in one submission", async () => {
    const { calls, run } = submitWith({
      SERPER_API_KEY: Secret.ref("serper"),
      DOUBAO_API_KEYS: Secret.value("ark-secret")
    });
    await run();
    const body = calls[0]!.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    const secrets = body.secrets as Record<string, unknown>;
    expect(submission.secretEnv).toEqual({
      SERPER_API_KEY: { ref: "serper" },
      DOUBAO_API_KEYS: { ephemeral: true }
    });
    expect(secrets.envSecrets).toEqual({ DOUBAO_API_KEYS: "ark-secret" });
  });

  it("omits both fields when secretEnv is not provided", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submit({ model: "claude-haiku-4-5", prompt: "p", secrets: { apiKey: "sk-x" } });
    const body = calls[0]!.body as Record<string, unknown>;
    expect("secretEnv" in (body.submission as object)).toBe(false);
    expect("envSecrets" in (body.secrets as object)).toBe(false);
  });

  it("rejects an invalid env var name", async () => {
    const { run } = submitWith({ "bad-name": Secret.ref("serper") });
    await expect(run()).rejects.toThrow(/env var name/i);
  });

  it("rejects a non-Secret value", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submit({
        model: "claude-haiku-4-5",
        prompt: "p",
        secrets: { apiKey: "sk-x" },
        secretEnv: { SERPER_API_KEY: "sk-x" as unknown as Secret }
      })
    ).rejects.toThrow(/must be a Secret/);
  });
});
