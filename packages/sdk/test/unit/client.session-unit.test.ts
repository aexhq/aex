import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";
import type { SessionUnit } from "@aexhq/contracts";

const SAMPLE_UNIT: SessionUnit = {
  id: "session-1",
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
  sessionFiles: [],
  fileCaptureFailures: []
};

describe("SessionHandle.unit", () => {
  it("hits GET /api/sessions/:id and parses the wire SessionUnit", async () => {
    const calls: string[] = [];
    let sessionReads = 0;
    const stub: typeof fetch = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      calls.push(url);
      if (url.endsWith("/api/sessions/session-1")) {
        sessionReads++;
        if (sessionReads > 1) {
          return new Response(JSON.stringify(SAMPLE_UNIT), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        // The session rehydrate read echoes a minimal session record; the unit
        // read returns the full SessionUnit from the same session-keyed route.
        return new Response(JSON.stringify({ id: "session-1", status: "succeeded" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      return new Response("not found", { status: 404 });
    };
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch: stub });
    const session = await client.openSession("session-1");
    const unit = await session.unit();
    // The unit read hits the session-keyed endpoint.
    expect(calls).toContain("https://example.test/api/sessions/session-1");
    expect(unit.id).toBe("session-1");
    expect(unit.submission.kind).toBe("submission");
    if (unit.submission.kind !== "submission") return;
    expect(unit.submission.submission.system).toBe("be helpful");
  });
});
