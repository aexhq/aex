import fc from "fast-check";
import { describe, expect, it } from "vitest";
import {
  SESSION_CONTROL_STATUSES,
  TERMINAL_SESSION_CONTROL_STATUSES,
  getSessionControlStatusKind,
  isTerminalSessionControlStatus,
  parseSessionSubmissionRequest,
  type JsonValue,
  type PlatformSessionSubmissionRequest
} from "../src/index.js";

const deniedSecretFields = [
  "providerApiKey",
  "anthropicApiKey",
  "apiKey",
  "accessToken",
  "refreshToken",
  "password",
  "mcpCredentials",
  "credentials"
];

// Non-whitespace: the submission validator rejects text fields that are present
// but whitespace-only (parsePrompt throws "must contain non-whitespace text"
// when every prompt element trims to empty), so a length-1 string of spaces is
// NOT a valid submission. Filter to guarantee real content — otherwise this
// "accepts valid submissions" property flakes whenever fast-check happens to
// generate an all-whitespace prompt array (seed-dependent).
const nonEmptyString = fc.string({ minLength: 1, maxLength: 32 }).filter((s) => s.trim().length > 0);
const safeKey = fc
  .string({ minLength: 1, maxLength: 20 })
  .filter((value) => !deniedSecretFields.includes(value));

const jsonValue = fc.letrec((tie) => ({
  value: fc.oneof(
    { depthSize: "small", maxDepth: 3 },
    fc.string({ maxLength: 64 }),
    fc.double({ noNaN: true, noDefaultInfinity: true }),
    fc.boolean(),
    fc.constant(null),
    fc.array(tie("value"), { maxLength: 5 }),
    fc.dictionary(safeKey, tie("value"), { maxKeys: 5 })
  )
})).value as fc.Arbitrary<JsonValue>;

const jsonRecord = fc.dictionary(safeKey, jsonValue, { maxKeys: 5 });

const submission = fc.record({
  workspaceId: nonEmptyString,
  idempotencyKey: nonEmptyString,
  submission: fc.record({
    model: fc.constant("claude-haiku-4-5"),
    system: fc.option(nonEmptyString, { nil: undefined }),
    prompt: fc.array(nonEmptyString, { minLength: 1, maxLength: 5 }),
    agentsMd: fc.constant([] as never[]),
    files: fc.constant([] as never[]),
    mcpServers: fc.constant([] as never[]),
    metadata: fc.option(jsonRecord, { nil: undefined })
  }),
  secrets: fc.record({
    apiKeys: fc.record({
      anthropic: nonEmptyString
    })
  })
}, { requiredKeys: ["workspaceId", "idempotencyKey", "submission", "secrets"] });

describe("shared platform invariants", () => {
  it("accepts generated JSON-serializable platform submissions", () => {
    fc.assert(fc.property(submission, (input) => {
      const parsed = parseSessionSubmissionRequest(input);
      expect(parsed.workspaceId).toBe(input.workspaceId);
      expect(parsed.idempotencyKey).toBe(input.idempotencyKey);
      expect(parsed.submission.prompt.length).toBeGreaterThan(0);
      expect(parsed.secrets.apiKeys?.anthropic).toBe(input.secrets.apiKeys.anthropic);
    }), { numRuns: 100 });
  });

  it("rejects secret-bearing fields at arbitrary nested depth", () => {
    fc.assert(fc.property(
      fc.array(safeKey, { maxLength: 4 }),
      fc.constantFrom(...deniedSecretFields),
      nonEmptyString,
      (path, secretKey, secretValue) => {
        const input = makeValidSubmission();
        const meta: Record<string, JsonValue> = {};
        insertNested(meta, [...path, secretKey], secretValue);
        (input.submission as { metadata?: Record<string, JsonValue> }).metadata = meta;
        expect(() => parseSessionSubmissionRequest(input)).toThrow(/Secret-bearing field is not allowed/);
      }
    ), { numRuns: 75 });
  });

  it("rejects non-finite numbers because submissions must be true JSON values", () => {
    fc.assert(fc.property(
      fc.constantFrom(Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY),
      (value) => {
        const input = makeValidSubmission();
        (input.submission as { metadata?: Record<string, JsonValue> }).metadata = { value };
        expect(() => parseSessionSubmissionRequest(input)).toThrow(/JSON-serializable/);
      }
    ));
  });

  it("keeps the session-status partition complete and explicit", () => {
    const terminal = new Set<string>(TERMINAL_SESSION_CONTROL_STATUSES);
    const active = SESSION_CONTROL_STATUSES.filter((status) => !terminal.has(status));

    expect([...terminal, ...active].sort()).toEqual([...SESSION_CONTROL_STATUSES].sort());
    for (const status of SESSION_CONTROL_STATUSES) {
      expect(getSessionControlStatusKind(status)).toBe(terminal.has(status) ? "terminal" : "active");
      expect(isTerminalSessionControlStatus(status)).toBe(terminal.has(status));
    }
  });
});

function makeValidSubmission(): PlatformSessionSubmissionRequest {
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

function insertNested(target: Record<string, JsonValue>, path: readonly string[], value: string): void {
  let cursor: Record<string, JsonValue> = target;
  for (const [index, key] of path.entries()) {
    if (index === path.length - 1) {
      defineJsonProperty(cursor, key, value);
      return;
    }
    const next: Record<string, JsonValue> = {};
    defineJsonProperty(cursor, key, next);
    cursor = next;
  }
}

function defineJsonProperty(target: Record<string, JsonValue>, key: string, value: JsonValue): void {
  Object.defineProperty(target, key, {
    value,
    enumerable: true,
    configurable: true,
    writable: true
  });
}
