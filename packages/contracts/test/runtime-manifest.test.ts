import { describe, expect, it } from "vitest";
import { buildRuntimeManifest, runtimePathsFor } from "../src/index.js";

describe("buildRuntimeManifest — Anthropic provider", () => {
  it("emits the Anthropic container paths verbatim", () => {
    const m = buildRuntimeManifest({ provider: "anthropic" });
    expect(m.provider).toBe("anthropic");
    expect(m.skillsRoot).toBe("/workspace/skills");
    expect(m.filesRoot).toBe("/mnt/session/uploads/antpath/files");
    expect(m.assetsRoot).toBe("/mnt/session/uploads/antpath/assets");
    expect(m.outputsRoot).toBe("/mnt/session/outputs");
    expect(m.antpathCli).toBe("/mnt/session/uploads/antpath/antpath");
    expect(m.indexJson).toBe("/mnt/session/uploads/antpath/index.json");
    expect(m.readme).toBe("/mnt/session/uploads/antpath/SKILLS.md");
    expect(m.runtimeJson).toBe("/mnt/session/uploads/antpath/RUNTIME.json");
    expect(m.runtimeEnv).toBe("/mnt/session/uploads/antpath/RUNTIME.env");
  });

  it("populates the antpath-set env vars from the same path table", () => {
    const m = buildRuntimeManifest({ provider: "anthropic" });
    expect(m.envVars.ANTPATH_PROVIDER).toBe("anthropic");
    expect(m.envVars.ANTPATH_CLI).toBe(m.antpathCli);
    expect(m.envVars.ANTPATH_OUTPUTS).toBe(m.outputsRoot);
    expect(m.envVars.ANTPATH_SKILLS_ROOT).toBe(m.skillsRoot);
    expect(m.envVars.ANTPATH_FILES_ROOT).toBe(m.filesRoot);
    expect(m.envVars.ANTPATH_ASSETS_ROOT).toBe(m.assetsRoot);
    expect(m.envVars.ANTPATH_INDEX_JSON).toBe(m.indexJson);
    expect(m.envVars.ANTPATH_README).toBe(m.readme);
    expect(m.envVars.ANTPATH_RUNTIME_JSON).toBe(m.runtimeJson);
    expect(m.envVars.ANTPATH_RUNTIME_ENV).toBe(m.runtimeEnv);
  });

  it("merges customer env vars after antpath keys in insertion order", () => {
    const m = buildRuntimeManifest({
      provider: "anthropic",
      customerEnvVars: {
        BROLL_STORE: "/mnt/session/broll/store",
        BROLL_OUTPUTS: "/mnt/session/outputs"
      }
    });
    const keys = Object.keys(m.envVars);
    expect(keys[0]).toBe("ANTPATH_PROVIDER");
    expect(keys.includes("BROLL_STORE")).toBe(true);
    expect(keys.includes("BROLL_OUTPUTS")).toBe(true);
    // antpath keys come first, customer keys afterwards
    const brollIndex = keys.indexOf("BROLL_STORE");
    const lastAntpathIndex = Math.max(...keys.map((k, i) => (k.startsWith("ANTPATH_") ? i : -1)));
    expect(brollIndex).toBeGreaterThan(lastAntpathIndex);
    expect(m.envVars.BROLL_STORE).toBe("/mnt/session/broll/store");
  });

  it("defensively drops customer keys that smuggle in the reserved ANTPATH_ prefix", () => {
    // The strict submission parser rejects this; the builder still filters
    // as a defence-in-depth layer so a poisoned snapshot can't shadow our
    // values inside the container.
    const m = buildRuntimeManifest({
      provider: "anthropic",
      customerEnvVars: { ANTPATH_OUTPUTS: "/elsewhere", BROLL_STORE: "/x" } as Record<string, string>
    });
    expect(m.envVars.ANTPATH_OUTPUTS).toBe("/mnt/session/outputs");
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
