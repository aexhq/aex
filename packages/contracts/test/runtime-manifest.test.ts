import { describe, expect, it } from "vitest";
import { buildRuntimeManifest, runtimePathsFor } from "../src/index.js";

describe("buildRuntimeManifest — Anthropic provider", () => {
  it("emits the Anthropic container paths verbatim", () => {
    const m = buildRuntimeManifest({ provider: "anthropic" });
    expect(m.provider).toBe("anthropic");
    expect(m.skillsRoot).toBe("/workspace/skills");
    expect(m.filesRoot).toBe("/mnt/session/uploads/aex/files");
    expect(m.assetsRoot).toBe("/mnt/session/uploads/aex/assets");
    expect(m.aexCli).toBe("/mnt/session/uploads/aex/aex");
    expect(m.indexJson).toBe("/mnt/session/uploads/aex/index.json");
    expect(m.readme).toBe("/mnt/session/uploads/aex/SKILLS.md");
    expect(m.runtimeJson).toBe("/mnt/session/uploads/aex/RUNTIME.json");
    expect(m.runtimeEnv).toBe("/mnt/session/uploads/aex/RUNTIME.env");
  });

  it("populates the aex-set env vars from the same path table", () => {
    const m = buildRuntimeManifest({ provider: "anthropic" });
    expect(m.envVars.AEX_PROVIDER).toBe("anthropic");
    expect(m.envVars.AEX_CLI).toBe(m.aexCli);
    expect(m.envVars.AEX_SKILLS_ROOT).toBe(m.skillsRoot);
    expect(m.envVars.AEX_FILES_ROOT).toBe(m.filesRoot);
    expect(m.envVars.AEX_ASSETS_ROOT).toBe(m.assetsRoot);
    expect(m.envVars.AEX_INDEX_JSON).toBe(m.indexJson);
    expect(m.envVars.AEX_README).toBe(m.readme);
    expect(m.envVars.AEX_RUNTIME_JSON).toBe(m.runtimeJson);
    expect(m.envVars.AEX_RUNTIME_ENV).toBe(m.runtimeEnv);
  });

  it("merges customer env vars after aex keys in insertion order", () => {
    const m = buildRuntimeManifest({
      provider: "anthropic",
      customerEnvVars: {
        BROLL_STORE: "/mnt/session/broll/store",
        BROLL_CACHE: "/mnt/session/broll/cache"
      }
    });
    const keys = Object.keys(m.envVars);
    expect(keys[0]).toBe("AEX_PROVIDER");
    expect(keys.includes("BROLL_STORE")).toBe(true);
    expect(keys.includes("BROLL_CACHE")).toBe(true);
    // aex keys come first, customer keys afterwards
    const brollIndex = keys.indexOf("BROLL_STORE");
    const lastAexIndex = Math.max(...keys.map((k, i) => (k.startsWith("AEX_") ? i : -1)));
    expect(brollIndex).toBeGreaterThan(lastAexIndex);
    expect(m.envVars.BROLL_STORE).toBe("/mnt/session/broll/store");
  });

  it("defensively drops customer keys that smuggle in the reserved AEX_ prefix", () => {
    // The strict submission parser rejects this; the builder still filters
    // as a defence-in-depth layer so a poisoned snapshot can't shadow our
    // values inside the container.
    const m = buildRuntimeManifest({
      provider: "anthropic",
      customerEnvVars: { AEX_CLI: "/elsewhere", BROLL_STORE: "/x" } as Record<string, string>
    });
    expect(m.envVars.AEX_CLI).toBe("/mnt/session/uploads/aex/aex");
    expect(m.envVars.BROLL_STORE).toBe("/x");
  });

  it("returns a frozen manifest and a frozen envVars map", () => {
    const m = buildRuntimeManifest({ provider: "anthropic" });
    expect(Object.isFrozen(m)).toBe(true);
    expect(Object.isFrozen(m.envVars)).toBe(true);
  });

  it("throws for an unknown provider", () => {
    expect(() => buildRuntimeManifest({ provider: "openai" as never })).toThrow(
      /Unknown runtime provider: openai/
    );
    expect(() => runtimePathsFor("deepseek" as never)).toThrow(
      /Unknown runtime provider: deepseek/
    );
  });
});
