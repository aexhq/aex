import { describe, expect, it, vi } from "vitest";
import { AntpathClient } from "../../src/index.js";

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
      filesRoot: "/mnt/session/uploads/antpath/files",
      assetsRoot: "/mnt/session/uploads/antpath/assets",
      outputsRoot: "/mnt/session/outputs",
      antpathCli: "/mnt/session/uploads/antpath/antpath",
      indexJson: "/mnt/session/uploads/antpath/index.json",
      readme: "/mnt/session/uploads/antpath/SKILLS.md",
      runtimeJson: "/mnt/session/uploads/antpath/RUNTIME.json",
      runtimeEnv: "/mnt/session/uploads/antpath/RUNTIME.env",
      envVars: {
        ANTPATH_CLI: "/mnt/session/uploads/antpath/antpath",
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
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch: fetchStub });
    const runId = await client.submitRun({
      model: "m",
      prompt: "p",
      secrets: { anthropic: { apiKey: "k" } },
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
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch: fetchStub });
    const runId = await client.submitRun({
      model: "m",
      prompt: "p",
      secrets: { anthropic: { apiKey: "k" } }
    });
    expect(runId).toBe("run_no_manifest");
    const run = await client.getRun(runId);
    expect(run.runtimeManifest).toBeUndefined();
  });
});
