import { describe, expect, it, vi } from "vitest";
import { Aex } from "../../src/index.js";

/**
 * SDK contract: runtimeManifest is accessed on the self-contained unit read
 * (`session.unit()` → GET /api/sessions/:id). Older BFFs may omit it, so callers
 * must treat the field as optional rather than relying on the create echo.
 */

function stubFetchReturning(args: { readonly createBody: unknown; readonly unitBody: unknown }): typeof fetch {
  return vi.fn(async (input, init) => {
    const method = init?.method ?? "GET";
    const body = method === "POST" ? args.createBody : args.unitBody;
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  }) as typeof fetch;
}

describe("SessionUnit.runtimeManifest — read from the unit record", () => {
  it("populates runtimeManifest when the BFF includes it on the unit read", async () => {
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
      createBody: { id: "ses_with_manifest", status: "queued" },
      unitBody: {
        id: "ses_with_manifest",
        status: "queued",
        runtimeManifest: manifest
      }
    });
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch: fetchStub });
    const session = await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "k" },
      environment: { variables: { BROLL_STORE: "/mnt/session/broll/store" } }
    });
    expect(session.id).toBe("ses_with_manifest");
    const unit = await session.unit();
    expect(unit.runtimeManifest).toEqual(manifest);
    expect(unit.runtimeManifest?.envVars.BROLL_STORE).toBe("/mnt/session/broll/store");
  });

  it("leaves runtimeManifest undefined when the BFF omits it (deployment skew)", async () => {
    const fetchStub = stubFetchReturning({
      createBody: { id: "ses_no_manifest", status: "queued" },
      unitBody: { id: "ses_no_manifest", status: "queued" }
    });
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch: fetchStub });
    const session = await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "k" }
    });
    expect(session.id).toBe("ses_no_manifest");
    const unit = await session.unit();
    expect(unit.runtimeManifest).toBeUndefined();
  });
});
