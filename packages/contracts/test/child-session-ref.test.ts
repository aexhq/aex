/** A child reference is observable without pretending to be a resumable session. */
import { describe, expect, it } from "bun:test";
import { HttpClient, type ChildSessionRef } from "../src/index.js";
import { operations } from "../src/internal.js";

function clientFor(body: unknown): HttpClient {
  return new HttpClient({
    apiKey: "test",
    baseUrl: "https://api.test",
    fetch: async () => new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" }
    })
  });
}

const CHILD = {
  id: "ses_child_1",
  parentSessionId: "ses_parent",
  status: "idle",
  createdAt: "2026-07-11T00:00:00.000Z",
  updatedAt: "2026-07-11T00:01:00.000Z",
  lastRun: { sessionId: "ses_child_1", runId: "run_1", turnSeq: 1, phase: "finished", outcome: "succeeded" }
} satisfies ChildSessionRef;

describe("ChildSessionRef observation invariant", () => {
  it("[compile-time] a ChildSessionRef carries an observable id and complete lineage timestamps", () => {
    const child: ChildSessionRef = CHILD;
    expect(child.id).toBe("ses_child_1");
  });

  it("[compile-time] a bare string is not an observable child reference", () => {
    // @ts-expect-error - a child reference carries more than a bare id string
    const bad: ChildSessionRef = "ses_child_1";
    void bad;
  });

  it("accepts the canonical read-only lineage snapshot", async () => {
    await expect(operations.listSessionChildren(clientFor({ children: [CHILD] }), "ses_parent"))
      .resolves.toEqual([CHILD]);
  });

  it.each(["sessionId", "runtime", "turnSeq", "cleanupStatus"])(
    "rejects the removed top-level %s field",
    async (field) => {
      await expect(operations.listSessionChildren(clientFor({
        children: [{ ...CHILD, [field]: field === "turnSeq" ? 1 : "removed" }]
      }), "ses_parent")).rejects.toThrow(new RegExp(`removed ${field} field`));
    }
  );
});
