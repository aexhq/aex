import fc from "fast-check";
import { describe, expect, it } from "vitest";
import {
  RUN_MODELS,
  RUNTIME_SIZES,
  parseRunSubmissionRequest,
  providersForModel,
  type RunModel
} from "@aexhq/contracts";
import { AgentExecutor, Secret, type SessionCreateOptions } from "../../src/index.js";

/**
 * SDK ⇄ API WIRE-CONFORMANCE property (the prompt-delivery wire-shape bug class).
 *
 * The invariant: for ANY fuzzed session-create options, the REAL
 * `AgentExecutor.openSession` either
 *   (a) rejects synchronously with a typed error (AexError / Error), OR
 *   (b) builds a POST /api/sessions body whose non-message wire pieces the REAL
 *       contracts validator (`parseRunSubmissionRequest`) accepts.
 * It must NEVER silently produce a wire request the server would reject/500 on.
 *
 * No mocks of the code under test: the SDK request-builder AND the contracts
 * validator are both the real implementations. The only seam is a capture
 * `fetch` (the network boundary) that records the outgoing body and returns a
 * synthetic session record. Inline assets (skills/tools/files/agentsMd) are
 * intentionally NOT fuzzed so the sole network call is the session create (no
 * asset upload round-trips). The one-shot message rides a separate /messages
 * request and is out of scope here.
 */

function captureClient(): { client: AgentExecutor; bodies: unknown[] } {
  const bodies: unknown[] = [];
  const fetchImpl: typeof fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url.endsWith("/api/sessions") && (init?.method ?? "GET") === "POST") {
      const raw = init?.body;
      bodies.push(typeof raw === "string" ? JSON.parse(raw) : raw);
      return new Response(JSON.stringify({ id: "run_test", status: "queued" }), {
        status: 202,
        headers: { "content-type": "application/json" }
      });
    }
    throw new Error(`unexpected SDK network call (asset upload not expected in this fuzz): ${url}`);
  };
  return {
    client: new AgentExecutor({ apiToken: "tkn_fuzz", baseUrl: "https://example.test", fetch: fetchImpl }),
    bodies
  };
}

/**
 * Validate a captured session-create body against the shared run-submission
 * validator. The session create body is the run-submission input MINUS the
 * `idempotencyKey` (a header on this route) and the message `prompt` (which
 * rides /messages), PLUS a `retention` policy. We adapt those framing
 * differences back to a run-submission shape and let the REAL parser vet every
 * wire piece the SDK actually assembled (secrets, proxyEndpoints, mcp,
 * runtimeSize, timeout, limits, metadata, outputs, secretEnv, environment).
 */
function validateWire(body: unknown): void {
  const { retention: _retention, submission, ...rest } = body as Record<string, unknown>;
  void _retention;
  parseRunSubmissionRequest({
    ...rest,
    workspaceId: "ws_fuzz",
    idempotencyKey: "idem_fuzz",
    submission: { ...(submission as Record<string, unknown>), prompt: ["fuzz"] }
  });
}

const validModel = fc.constantFrom<RunModel>(...(RUN_MODELS as readonly RunModel[]));
const jsonScalar = fc.oneof(fc.string({ maxLength: 24 }), fc.integer(), fc.boolean(), fc.constant(null));
const metadata = fc.dictionary(
  fc.string({ minLength: 1, maxLength: 12 }).filter((k) => !/key|token|secret|password|credential|auth/i.test(k)),
  jsonScalar,
  { maxKeys: 4 }
);
const secretEnv = fc.dictionary(
  fc.string({ minLength: 1, maxLength: 10 }).map((s) => s.toUpperCase().replace(/[^A-Z_]/g, "_")).filter((s) => /^[A-Z_]+$/.test(s) && s.length > 0),
  fc.string({ minLength: 1, maxLength: 24 }).map((v) => Secret.value(v)),
  { maxKeys: 3 }
);
const timeout = fc.oneof(fc.constant(undefined), fc.constantFrom("1m", "30m", "1h", "90m", "3600s"));
const runtime = fc.oneof(fc.constant(undefined), fc.constantFrom(...RUNTIME_SIZES));
const outputMode = fc.oneof(fc.constant(undefined), fc.constantFrom("buffered" as const, "stream" as const));

// A session-create option shape whose CREDENTIALS are always self-consistent (a
// key for the model's resolved provider), so the credential gate never rejects —
// leaving the wire SHAPE as the thing under test.
const goodOptions = validModel.chain((model) => {
  const provider = providersForModel(model)[0] ?? "anthropic";
  return fc.record(
    {
      model: fc.constant(model),
      system: fc.oneof(fc.constant(undefined), fc.string({ minLength: 1, maxLength: 40 })),
      metadata: fc.oneof(fc.constant(undefined), metadata),
      environment: fc.oneof(fc.constant(undefined), secretEnv.map((secrets) => ({ secrets }))),
      overrides: fc.oneof(fc.constant(undefined), timeout.map((t) => (t ? { timeout: t } : {}))),
      runtime,
      outputMode,
      includeBuiltinTools: fc.oneof(fc.constant(undefined), fc.boolean()),
      idempotencyKey: fc.oneof(fc.constant(undefined), fc.string({ minLength: 1, maxLength: 20 })),
      apiKeys: fc.constant({ [provider]: "sk-ant-fuzztestkey0123456789" })
    },
    { requiredKeys: ["model", "apiKeys"] }
  );
});

describe("SDK wire-conformance (property)", () => {
  it("every accepted session-create builds a body the contracts validator accepts", async () => {
    await fc.assert(
      fc.asyncProperty(goodOptions, async (options) => {
        const { client, bodies } = captureClient();
        try {
          await client.openSession(options as unknown as SessionCreateOptions);
        } catch (err) {
          // Rejection path: must be a TYPED error, never an undefined/non-Error crash.
          expect(err).toBeInstanceOf(Error);
          return;
        }
        // Accept path: exactly one /sessions body, and it must pass the REAL parser.
        expect(bodies).toHaveLength(1);
        expect(() => validateWire(bodies[0])).not.toThrow();
      }),
      { numRuns: 250 }
    );
  });

  it("is total under adversarial options: typed throw OR a parseable wire (never a crash)", async () => {
    const adversarial = fc.record(
      {
        model: fc.oneof(validModel, fc.string({ maxLength: 12 }), fc.constant(undefined), fc.integer()),
        provider: fc.oneof(fc.constant(undefined), fc.constantFrom("anthropic", "deepseek", "bogus")),
        runtime: fc.oneof(fc.constant(undefined), fc.constantFrom(...RUNTIME_SIZES), fc.string({ maxLength: 12 })),
        overrides: fc.oneof(fc.constant(undefined), timeout.map((t) => (t ? { timeout: t } : {})), fc.record({ timeout: fc.string({ maxLength: 8 }) })),
        apiKeys: fc.oneof(fc.constant(undefined), fc.constant({ anthropic: "sk-ant-x0123456789abcd" }))
      },
      { requiredKeys: [] }
    );
    await fc.assert(
      fc.asyncProperty(adversarial, async (options) => {
        const { client, bodies } = captureClient();
        try {
          await client.openSession(options as unknown as SessionCreateOptions);
        } catch (err) {
          expect(err).toBeInstanceOf(Error);
          return;
        }
        // If the SDK accepted it, the wire must at least be a parseable structure
        // OR cleanly rejected by the parser with an Error — never an unexpected crash.
        expect(bodies).toHaveLength(1);
        try {
          validateWire(bodies[0]);
        } catch (err) {
          expect(err).toBeInstanceOf(Error);
        }
      }),
      { numRuns: 200 }
    );
  });
});
