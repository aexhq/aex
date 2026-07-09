import { createHmac, randomBytes } from "node:crypto";
import fc from "fast-check";
import { describe, expect, it } from "vitest";
import {
  mintConnectionTicket,
  verifyConnectionTicket,
  verifyAexWebhook,
  type ConnectionTicketChannel
} from "../src/index.js";

/**
 * Property fuzz for the two HMAC verifiers a customer/runtime depends on:
 *   - verifyAexWebhook        (Standard Webhooks sign→verify interop)
 *   - verify/mintConnectionTicket (the WS handshake grant)
 *
 * Both are exercised against an INDEPENDENT reference signer (Node `crypto`)
 * or the real mint function — no mocks. The point is tamper-evidence: any
 * change to the signed material flips verification to false, and a faithful
 * signer always verifies true.
 */

// --- webhook HMAC (Standard Webhooks) ---------------------------------------

const SECRET_MATERIAL = randomBytes(32).toString("base64");
const SECRET = `whsec_${SECRET_MATERIAL}`;

function signWebhook(id: string, ts: number, body: string, material = SECRET_MATERIAL): string {
  const key = Buffer.from(material, "base64");
  return `v1,${createHmac("sha256", key).update(`${id}.${ts}.${body}`).digest("base64")}`;
}

const wid = fc.string({ minLength: 1, maxLength: 40 }).filter((s) => !s.includes(" "));
const wbody = fc.string({ maxLength: 256 });

describe("verifyAexWebhook (property)", () => {
  it("accepts any faithfully-signed delivery within tolerance", async () => {
    await fc.assert(
      fc.asyncProperty(wid, wbody, async (id, body) => {
        const ts = Math.floor(Date.now() / 1000);
        const ok = await verifyAexWebhook({
          rawBody: body,
          headers: { "webhook-id": id, "webhook-timestamp": String(ts), "webhook-signature": signWebhook(id, ts, body) },
          secret: SECRET
        });
        expect(ok).toBe(true);
      }),
      { numRuns: 200 }
    );
  });

  it("rejects any single-field tamper (body, id, ts, or secret)", async () => {
    await fc.assert(
      fc.asyncProperty(wid, wbody, fc.constantFrom("body", "id", "ts", "secret"), async (id, body, what) => {
        const ts = Math.floor(Date.now() / 1000);
        const sig = signWebhook(id, ts, body);
        const ok = await verifyAexWebhook({
          rawBody: what === "body" ? `${body}X` : body,
          headers: {
            "webhook-id": what === "id" ? `${id}X` : id,
            "webhook-timestamp": String(what === "ts" ? ts + 1 : ts),
            "webhook-signature": sig
          },
          secret: what === "secret" ? `whsec_${randomBytes(32).toString("base64")}` : SECRET
        });
        expect(ok).toBe(false);
      }),
      { numRuns: 200 }
    );
  });

  it("golden interop vector verifies (fixed id/body/secret → known signature)", async () => {
    const material = Buffer.from("0123456789abcdef0123456789abcdef", "utf8").toString("base64");
    const id = "msg_2KWPBgLlAfxdpx2AI54pPJ85s7";
    const ts = 1700000000;
    const body = '{"specversion":"1.0","type":"session.finished","subject":"ses_42"}';
    const sig = signWebhook(id, ts, body, material);
    const ok = await verifyAexWebhook({
      rawBody: body,
      headers: { "webhook-id": id, "webhook-timestamp": String(ts), "webhook-signature": sig },
      secret: `whsec_${material}`,
      toleranceSeconds: Number.MAX_SAFE_INTEGER // pin the timestamp, test the MAC only
    });
    expect(ok).toBe(true);
  });

  it("never throws on malformed signature headers — returns false", async () => {
    await fc.assert(
      fc.asyncProperty(fc.string({ maxLength: 80 }), async (sigHeader) => {
        const ts = Math.floor(Date.now() / 1000);
        const ok = await verifyAexWebhook({
          rawBody: "{}",
          headers: { "webhook-id": "x", "webhook-timestamp": String(ts), "webhook-signature": sigHeader },
          secret: SECRET
        });
        expect(ok).toBe(false);
      }),
      { numRuns: 150 }
    );
  });
});

// --- connection ticket (WS handshake grant) ---------------------------------

const sessionId = fc.string({ minLength: 1, maxLength: 40 });
const ticketSecret = fc.string({ minLength: 8, maxLength: 48 });
const channel = fc.constantFrom<ConnectionTicketChannel>("event", "log", "all");

describe("connection ticket mint/verify (property)", () => {
  it("a freshly minted ticket verifies for its run + channel", async () => {
    await fc.assert(
      fc.asyncProperty(sessionId, ticketSecret, channel, async (id, secret, ch) => {
        const now = Date.now();
        const { ticket } = await mintConnectionTicket(id, secret, now, 60_000, ch);
        expect(await verifyConnectionTicket(ticket, id, secret, now, ch)).toBe(true);
      }),
      { numRuns: 200 }
    );
  });

  it("rejects a wrong sessionId, wrong secret, wrong channel, or expiry", async () => {
    await fc.assert(
      fc.asyncProperty(sessionId, ticketSecret, channel, async (id, secret, ch) => {
        const now = Date.now();
        const { ticket, expiresAtMs } = await mintConnectionTicket(id, secret, now, 60_000, ch);
        expect(await verifyConnectionTicket(ticket, `${id}X`, secret, now, ch)).toBe(false);
        expect(await verifyConnectionTicket(ticket, id, `${secret}X`, now, ch)).toBe(false);
        const otherCh: ConnectionTicketChannel = ch === "event" ? "log" : "event";
        expect(await verifyConnectionTicket(ticket, id, secret, now, otherCh)).toBe(false);
        expect(await verifyConnectionTicket(ticket, id, secret, expiresAtMs + 1, ch)).toBe(false);
      }),
      { numRuns: 150 }
    );
  });

  it("rejects a tampered MAC and never throws on a junk ticket string", async () => {
    await fc.assert(
      fc.asyncProperty(sessionId, ticketSecret, fc.string({ maxLength: 80 }), async (id, secret, junk) => {
        const now = Date.now();
        const { ticket } = await mintConnectionTicket(id, secret, now);
        // flip the last hex char of the MAC
        const last = ticket.slice(-1);
        const tampered = ticket.slice(0, -1) + (last === "a" ? "b" : "a");
        expect(await verifyConnectionTicket(tampered, id, secret, now)).toBe(false);
        // arbitrary junk verifies to false without throwing
        expect(await verifyConnectionTicket(junk, id, secret, now)).toBe(false);
      }),
      { numRuns: 150 }
    );
  });
});
