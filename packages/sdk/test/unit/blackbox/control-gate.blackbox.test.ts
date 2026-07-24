/**
 * BLACKBOX — a control intent is a GATE, not advice (WS10 / T10).
 *
 * The finding: `requestApproval`/`approve`/`deny` (and `suspend`/`cancel`) must be
 * real server-side transitions the caller can observe — not client-side no-ops.
 * Each assertion drives the public handle method and checks BOTH that the intent
 * reached the server (the route was POSTed) AND that the returned session moved to
 * the expected state.
 */
import { describe, expect, it } from "bun:test";
import type { SessionHandle } from "../../../src/index.js";
import { FakePlatform } from "./fake-platform.js";

async function openHandle(platform: FakePlatform): Promise<SessionHandle> {
  return platform.aex.sessions.create({ model: "anthropic/claude-haiku-4-5" });
}

describe("blackbox: HITL + control gates", () => {
  it("requestApproval parks the session awaiting_approval", async () => {
    const platform = new FakePlatform();
    const handle = await openHandle(platform);
    const accepted = await handle.requestApproval();
    expect(accepted.session.status).toBe("awaiting_approval");
    // The SDK also folds the transition into the handle's OWN record.
    expect(handle.record.status).toBe("awaiting_approval");
    expect(platform.requests.some((r) => r.endsWith("/request-approval"))).toBe(true);
  });

  it("approve resumes the held turn (→ running) — one approval, one gated dispatch", async () => {
    const platform = new FakePlatform();
    const handle = await openHandle(platform);
    await handle.requestApproval();
    const accepted = await handle.approve();
    expect(accepted.session.status).toBe("running");
    expect(handle.record.status).toBe("running");
    expect(platform.requests.some((r) => r.endsWith("/approve"))).toBe(true);
  });

  it("deny returns the thread to idle and records the cancelled run outcome", async () => {
    const platform = new FakePlatform();
    const handle = await openHandle(platform);
    await handle.requestApproval();
    const accepted = await handle.deny();
    expect(accepted.session.status).toBe("idle");
    expect(accepted.session.lastRun?.outcome).toBe("cancelled");
    expect(handle.record.status).toBe("idle");
    expect(platform.requests.some((r) => r.endsWith("/deny"))).toBe(true);
  });

  it("suspend and cancel reach the server and move the session", async () => {
    const platform = new FakePlatform();
    const handle = await openHandle(platform);

    const suspended = await handle.suspend();
    expect(suspended.session.status).toBe("suspended");
    expect(platform.requests.some((r) => r.endsWith("/suspend"))).toBe(true);

    const cancelled = await handle.cancel();
    // The cancel intent is recorded server-side — not swallowed client-side.
    expect(platform.requests.some((r) => r.endsWith("/cancel"))).toBe(true);
    expect(cancelled.session.status).toBe("cancelling");
  });
});
