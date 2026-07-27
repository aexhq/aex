import { describe, expect, it } from "bun:test";
import { CANONICAL_SHA256_DIGEST_PATTERN } from "../../packages/contracts/src/canonical-sha256.js";
// @ts-expect-error The JavaScript CI boundary is intentionally exercised directly.
import { parseRuntimeCapabilities } from "../cicd/runtime-capabilities.mjs";

const VALID_HASH = `sha256:${"a".repeat(64)}`;

/** One well-formed profile per kind. This parser checks shape, not published values. */
function profile(runtimeKind: string): Record<string, unknown> {
  return {
    schemaVersion: 1,
    runtimeKind,
    capabilities: { toolExecution: runtimeKind === "lambda" ? "unsupported" : "supported" },
    limits: { maxSessionMs: 28_800_000, maxSingleEffectMs: 840_000, maxWorkspaceBytes: 1, maxConcurrentToolCalls: 1 },
    delivery: {
      toolExecution: runtimeKind === "spot_container" ? "at-least-once" : "exactly-once",
      coldStartClass: "cold-seconds",
      idleBilling: runtimeKind === "lambda" ? "zero" : "wall-clock"
    },
    computeBasis: runtimeKind === "lambda" ? "microvm_running" : "wall_clock"
  };
}

function profiles(): Record<string, unknown> {
  return {
    container: profile("container"),
    spot_container: profile("spot_container"),
    lambda: profile("lambda")
  };
}

function projection(capabilityHash: unknown = VALID_HASH): Record<string, unknown> {
  return {
    schemaVersion: 1,
    capabilityVersion: "runtime-capabilities.v1",
    capabilityHash,
    availableRuntimeKinds: ["container", "spot_container"],
    sizesByRuntimeKind: {
      container: ["0.25cpu-1gb"],
      spot_container: ["shared-0.5x-4gb"]
    },
    unavailable: { lambda: { code: "not_enabled" } },
    profilesByRuntimeKind: profiles()
  };
}

describe("authenticated runtime-capability parser", () => {
  it("preserves the complete validated frozen projection", () => {
    const parsed = parseRuntimeCapabilities(projection());
    expect(parsed).toEqual({
      schemaVersion: 1,
      capabilityVersion: "runtime-capabilities.v1",
      capabilityHash: VALID_HASH,
      availableRuntimeKinds: ["container", "spot_container"],
      sizesByRuntimeKind: {
        container: ["0.25cpu-1gb"],
        spot_container: ["0.5cpu-4gb"]
      },
      unavailable: { lambda: { code: "not_enabled" } },
      profilesByRuntimeKind: profiles()
    });
    expect(Object.isFrozen(parsed)).toBe(true);
    expect(Object.isFrozen(parsed.availableRuntimeKinds)).toBe(true);
    expect(Object.isFrozen(parsed.sizesByRuntimeKind)).toBe(true);
  });

  it.each([
    ["uppercase prefix", `SHA256:${"a".repeat(64)}`],
    ["uppercase hex", `sha256:${"A".repeat(64)}`],
    ["bare digest", "a".repeat(64)],
    ["alternate algorithm", `sha512:${"a".repeat(64)}`],
    ["63 characters", `sha256:${"a".repeat(63)}`],
    ["65 characters", `sha256:${"a".repeat(65)}`],
    ["invalid hex", `sha256:${"g".repeat(64)}`],
    ["leading whitespace", ` ${VALID_HASH}`],
    ["trailing whitespace", `${VALID_HASH} `],
    ["trailing newline", `${VALID_HASH}\n`],
    ["prefix text", `x${VALID_HASH}`],
    ["suffix text", `${VALID_HASH}x`],
    ["empty", ""]
  ])("rejects non-canonical capabilityHash: %s", (_name, capabilityHash) => {
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test(capabilityHash)).toBe(false);
    expect(() => parseRuntimeCapabilities(projection(capabilityHash))).toThrowError(
      "invalid runtimeCapabilities: capabilityHash must be a lowercase sha256 digest"
    );
  });

  it("accepts exactly the owner language for the exercised boundary corpus", () => {
    const samples = [VALID_HASH, `sha256:${"0".repeat(64)}`, `sha256:${"f".repeat(64)}`, "sha256:", ""];
    for (const sample of samples) {
      const accepted = (() => {
        try {
          parseRuntimeCapabilities(projection(sample));
          return true;
        } catch {
          return false;
        }
      })();
      expect(accepted, sample).toBe(CANONICAL_SHA256_DIGEST_PATTERN.test(sample));
    }
  });

  it("keeps field validation ordered before capabilityHash", () => {
    expect(() => parseRuntimeCapabilities({ ...projection("bad"), schemaVersion: 0 })).toThrowError(
      "invalid runtimeCapabilities: schemaVersion must be 1"
    );
    expect(() => parseRuntimeCapabilities({ ...projection("bad"), capabilityVersion: "" })).toThrowError(
      "invalid runtimeCapabilities: capabilityVersion must be a non-empty string"
    );
  });

  it("refuses a projection that lists runtimes without saying what they do", () => {
    // A capability document that declares availability but not capability is exactly
    // the gap the retired public parity claim papered over; CI must stop on it.
    const { profilesByRuntimeKind: _omitted, ...withoutProfiles } = projection();
    expect(() => parseRuntimeCapabilities(withoutProfiles)).toThrowError(
      "invalid runtimeCapabilities: profilesByRuntimeKind must be an object"
    );
    expect(() => parseRuntimeCapabilities({
      ...projection(),
      profilesByRuntimeKind: { container: profile("container") }
    })).toThrowError("invalid runtimeCapabilities: profilesByRuntimeKind.spot_container must be an object");
    expect(() => parseRuntimeCapabilities({
      ...projection(),
      profilesByRuntimeKind: { ...profiles(), native: profile("native") }
    })).toThrowError("invalid runtimeCapabilities: profilesByRuntimeKind contains unknown runtime native");
  });
});
