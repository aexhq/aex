/**
 * Unit tests for `AntpathClient.getRunDebugLogs` / `AntpathClient.debugLogs`.
 *
 * The helper bundles the per-run debug artifacts antpath captures
 * automatically. These all live in the run's `logs` namespace, so the
 * helper lists `/logs` and
 * downloads each via the gated `/logs/:id/download` proxy (not anonymous
 * storage links), decodes textual content as UTF-8, and surfaces partial
 * failures via `errors` without abandoning the rest.
 */

import { describe, expect, it } from "vitest";
import { AntpathClient } from "../../src/index.js";

const RUN_ID = "run_debug_logs";
const STDERR_TEXT = "boot ok\nruntime spawn\nMCP loaded\n";
const HOST_TEXT = "host started\nrunner exited\n";
const ARGS_JSON = '{"command":"runtime","args":["run","--recipe","/workspace/recipe.yaml"]}';

const PROXY_DEBUG_JSON = '{"mcpName":"docs","status":200,"upstreamHost":"mcp.example.test"}';

// The `logs` namespace lists ONLY diagnostics — customer deliverables
// (outputs/report.md) live in the separate `outputs` namespace.
const LIST_BODY = {
  logs: [
    {
      id: "out_runtime_stderr",
      filename: "goose-logs/stderr.log",
      sizeBytes: STDERR_TEXT.length,
      contentType: "text/plain; charset=utf-8",
      createdAt: "2026-05-28T00:00:00.000Z"
    },
    {
      id: "out_runtime_args",
      filename: "goose-logs/args.json",
      sizeBytes: ARGS_JSON.length,
      contentType: "application/json",
      createdAt: "2026-05-28T00:00:00.001Z"
    },
    {
      id: "out_host_machine",
      filename: "fly-logs/machine.log",
      sizeBytes: HOST_TEXT.length,
      contentType: "text/plain; charset=utf-8",
      createdAt: "2026-05-28T00:00:00.002Z"
    },
    {
      id: "out_proxy_debug",
      filename: "anthropic-debug/mcp-access-1.log",
      sizeBytes: PROXY_DEBUG_JSON.length,
      contentType: "application/json",
      createdAt: "2026-05-28T00:00:00.004Z"
    }
  ]
};

describe("AntpathClient.getRunDebugLogs", () => {
  it("lists the logs namespace and decodes textual content as UTF-8", async () => {
    const calls: string[] = [];
    const stub: typeof fetch = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      calls.push(url);
      if (url.endsWith(`/api/runs/${RUN_ID}/logs`)) {
        return new Response(JSON.stringify(LIST_BODY), { status: 200, headers: { "content-type": "application/json" } });
      }
      if (url.endsWith("/logs/out_runtime_stderr/download")) {
        return new Response(STDERR_TEXT, { status: 200, headers: { "content-type": "text/plain; charset=utf-8" } });
      }
      if (url.endsWith("/logs/out_runtime_args/download")) {
        return new Response(ARGS_JSON, { status: 200, headers: { "content-type": "application/json" } });
      }
      if (url.endsWith("/logs/out_host_machine/download")) {
        return new Response(HOST_TEXT, { status: 200, headers: { "content-type": "text/plain; charset=utf-8" } });
      }
      if (url.endsWith("/logs/out_proxy_debug/download")) {
        return new Response(PROXY_DEBUG_JSON, { status: 200, headers: { "content-type": "application/json" } });
      }
      return new Response("no handler", { status: 500 });
    };
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://example.test", fetch: stub });
    const bundle = await client.getRunDebugLogs(RUN_ID);
    expect(bundle.runId).toBe(RUN_ID);
    expect(bundle.logs).toHaveLength(4);
    expect(bundle.errors).toHaveLength(0);
    const names = bundle.logs.map((l) => l.filename).sort();
    expect(names).toEqual([
      "host/machine.log",
      "provider-proxy/mcp-access-1.log",
      "runtime/args.json",
      "runtime/stderr.log"
    ]);
    const stderrLog = bundle.logs.find((l) => l.filename === "runtime/stderr.log")!;
    expect(stderrLog.text).toBe(STDERR_TEXT);
    expect(stderrLog.bytesBase64.length).toBeGreaterThan(0);
    // Hits the gated /logs/:id/download endpoint (NOT a signed URL or list-link round-trip).
    const downloads = calls.filter((u) => u.includes("/logs/") && u.endsWith("/download"));
    expect(downloads).toHaveLength(4);
  });

  it("surfaces a per-file download failure via `errors` without abandoning the rest", async () => {
    const stub: typeof fetch = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      if (url.endsWith(`/api/runs/${RUN_ID}/logs`)) {
        return new Response(JSON.stringify(LIST_BODY), { status: 200, headers: { "content-type": "application/json" } });
      }
      if (url.endsWith("/logs/out_runtime_args/download")) {
        // Broken JSON download for this one key.
        return new Response(JSON.stringify({ error: "internal" }), { status: 500, headers: { "content-type": "application/json" } });
      }
      if (url.endsWith("/logs/out_runtime_stderr/download")) {
        return new Response(STDERR_TEXT, { status: 200, headers: { "content-type": "text/plain; charset=utf-8" } });
      }
      if (url.endsWith("/logs/out_host_machine/download")) {
        return new Response(HOST_TEXT, { status: 200, headers: { "content-type": "text/plain; charset=utf-8" } });
      }
      if (url.endsWith("/logs/out_proxy_debug/download")) {
        return new Response(PROXY_DEBUG_JSON, { status: 200, headers: { "content-type": "application/json" } });
      }
      return new Response("no handler", { status: 500 });
    };
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://example.test", fetch: stub });
    const bundle = await client.getRunDebugLogs(RUN_ID);
    expect(bundle.logs).toHaveLength(3);
    expect(bundle.errors).toHaveLength(1);
    expect(bundle.errors[0]?.filename).toBe("runtime/args.json");
  });

  it("returns an empty bundle when the logs namespace is empty", async () => {
    const stub: typeof fetch = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      if (url.endsWith(`/api/runs/${RUN_ID}/logs`)) {
        return new Response(JSON.stringify({ logs: [] }), { status: 200, headers: { "content-type": "application/json" } });
      }
      return new Response("no handler", { status: 500 });
    };
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://example.test", fetch: stub });
    const bundle = await client.getRunDebugLogs(RUN_ID);
    expect(bundle.logs).toHaveLength(0);
    expect(bundle.errors).toHaveLength(0);
  });
});
