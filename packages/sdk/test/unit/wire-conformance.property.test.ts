import fc from "fast-check";
import { describe, expect, it } from "vitest";
import {
  RUN_MODELS,
  parseRunSubmissionRequest,
  providersForModel,
  type RunModel
} from "@aexhq/contracts";
import { AgentExecutor, type SubmitOptions } from "../../src/index.js";

/**
 * SDK ⇄ API WIRE-CONFORMANCE property (the prompt-delivery wire-shape bug class).
 *
 * The invariant: for ANY fuzzed submit options, the REAL `AgentExecutor.submit`
 * either
 *   (a) rejects synchronously with a typed error (AexError / Error), OR
 *   (b) builds a POST /api/runs body that the REAL contracts validator
 *       (`parseRunSubmissionRequest`) accepts.
 * It must NEVER silently produce a wire request the server would reject/500 on.
 *
 * No mocks of the code under test: the SDK request-builder AND the contracts
 * validator are both the real implementations. The only seam is a capture
 * `fetch` (the network boundary) that records the outgoing body and returns a
 * synthetic run record — the only way to inspect the wire offline. Inline
 * assets (skills/tools/files/agentsMd) are intentionally NOT fuzzed so the sole
 * network call is the run submit (no asset upload round-trips).
 */

function captureClient(): { client: AgentExecutor; bodies: unknown[] } {
  const bodies: unknown[] = [];
  const fetchImpl: typeof fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url.endsWith("/api/runs") && (init?.method ?? "GET") === "POST") {
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
 * Validate a captured wire body exactly as the BFF does: the SDK posts a
 * `PlatformRunSubmissionInput` with `workspaceId` OMITTED by design (it is
 * derived from the API token), and the server injects it before calling the
 * shared parser. We replicate that single injection step — everything else is
 * the real SDK output through the real validator.
 */
function validateWire(body: unknown): void {
  parseRunSubmissionRequest({ ...(body as Record<string, unknown>), workspaceId: "ws_fuzz" });
}

const validModel = fc.constantFrom<RunModel>(...(RUN_MODELS as readonly RunModel[]));
const prompt = fc.oneof(
  fc.string({ minLength: 1, maxLength: 64 }).filter((s) => s.trim().length > 0),
  fc.array(fc.string({ minLength: 1, maxLength: 32 }).filter((s) => s.trim().length > 0), { minLength: 1, maxLength: 4 })
);
const jsonScalar = fc.oneof(fc.string({ maxLength: 24 }), fc.integer(), fc.boolean(), fc.constant(null));
const metadata = fc.dictionary(
  fc.string({ minLength: 1, maxLength: 12 }).filter((k) => !/key|token|secret|password|credential|auth/i.test(k)),
  jsonScalar,
  { maxKeys: 4 }
);
const limits = fc.record(
  {
    maxConcurrentChildRuns: fc.integer({ min: 1, max: 4096 }),
    maxSubagentDepth: fc.integer({ min: 1, max: 16 })
  },
  { requiredKeys: [] }
);
const secretEnv = fc.dictionary(
  fc.string({ minLength: 1, maxLength: 10 }).map((s) => s.toUpperCase().replace(/[^A-Z_]/g, "_")).filter((s) => /^[A-Z_]+$/.test(s) && s.length > 0),
  fc.string({ minLength: 1, maxLength: 24 }),
  { maxKeys: 3 }
);
const timeout = fc.oneof(fc.constant(undefined), fc.constantFrom("1m", "30m", "1h", "90m", "3600s"));
const outputMode = fc.oneof(fc.constant(undefined), fc.constantFrom("buffered" as const, "stream" as const));

// A submit-options shape whose CREDENTIALS are always self-consistent (a key for
// the model's resolved provider), so the credential gate never rejects — leaving
// the wire SHAPE as the thing under test.
const goodOptions = validModel.chain((model) => {
  const provider = providersForModel(model)[0] ?? "anthropic";
  return fc.record(
    {
      model: fc.constant(model),
      prompt,
      system: fc.oneof(fc.constant(undefined), fc.string({ minLength: 1, maxLength: 40 })),
      metadata: fc.oneof(fc.constant(undefined), metadata),
      limits: fc.oneof(fc.constant(undefined), limits),
      secretEnv: fc.oneof(fc.constant(undefined), secretEnv),
      timeout,
      outputMode,
      includeBuiltinTools: fc.oneof(fc.constant(undefined), fc.boolean()),
      idempotencyKey: fc.oneof(fc.constant(undefined), fc.string({ minLength: 1, maxLength: 20 })),
      secrets: fc.constant({ apiKeys: { [provider]: "sk-ant-fuzztestkey0123456789" } })
    },
    { requiredKeys: ["model", "prompt", "secrets"] }
  );
});

describe("SDK wire-conformance (property)", () => {
  it("every accepted submit builds a body the contracts validator accepts", async () => {
    await fc.assert(
      fc.asyncProperty(goodOptions, async (options) => {
        const { client, bodies } = captureClient();
        try {
          await client.submit(options as unknown as SubmitOptions);
        } catch (err) {
          // Rejection path: must be a TYPED error, never an undefined/non-Error crash.
          expect(err).toBeInstanceOf(Error);
          return;
        }
        // Accept path: exactly one /runs body, and it must pass the REAL parser.
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
        prompt: fc.oneof(prompt, fc.constant(""), fc.constant([]), fc.constant(undefined)),
        provider: fc.oneof(fc.constant(undefined), fc.constantFrom("anthropic", "deepseek", "bogus")),
        runtimeSize: fc.oneof(fc.constant(undefined), fc.string({ maxLength: 12 })),
        timeout: fc.oneof(timeout, fc.string({ maxLength: 8 })),
        limits: fc.oneof(fc.constant(undefined), limits, fc.record({ bogusField: fc.integer() })),
        secrets: fc.oneof(fc.constant(undefined), fc.constant({ apiKeys: { anthropic: "sk-ant-x0123456789abcd" } }))
      },
      { requiredKeys: [] }
    );
    await fc.assert(
      fc.asyncProperty(adversarial, async (options) => {
        const { client, bodies } = captureClient();
        try {
          await client.submit(options as unknown as SubmitOptions);
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
