import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";
import type { RunUnit } from "@aexhq/contracts";

const SAMPLE_UNIT: RunUnit = {
  id: "run-1",
  workspaceId: "workspace-1",
  status: "succeeded",
  cleanupStatus: "succeeded",
  createdAt: "2026-01-01T00:00:00.000Z",
  updatedAt: "2026-01-01T00:01:00.000Z",
  attemptCount: 1,
  submission: {
    kind: "submission",
    submission: {
      model: "claude-haiku-4-5",
      system: "be helpful",
      prompt: ["hi"],
      agentsMd: [],
      files: [],
      mcpServers: []
    }
  },
  attempts: [],
  events: { entries: [], totalCount: 0, truncated: false },
  rawEventPages: [],
  outputs: [],
  outputCaptureFailures: []
};

describe("SessionHandle.unit", () => {
  it("hits GET /api/runs/:id and parses the wire RunUnit", async () => {
    const calls: string[] = [];
    const stub: typeof fetch = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      calls.push(url);
      // The session rehydrate read echoes a minimal session record; the unit
      // read returns the full RunUnit. Both are served from the same fixture.
      if (url.endsWith("/api/sessions/run-1")) {
        return new Response(JSON.stringify({ id: "run-1", status: "succeeded" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      return new Response(JSON.stringify(SAMPLE_UNIT), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    };
    const client = new Aex({ apiToken: "tkn", baseUrl: "https://example.test", fetch: stub });
    const session = await client.openSession("run-1");
    const unit = await session.unit();
    // The unit read hits the run-keyed endpoint.
    expect(calls).toContain("https://example.test/api/runs/run-1");
    expect(unit.id).toBe("run-1");
    expect(unit.submission.kind).toBe("submission");
    if (unit.submission.kind !== "submission") return;
    expect(unit.submission.submission.system).toBe("be helpful");
  });
});
