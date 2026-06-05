import { describe, expect, it } from "vitest";
import { AgentExecutor } from "../../src/index.js";
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
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
    }
  },
  attempts: [],
  events: { entries: [], totalCount: 0, truncated: false },
  rawEventPages: [],
  outputs: [],
  outputCaptureFailures: [],
  proxyCalls: { entries: [], totalCount: 0, truncated: false },
  skillSnapshots: [],
  providerSkills: [],
  inlineSkills: []
};

describe("AgentExecutor.getRunUnit", () => {
  it("hits GET /api/runs/:id and parses the wire RunUnit", async () => {
    const calls: string[] = [];
    const stub: typeof fetch = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      calls.push(url);
      return new Response(JSON.stringify(SAMPLE_UNIT), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    };
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://example.test", fetch: stub });
    const unit = await client.getRunUnit("run-1");
    expect(calls).toEqual(["https://example.test/api/runs/run-1"]);
    expect(unit.id).toBe("run-1");
    expect(unit.submission.kind).toBe("submission");
    if (unit.submission.kind !== "submission") return;
    expect(unit.submission.submission.system).toBe("be helpful");
  });

  it("getUnit delegates to getRunUnit", async () => {
    let called = 0;
    const stub: typeof fetch = async (input) => {
      called += 1;
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      if (called === 1) {
        // submission echo
        return new Response(JSON.stringify({ id: "run-1", status: "queued" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      // RunUnit fetch
      expect(url).toBe("https://example.test/api/runs/run-1");
      return new Response(JSON.stringify(SAMPLE_UNIT), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    };
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://example.test", fetch: stub });
    const runId = await client.submitRun({
      model: "claude-haiku-4-5",
      prompt: "hi",
      secrets: { anthropic: { apiKey: "sk-test" } }
    });
    const unit = await client.getUnit(runId);
    expect(unit.id).toBe("run-1");
  });
});
