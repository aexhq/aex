import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import {
  HANDLED_EVENT_TYPES,
  MAX_BODY_BYTES,
  decodeRawBody,
  normalizeEvent,
  verifyWithRotatingSecrets,
} from "../src/edge.js";
import { ingestRequest } from "../src/handler.js";

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

describe("finance-ingest handoff", () => {
  test("payment-method fixture is byte-shape compatible with the Rust request", () => {
    const expected = JSON.parse(
      readFileSync(
        new URL("./fixtures/payment-method-attached-ingest.json", import.meta.url),
        "utf8",
      ),
    );
    const event = {
      id: "evt_pm_attached_fixture",
      type: "payment_method.attached",
      api_version: "2026-06-24.dahlia",
      created: 1_800_000_100,
      data: {
        object: {
          id: "pm_fixture",
          object: "payment_method",
          type: "card",
          customer: "cus_fixture",
          created: 1_799_999_000,
          metadata: {},
          card: { brand: "visa", last4: "4242", exp_month: 12, exp_year: 2032 },
        },
      },
    };
    expect(
      ingestRequest(
        event as never,
        Buffer.from("signed-body-fixture"),
        new Date("2027-01-15T08:03:20.000Z"),
      ),
    ).toEqual(expected);
  });

  test("detach needs neither customer nor metadata and emits no card secrets", () => {
    const request = ingestRequest(
      {
        id: "evt_pm_detached_fixture",
        type: "payment_method.detached",
        api_version: "2026-06-24.dahlia",
        created: 1_800_000_200,
        data: { object: { id: "pm_fixture", customer: null, metadata: {} } },
      } as never,
      Buffer.from("detached"),
      new Date("2027-01-15T08:05:00.000Z"),
    );
    expect(request.event.facts).toEqual({
      kind: "payment_method_detached",
      method: "pm_fixture",
    });
    expect(JSON.stringify(request)).not.toContain("424242");
  });

  test("money facts keep the exact existing integer transition inputs", () => {
    const expected = JSON.parse(
      readFileSync(
        new URL("./fixtures/payment-intent-succeeded-ingest.json", import.meta.url),
        "utf8",
      ),
    );
    const event = {
      id: "evt_pi_succeeded_fixture",
      type: "payment_intent.succeeded",
      api_version: "2026-06-24.dahlia",
      created: 1_800_000_100,
      data: {
        object: {
          id: "pi_fixture",
          amount_received: 1000,
          metadata: {
            aex_org_id: "org_01kyw2qa4pew48j2gb1g6gw3rg",
            aex_credit_cents: "1000",
          },
        },
      },
    };
    expect(
      ingestRequest(
        event as never,
        Buffer.from("money-fixture"),
        new Date("2027-01-15T08:03:20.000Z"),
      ),
    ).toEqual(expected);
  });
});
