/**
 * sessions.create splits `environment.secrets: Record<envName, Secret>` exactly like
 * other value-free declarations: `submission.secretEnv` is hashed while
 * ephemeral values live in `secrets.envSecrets` (vaulted, hash-excluded).
 */
import { describe, expect, it, mock } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex, Secret, SessionConfigValidationError } from "../../src/index.js";

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
  readonly body: unknown;
}

function makeStubFetch(): { fetch: FetchLike; calls: CapturedRequest[] } {
  const calls: CapturedRequest[] = [];
  const stub: FetchLike = mock(async (input, init) => {
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
    return new Response(JSON.stringify({ session: { id: "ses_test", status: "idle", acceptsMessages: true } }), {
      status: 201,
      headers: { "content-type": "application/json" }
    });
  });
  return { fetch: stub, calls };
}

function openWith(secrets: Record<string, Secret>) {
  const { fetch, calls } = makeStubFetch();
  const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
  return {
    client,
    calls,
    run: () =>
      client.sessions.create({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        environment: { secrets }
      })
  };
}

async function rejected(operation: () => Promise<unknown>): Promise<unknown> {
  return operation().then(
    () => undefined,
    (error: unknown) => error
  );
}

function expectConfigError(error: unknown, field: string): void {
  expect(error).toBeInstanceOf(SessionConfigValidationError);
  expect(error).toMatchObject({ name: "SessionConfigValidationError", code: "SESSION_CONFIG_INVALID" });
  expect((error as SessionConfigValidationError).details).toEqual({ field });
  expect((error as Error).message.trim().length).toBeGreaterThan(0);
}

describe("sessions.create environment.secrets split", () => {
  it("workspace ref → submission.secretEnv {ref}; no value travels", async () => {
    const { calls, run } = openWith({ SERPER_API_KEY: Secret.ref("serper") });
    await run();
    const body = calls[0]!.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    const secrets = body.secrets as Record<string, unknown>;
    expect(submission.secretEnv).toEqual({ SERPER_API_KEY: { ref: "serper" } });
    expect("envSecrets" in secrets).toBe(false);
  });

  it("ephemeral value → submission.secretEnv {ephemeral} + secrets.envSecrets value", async () => {
    const { calls, run } = openWith({ SERPER_API_KEY: Secret.value("sk-live-XYZ") });
    await run();
    const body = calls[0]!.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    const secrets = body.secrets as Record<string, unknown>;
    expect(submission.secretEnv).toEqual({ SERPER_API_KEY: { ephemeral: true } });
    expect(secrets.envSecrets).toEqual({ SERPER_API_KEY: "sk-live-XYZ" });
  });

  it("the ephemeral value is NEVER in the (hashed) submission half", async () => {
    const { calls, run } = openWith({ SERPER_API_KEY: Secret.value("sk-live-XYZ") });
    await run();
    const body = calls[0]!.body as Record<string, unknown>;
    expect(JSON.stringify(body.submission)).not.toContain("sk-live-XYZ");
    expect(JSON.stringify(body.secrets)).toContain("sk-live-XYZ");
  });

  it("mixes refs and ephemeral values in one submission", async () => {
    const { calls, run } = openWith({
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

  it("omits both fields when environment.secrets is not provided", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await client.sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-x" } });
    const body = calls[0]!.body as Record<string, unknown>;
    expect("secretEnv" in (body.submission as object)).toBe(false);
    expect("envSecrets" in (body.secrets as object)).toBe(false);
  });

  it("rejects an invalid env var name", async () => {
    const { calls, run } = openWith({ "bad-name": Secret.ref("serper") });
    expectConfigError(await rejected(run), "environment.secrets");
    expect(calls).toHaveLength(0);
  });

  it("rejects a non-Secret value", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    const error = await rejected(() => client.sessions.create({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        environment: { secrets: { SERPER_API_KEY: "sk-x" as unknown as Secret } }
      }));
    expectConfigError(error, "environment.secrets");
    expect(calls).toHaveLength(0);
  });
});
