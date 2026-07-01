/**
 * `createCorpusTools` (chat-mvp) — corpus-scoped read tools. Every read tool is
 * fenced to the corpus sessions; search_outputs is auto-scoped; list_sessions
 * returns only corpus sessions.
 */
import { describe, expect, it } from "vitest";
import { createCorpusTools, DataToolError } from "../../src/index.js";
import type { AgentExecutor } from "../../src/index.js";

function stubClient(): {
  client: AgentExecutor;
  calls: Array<{ method: string; args: unknown[] }>;
} {
  const calls: Array<{ method: string; args: unknown[] }> = [];
  const record =
    (method: string, value: (...a: unknown[]) => unknown) =>
    (...args: unknown[]) => {
      calls.push({ method, args });
      return value(...args);
    };
  const client = {
    sessions: {
      list: record("list", () => ({
        sessions: [{ id: "run-1" }, { id: "run-2" }, { id: "run-3" }]
      })),
      get: record("get", (id) => ({ id, status: "idle", createdAt: "t" })),
      outputs: record("outputs", () => [{ id: "o1", filename: "f.md", sizeBytes: 1, contentType: "text/markdown" }]),
      readOutput: record("readOutput", () => ({ output: { id: "o1", filename: "f.md" }, text: "x", truncated: false, totalBytes: 1 })),
      searchOutputs: record("searchOutputs", () => ({ hits: [{ runId: "run-1", outputId: "o1" }] }))
    }
  } as unknown as AgentExecutor;
  return { client, calls };
}

describe("createCorpusTools (sessionIds corpus)", () => {
  it("rejects get_session / list_outputs / read_output for a session outside the corpus", async () => {
    const { client } = stubClient();
    const tools = createCorpusTools(client, { sessionIds: ["run-1", "run-2"] });
    await expect(tools.execute("get_session", { session_id: "run-9" })).rejects.toBeInstanceOf(DataToolError);
    await expect(tools.execute("list_outputs", { session_id: "run-9" })).rejects.toThrow(/not in this chat's corpus/);
    await expect(tools.execute("read_output", { session_id: "run-9", path: "f.md" })).rejects.toBeInstanceOf(DataToolError);
  });

  it("allows in-corpus reads", async () => {
    const { client } = stubClient();
    const tools = createCorpusTools(client, { sessionIds: ["run-1", "run-2"] });
    const summary = (await tools.execute("get_session", { session_id: "run-1" })) as { id: string };
    expect(summary.id).toBe("run-1");
  });

  it("auto-scopes search_outputs to the corpus sessionIds", async () => {
    const { client, calls } = stubClient();
    const tools = createCorpusTools(client, { sessionIds: ["run-1", "run-2"] });
    await tools.execute("search_outputs", { filename: "report" });
    const call = calls.find((c) => c.method === "searchOutputs")!;
    expect(call.args[0]).toEqual({ runIds: ["run-1", "run-2"], filename: "report" });
  });

  it("list_sessions returns only corpus sessions (built from get, no list call)", async () => {
    const { client, calls } = stubClient();
    const tools = createCorpusTools(client, { sessionIds: ["run-1", "run-2"] });
    const result = (await tools.execute("list_sessions", {})) as { sessions: Array<{ id: string }> };
    expect(result.sessions.map((r) => r.id).sort()).toEqual(["run-1", "run-2"]);
    expect(calls.some((c) => c.method === "list")).toBe(false);
  });
});

describe("createCorpusTools (filter corpus)", () => {
  it("resolves the allow-list from listSessions and scopes to it", async () => {
    const { client, calls } = stubClient();
    const tools = createCorpusTools(client, { filter: { status: "idle" } });
    // run-1/run-2/run-3 come from listSessions; run-9 is not in the set
    await expect(tools.execute("get_session", { session_id: "run-9" })).rejects.toBeInstanceOf(DataToolError);
    const ok = (await tools.execute("get_session", { session_id: "run-3" })) as { id: string };
    expect(ok.id).toBe("run-3");
    expect(calls.some((c) => c.method === "list")).toBe(true);
  });
});
