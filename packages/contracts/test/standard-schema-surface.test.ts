import type { StandardSchemaV1 } from "@standard-schema/spec";
import { describe, expect, it } from "bun:test";
import {
  SessionLimitsSchema,
  SessionMachineSchema,
  SessionWebhookSchema
} from "../src/index.js";

/**
 * D2: `~standard` is the published interface, not Zod. These assertions are what
 * make that claim true rather than aspirational — if a schema stopped conforming,
 * or the barrel stopped exporting it, this fails.
 */
const published = {
  SessionWebhookSchema,
  SessionLimitsSchema,
  SessionMachineSchema
} as const;

// Compile-time half: every published schema satisfies the vendor-neutral
// interface. `@standard-schema/spec` is types-only and adds no runtime bytes.
const _conforms: Readonly<Record<string, StandardSchemaV1>> = published;

describe("standard schema surface", () => {
  it.each(Object.entries(published))("%s reports the Standard Schema v1 contract", (_name, schema) => {
    const standard = (schema as StandardSchemaV1)["~standard"];
    expect(standard.version).toBe(1);
    expect(standard.vendor).toBe("zod");
    expect(typeof standard.validate).toBe("function");
  });

  it("validates and reports issues through ~standard alone", async () => {
    const standard = (SessionWebhookSchema as StandardSchemaV1)["~standard"];

    const accepted = await standard.validate({ url: "https://hooks.example.com/aex" });
    expect(accepted.issues).toBeUndefined();
    expect((accepted as { value: unknown }).value).toEqual({ url: "https://hooks.example.com/aex" });

    const rejected = await standard.validate({ url: "http://hooks.example.com/aex" });
    expect(rejected.issues?.[0]?.message).toBe("webhook.url must use https (got http)");
  });

  it("carries the full rejected-key list on an unknown field", async () => {
    const standard = (SessionLimitsSchema as StandardSchemaV1)["~standard"];
    const rejected = await standard.validate({ maxTurns: 1, nope: true, alsoNope: 1 });
    // The message names only the first offending key, matching the parser it
    // replaced; the issue itself still carries every one.
    expect(rejected.issues?.[0]?.message).toBe(
      "limits.nope is not an allowed field; permitted: maxConcurrentChildSessions, maxSubagentDepth, maxSpendUsd, maxTurns, maxStepsPerTurn"
    );
  });

  it("does not expose ~standard.jsonSchema — mini tree-shakes the converter out", () => {
    // Documented in schemas/index.ts. Build-time tooling imports full `zod`
    // off these same objects instead; if this ever starts passing, the OpenAPI
    // generator can drop that indirection.
    const standard = (SessionWebhookSchema as StandardSchemaV1)["~standard"] as {
      jsonSchema?: unknown;
    };
    expect(standard.jsonSchema).toBeUndefined();
  });
});
