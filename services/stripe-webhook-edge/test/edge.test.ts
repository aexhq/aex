import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import {
  HANDLED_EVENT_TYPES,
  MAX_BODY_BYTES,
  decodeRawBody,
  normalizeEvent,
  verifyWithRotatingSecrets,
} from "../src/edge.js";

const endpointPolicy = JSON.parse(
  readFileSync(new URL("../../../release/stripe-endpoint.json", import.meta.url), "utf8"),
);

describe("raw webhook boundary", () => {
  test("shares one endpoint policy with provider provisioning", () => {
    expect(endpointPolicy).toEqual({
      schema: "aex.stripe-webhook-endpoint-policy.v1",
      apiVersion: "2026-06-24.dahlia",
      connect: false,
      enabledEvents: [...HANDLED_EVENT_TYPES],
    });
    expect(new Set(endpointPolicy.enabledEvents).size).toBe(endpointPolicy.enabledEvents.length);
  });

  test("preserves ordinary and base64 bodies byte for byte", () => {
    expect(decodeRawBody({ body: "raw", isBase64Encoded: false })).toEqual(Buffer.from("raw"));
    expect(
      decodeRawBody({ body: Buffer.from([0, 255, 7]).toString("base64"), isBase64Encoded: true }),
    ).toEqual(Buffer.from([0, 255, 7]));
  });

  test("rejects an oversized body before signature work", () => {
    expect(() =>
      decodeRawBody({ body: "x".repeat(MAX_BODY_BYTES + 1), isBase64Encoded: false }),
    ).toThrow("exceeds 262144 bytes");
  });

  test("tries only the current and previous rotation secrets", () => {
    const attempted: string[] = [];
    const verifier = (_body: Buffer, _signature: string, secret: string) => {
      attempted.push(secret);
      if (secret === "previous") return { id: "evt_1", type: "refund.failed" };
      throw new Error("bad signature");
    };
    expect(
      verifyWithRotatingSecrets(Buffer.from("body"), "sig", ["current", "previous"], verifier),
    ).toEqual({ id: "evt_1", type: "refund.failed" });
    expect(attempted).toEqual(["current", "previous"]);
  });

  test("quarantines a mismatched provider API version", () => {
    expect(normalizeEvent({ id: "evt_1", type: "refund.failed", api_version: "wrong" })).toEqual({
      disposition: "quarantined_api_version",
      providerEventId: "evt_1",
    });
  });

  test("acknowledges a signed unsupported event without forwarding it", () => {
    expect(normalizeEvent({ id: "evt_2", type: "customer.created", api_version: "2026-06-24.dahlia" })).toEqual({
      disposition: "ignored_unsupported",
      providerEventId: "evt_2",
    });
  });
});
