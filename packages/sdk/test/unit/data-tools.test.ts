import { describe, expect, it } from "vitest";
import { createDataTools, DataToolError } from "../../src/index.js";
import type { AgentExecutor } from "../../src/index.js";

/** A stub exposing only the `sessions.*` read methods createDataTools depends on. */
function stubClient(overrides: Partial<Record<string, (...args: unknown[]) => unknown>> = {}): {
  client: AgentExecutor;
  calls: Array<{ method: string; args: unknown[] }>;
} {
  const calls: Array<{ method: string; args: unknown[] }> = [];
  const record =
    (method: string, value: unknown) =>
    (...args: unknown[]) => {
      calls.push({ method, args });
      return overrides[method] ? overrides[method]!(...args) : value;
    };
  const client = {
    sessions: {
      list: record("list", {
        sessions: [{ id: "run-1", status: "idle", createdAt: "t", updatedAt: "t" }]
      }),
      get: record("get", {
        id: "run-1",
        status: "idle",
        createdAt: "t0",
        updatedAt: "t1",
        costUsd: 0.42,
        // a field that MUST NOT leak into the summary:
        template_snapshot: { prompt: "secret prompt" }
      }),
      // `outputs(id)` returns the rich accessor (mirrors the real SDK), so the
      // executor reaches outputs via `.list()` / `.read(sel, opts)`. Each verb
      // records the id-folded call so assertions stay `[id, ...args]`-shaped.
      outputs: (id: unknown) => ({
        list: (...args: unknown[]) => {
          calls.push({ method: "outputs", args: [id, ...args] });
          return overrides.outputs
            ? overrides.outputs(id, ...args)
            : [{ id: "out-1", filename: "report.md", sizeBytes: 12, contentType: "text/markdown", extra: "drop-me" }];
        },
        read: (...args: unknown[]) => {
          calls.push({ method: "readOutput", args: [id, ...args] });
          return overrides.readOutput
            ? overrides.readOutput(id, ...args)
            : { output: { id: "out-1", filename: "report.md" }, text: "# Report", truncated: false, totalBytes: 8 };
        }
      })
    }
  } as unknown as AgentExecutor;
  return { client, calls };
}

describe("createDataTools", () => {
  it("exposes the read tools (incl. search_outputs) with valid JSON-schema shapes", () => {
    const { client } = stubClient();
    const { tools, instructions } = createDataTools(client);
    expect(tools.map((t) => t.name)).toEqual([
      "list_sessions",
      "get_session",
      "list_outputs",
      "read_output",
      "search_outputs"
    ]);
    for (const tool of tools) {
      expect(tool.input_schema.type).toBe("object");
      expect(typeof tool.description).toBe("string");
    }
    expect(instructions.toLowerCase()).toContain("list_sessions");
  });

  it("dispatches list_sessions with coerced args", async () => {
    const { client, calls } = stubClient();
    const tools = createDataTools(client);
    const result = (await tools.execute("list_sessions", { limit: 5, status: "idle" })) as { sessions: unknown[] };
    expect(result.sessions).toHaveLength(1);
    expect(calls[0]).toEqual({ method: "list", args: [{ limit: 5, status: "idle" }] });
  });

  it("get_session returns only the safe summary subset (no submission snapshot)", async () => {
    const { client } = stubClient();
    const tools = createDataTools(client);
    const summary = (await tools.execute("get_session", { session_id: "run-1" })) as Record<string, unknown>;
    expect(summary).toMatchObject({ id: "run-1", status: "idle", costUsd: 0.42 });
    expect(summary).not.toHaveProperty("template_snapshot");
  });

  it("list_outputs returns lean metadata only", async () => {
    const { client } = stubClient();
    const tools = createDataTools(client);
    const outputs = (await tools.execute("list_outputs", { session_id: "run-1" })) as Array<Record<string, unknown>>;
    expect(outputs[0]).toEqual({ id: "out-1", filename: "report.md", sizeBytes: 12, contentType: "text/markdown" });
    expect(outputs[0]).not.toHaveProperty("extra");
  });

  it("read_output prefers a path selector and passes maxBytes/grep through", async () => {
    const { client, calls } = stubClient();
    const tools = createDataTools(client);
    const result = (await tools.execute("read_output", {
      session_id: "run-1",
      path: "report.md",
      max_bytes: 100,
      grep: "Report"
    })) as Record<string, unknown>;
    expect(result).toEqual({ path: "report.md", text: "# Report", truncated: false, totalBytes: 8 });
    expect(calls[0]!.args).toEqual(["run-1", { path: "report.md", match: "suffix" }, { maxBytes: 100, grep: "Report" }]);
  });

  it("throws DataToolError for unknown tools and missing args", async () => {
    const { client } = stubClient();
    const tools = createDataTools(client);
    await expect(tools.execute("nope", {})).rejects.toBeInstanceOf(DataToolError);
    await expect(tools.execute("get_session", {})).rejects.toBeInstanceOf(DataToolError);
    await expect(tools.execute("read_output", { session_id: "run-1" })).rejects.toBeInstanceOf(DataToolError);
  });
});
