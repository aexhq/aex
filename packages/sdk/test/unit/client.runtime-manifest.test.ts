import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";

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
  envVars: { BROLL_STORE: "/mnt/session/broll/store" }
};

function clientFor(session: Record<string, unknown>): Aex {
  const fetch: typeof globalThis.fetch = async () => new Response(JSON.stringify({ session }), {
    status: 200,
    headers: { "content-type": "application/json" }
  });
  return new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
}

describe("Session.runtimeManifest", () => {
  it("reads runtimeManifest directly from the canonical session record", async () => {
    const session = await clientFor({
      id: "ses_with_manifest",
      status: "idle",
      acceptsMessages: true,
      runtimeManifest: manifest
    }).sessions.open("ses_with_manifest");

    expect(session.record.runtimeManifest).toEqual(manifest);
    expect(session.record.runtimeManifest?.envVars.BROLL_STORE).toBe("/mnt/session/broll/store");
  });

  it("leaves runtimeManifest undefined when the session record omits it", async () => {
    const session = await clientFor({
      id: "ses_no_manifest",
      status: "idle",
      acceptsMessages: true
    }).sessions.open("ses_no_manifest");

    expect(session.record.runtimeManifest).toBeUndefined();
  });
});
