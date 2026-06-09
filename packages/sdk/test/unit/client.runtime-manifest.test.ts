import { describe, expect, it, vi } from "vitest";
import { AgentExecutor } from "../../src/index.js";

/**
 * SDK contract: runtimeManifest is accessed on the Run record returned by
 * `client.get(runId)` / `client.getRun(runId)`. Older BFFs may omit it, so
 * callers must treat the field as optional rather than relying on submit echo.
 */

function stubFetchReturning(args: { readonly submitBody: unknown; readonly getBody: unknown }): typeof fetch {
  return vi.fn(async (input, init) => {
    const method = init?.method ?? "GET";
    const body = method === "POST" ? args.submitBody : args.getBody;
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  }) as typeof fetch;
}

describe("Run.runtimeManifest — read from the run record", () => {
  it("populates runtimeManifest when the BFF includes it on get", async () => {
    const manifest = {
      provider: "anthropic" as const,
      skillsRoot: "/workspace/skills",
      filesRoot: "/mnt/session/uploads/aex/files",
      assetsRoot: "/mnt/session/uploads/aex/assets",
      aexCli: "/mnt/session/uploads/aex/aex",
      indexJson: "/mnt/session/uploads/aex/index.json",
      readme: "/mnt/session/uploads/aex/SKILLS.md",
      runtimeJson: "/mnt/session/uploads/aex/RUNTIME.json",
      runtimeEnv: "/mnt/session/uploads/aex/RUNTIME.env",
      envVars: {
        AEX_CLI: "/mnt/session/uploads/aex/aex",
        BROLL_STORE: "/mnt/session/broll/store"
      }
    };
    const fetchStub = stubFetchReturning({
      submitBody: { id: "run_with_manifest", status: "queued" },
      getBody: {
        id: "run_with_manifest",
        status: "queued",
        runtimeManifest: manifest
      }
    });
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch: fetchStub });
    const runId = await client.submitRun({
      model: "claude-haiku-4-5",
      prompt: "p",
      secrets: { apiKey: "k" },
      environment: { envVars: { BROLL_STORE: "/mnt/session/broll/store" } }
    });
    expect(runId).toBe("run_with_manifest");
    const run = await client.get(runId);
    expect(run.runtimeManifest).toEqual(manifest);
    expect(run.runtimeManifest?.envVars.BROLL_STORE).toBe("/mnt/session/broll/store");
  });

  it("leaves runtimeManifest undefined when the BFF omits it (deployment skew)", async () => {
    const fetchStub = stubFetchReturning({
      submitBody: { id: "run_no_manifest", status: "queued" },
      getBody: { id: "run_no_manifest", status: "queued" }
    });
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch: fetchStub });
    const runId = await client.submitRun({
      model: "claude-haiku-4-5",
      prompt: "p",
      secrets: { apiKey: "k" }
    });
    expect(runId).toBe("run_no_manifest");
    const run = await client.getRun(runId);
    expect(run.runtimeManifest).toBeUndefined();
  });
});
