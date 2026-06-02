import { describe, expect, it } from "vitest";
import { mintConnectionTicket, verifyConnectionTicket } from "../src/index.js";

const SECRET = "dev-coordinator-secret-at-least-32-chars-xx";
const NOW = 1_700_000_000_000;

describe("connection ticket", () => {
  it("a freshly minted ticket verifies for its run", async () => {
    const { ticket, expiresAtMs } = await mintConnectionTicket("run_a", SECRET, NOW);
    expect(expiresAtMs).toBe(NOW + 60_000);
    expect(await verifyConnectionTicket(ticket, "run_a", SECRET, NOW + 1_000)).toBe(true);
  });

  it("rejects a ticket presented after expiry", async () => {
    const { ticket, expiresAtMs } = await mintConnectionTicket("run_a", SECRET, NOW);
    expect(await verifyConnectionTicket(ticket, "run_a", SECRET, expiresAtMs + 1)).toBe(false);
  });

  it("rejects a ticket bound to a different run", async () => {
    const { ticket } = await mintConnectionTicket("run_a", SECRET, NOW);
    expect(await verifyConnectionTicket(ticket, "run_b", SECRET, NOW + 1_000)).toBe(false);
  });

  it("rejects a ticket signed with a different secret", async () => {
    const { ticket } = await mintConnectionTicket("run_a", SECRET, NOW);
    expect(await verifyConnectionTicket(ticket, "run_a", "some-other-secret-32-chars-aaaaaaaaaa", NOW + 1_000)).toBe(false);
  });

  it("rejects a tampered MAC", async () => {
    const { ticket } = await mintConnectionTicket("run_a", SECRET, NOW);
    const tampered = `${ticket.slice(0, -1)}${ticket.endsWith("0") ? "1" : "0"}`;
    expect(await verifyConnectionTicket(tampered, "run_a", SECRET, NOW + 1_000)).toBe(false);
  });

  it("rejects a malformed ticket", async () => {
    expect(await verifyConnectionTicket("garbage", "run_a", SECRET, NOW)).toBe(false);
  });
});
