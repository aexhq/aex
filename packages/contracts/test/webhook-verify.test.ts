import { createHmac, randomBytes } from "node:crypto";
import { describe, expect, it } from "vitest";
import { verifyAexWebhook } from "../src/index.js";

// 32 random bytes is the production secret size; a fixed value keeps the vector
// reproducible. The `whsec_` prefix mirrors the Standard Webhooks convention.
const SECRET_MATERIAL_B64 = randomBytes(32).toString("base64");
const SECRET = `whsec_${SECRET_MATERIAL_B64}`;

const ID = "msg_test_p5jXN8AQM9LWM0D4loKWxJek";
const BODY = JSON.stringify({ specversion: "1.0", type: "session.finished", subject: "ses_42" });

/**
 * Independent reference signer (Node `crypto`) — this is the interop
 * cross-check: if `verifyAexWebhook` (Web Crypto) accepts what Node's
 * `createHmac` produces, an off-the-shelf `standardwebhooks` consumer will too.
 */
function sign(id: string, ts: number, body: string, secretMaterialB64 = SECRET_MATERIAL_B64): string {
  const key = Buffer.from(secretMaterialB64, "base64");
  const mac = createHmac("sha256", key).update(`${id}.${ts}.${body}`).digest("base64");
  return `v1,${mac}`;
}

function headers(id: string, ts: number, signature: string): Record<string, string> {
  return {
    "webhook-id": id,
    "webhook-timestamp": String(ts),
    "webhook-signature": signature
  };
}

describe("verifyAexWebhook", () => {
  it("accepts a valid signature within tolerance", async () => {
    const ts = Math.floor(Date.now() / 1000);
    const ok = await verifyAexWebhook({
      rawBody: BODY,
      headers: headers(ID, ts, sign(ID, ts, BODY)),
      secret: SECRET
    });
    expect(ok).toBe(true);
  });

  it("accepts the secret without the whsec_ prefix too", async () => {
    const ts = Math.floor(Date.now() / 1000);
    const ok = await verifyAexWebhook({
      rawBody: BODY,
      headers: headers(ID, ts, sign(ID, ts, BODY)),
      secret: SECRET_MATERIAL_B64
    });
    expect(ok).toBe(true);
  });

  it("accepts a Headers instance", async () => {
    const ts = Math.floor(Date.now() / 1000);
    const h = new Headers(headers(ID, ts, sign(ID, ts, BODY)));
    const ok = await verifyAexWebhook({ rawBody: BODY, headers: h, secret: SECRET });
    expect(ok).toBe(true);
  });

  it("accepts a match anywhere in a space-delimited rotation list", async () => {
    const ts = Math.floor(Date.now() / 1000);
    const wrong = sign(ID, ts, BODY, randomBytes(32).toString("base64"));
    const right = sign(ID, ts, BODY);
    const ok = await verifyAexWebhook({
      rawBody: BODY,
      headers: headers(ID, ts, `${wrong} ${right}`),
      secret: SECRET
    });
    expect(ok).toBe(true);
  });

  it("rejects a tampered body", async () => {
    const ts = Math.floor(Date.now() / 1000);
    const ok = await verifyAexWebhook({
      rawBody: BODY + " ",
      headers: headers(ID, ts, sign(ID, ts, BODY)),
      secret: SECRET
    });
    expect(ok).toBe(false);
  });

  it("rejects a stale timestamp beyond tolerance", async () => {
    const ts = Math.floor(Date.now() / 1000) - 600; // 10 min ago, tolerance 300s
    const ok = await verifyAexWebhook({
      rawBody: BODY,
      headers: headers(ID, ts, sign(ID, ts, BODY)),
      secret: SECRET
    });
    expect(ok).toBe(false);
  });

  it("rejects the wrong secret", async () => {
    const ts = Math.floor(Date.now() / 1000);
    const ok = await verifyAexWebhook({
      rawBody: BODY,
      headers: headers(ID, ts, sign(ID, ts, BODY, randomBytes(32).toString("base64"))),
      secret: SECRET
    });
    expect(ok).toBe(false);
  });

  it("rejects when required headers are missing", async () => {
    const ts = Math.floor(Date.now() / 1000);
    const ok = await verifyAexWebhook({
      rawBody: BODY,
      headers: { "webhook-id": ID, "webhook-timestamp": String(ts) },
      secret: SECRET
    });
    expect(ok).toBe(false);
  });
});
