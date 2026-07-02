import fc from "fast-check";
import { describe, expect, it } from "vitest";
import { parseRunSubmissionRequest, type PlatformRunSubmissionRequest } from "../src/index.js";

/**
 * Robustness/totality fuzz for the top-level submission validator. Complements
 * invariants.property.test.ts (which proves accept + nested-secret rejection):
 * here we prove the parser is TOTAL — every input is either a clean accept or a
 * typed Error — and that the strict top-level allow-list + required-field gates
 * hold under fuzzing. A validator that throws a non-Error or accepts an
 * unknown/extra field is the wire-shape bug class this guards.
 */

function makeValid(): PlatformRunSubmissionRequest {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "key-1",
    provider: "anthropic",
    submission: {
      model: "claude-haiku-4-5",
      prompt: ["hello"],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: { apiKeys: { anthropic: "sk-ant-test" } }
  };
}

describe("parseRunSubmissionRequest robustness (property)", () => {
  it("is TOTAL: arbitrary input either parses to a valid request or throws an Error", () => {
    fc.assert(
      fc.property(fc.anything(), (input) => {
        try {
          const parsed = parseRunSubmissionRequest(input);
          // On the rare accept, the core required fields must be present + well-typed.
          expect(typeof parsed.workspaceId === "string" || parsed.workspaceId === undefined).toBe(true);
          expect(Array.isArray(parsed.submission.prompt)).toBe(true);
          expect(parsed.submission.prompt.length).toBeGreaterThan(0);
        } catch (err) {
          expect(err).toBeInstanceOf(Error);
        }
      }),
      { numRuns: 600 }
    );
  });

  it("rejects any unknown top-level field (strict allow-list)", () => {
    fc.assert(
      fc.property(
        fc.string({ minLength: 1, maxLength: 16 }).filter((k) => !RESERVED.has(k)),
        fc.anything(),
        (key, value) => {
          const input = makeValid() as unknown as Record<string, unknown>;
          Object.defineProperty(input, key, {
            value,
            enumerable: true,
            configurable: true,
            writable: true
          });
          expect(() => parseRunSubmissionRequest(input)).toThrow();
        }
      ),
      { numRuns: 200 }
    );
  });

  it("rejects a missing/empty/whitespace-only prompt", () => {
    for (const prompt of [[], [""], ["   "], ["\n\t "]]) {
      const input = makeValid();
      (input.submission as { prompt: unknown }).prompt = prompt;
      expect(() => parseRunSubmissionRequest(input)).toThrow();
    }
    const noSub = makeValid() as unknown as Record<string, unknown>;
    delete noSub.submission;
    expect(() => parseRunSubmissionRequest(noSub)).toThrow();
  });

  it("rejects a non-record top-level input", () => {
    fc.assert(
      fc.property(
        fc.oneof(fc.string(), fc.integer(), fc.boolean(), fc.constant(null), fc.array(fc.anything())),
        (input) => {
          expect(() => parseRunSubmissionRequest(input)).toThrow();
        }
      ),
      { numRuns: 150 }
    );
  });
});

const RESERVED = new Set([
  "workspaceId",
  "idempotencyKey",
  "provider",
  "submission",
  "runtimeSize",
  "timeout",
  "webhook",
  "limits",
  "machine",
  "secrets"
]);
