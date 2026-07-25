import { describe, expect, it } from "bun:test";
import {
  CANONICAL_SHA256_DIGEST_PATTERN,
  INLINE_CONTENT_HASH_PATTERN,
  assertPinnedWorkspaceResource
} from "../src/index.js";
import { HttpClient } from "../src/http.js";
import { whoami } from "../src/operations.js";
import { RUNTIME_SIZES } from "../src/runtime-sizes.js";
import { runtimeProfilesFixture } from "./runtime-profile-fixture.js";

const canonicalDigest = `sha256:${"0123456789abcdef".repeat(4)}`;

const runtimeCapabilities = {
  schemaVersion: 2,
  capabilityVersion: "runtime-capabilities.v2",
  capabilityHash: canonicalDigest,
  availableRuntimeKinds: ["container"],
  sizesByRuntimeKind: { container: [RUNTIME_SIZES[0]!] },
  unavailable: {
    spot_container: { code: "runtime_unavailable" },
    lambda: { code: "runtime_unavailable" }
  },
  profilesByRuntimeKind: runtimeProfilesFixture()
} as const;

const limits = {
  maxConcurrentSessions: 1,
  submitRatePerMinute: 1,
  spendCapUsd: 1,
  monthSpendUsd: 0,
  balanceUsd: 1,
  balanceGraceFloorUsd: 0,
  balanceGateActive: true,
  paymentMethodStatus: "none",
  planKey: "free",
  accountType: "standard",
  subscriptionStatus: "none",
  subscriptionGate: "ok"
} as const;

function pinnedResource(contentHash: string) {
  return {
    kind: "file" as const,
    resourceId: `wres_${"1".repeat(32)}`,
    version: 1,
    assetId: `asset_${contentHash.slice("sha256:".length)}`,
    contentHash,
    name: "input.txt",
    mountPath: "/workspace"
  };
}

function whoamiClient(capabilityHash: string): HttpClient {
  return new HttpClient({
    baseUrl: "https://api.example.test",
    apiKey: "test-token",
    fetch: async () => new Response(JSON.stringify({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: ["sessions:read"],
      limits,
      runtimeCapabilities: { ...runtimeCapabilities, capabilityHash }
    }), { headers: { "content-type": "application/json" } })
  });
}

describe("canonical SHA-256 digest grammar", () => {
  it("retains INLINE_CONTENT_HASH_PATTERN as the same public RegExp object", () => {
    expect(INLINE_CONTENT_HASH_PATTERN).toBe(CANONICAL_SHA256_DIGEST_PATTERN);
  });

  it.each([
    `sha256:${"0".repeat(64)}`,
    `sha256:${"a".repeat(64)}`,
    `sha256:${"f".repeat(64)}`,
    canonicalDigest
  ])("accepts canonical lowercase wire value %s", async (digest) => {
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test(digest)).toBe(true);
    expect(() => assertPinnedWorkspaceResource(pinnedResource(digest), "assets.files[0]")).not.toThrow();
    await expect(whoami(whoamiClient(digest))).resolves.toMatchObject({
      runtimeCapabilities: { capabilityHash: digest }
    });
  });

  it.each([
    `SHA256:${"a".repeat(64)}`,
    `sha256:${"A".repeat(64)}`,
    "a".repeat(64),
    `sha512:${"a".repeat(64)}`,
    `sha256:${"a".repeat(63)}`,
    `sha256:${"a".repeat(65)}`,
    `sha256:${"g".repeat(64)}`,
    ` sha256:${"a".repeat(64)}`,
    `sha256:${"a".repeat(64)}\n`,
    `prefix-sha256:${"a".repeat(64)}`,
    `sha256:${"a".repeat(64)}-suffix`,
    ""
  ])("rejects non-canonical wire value %j in every consumer", async (digest) => {
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test(digest)).toBe(false);
    expect(() => assertPinnedWorkspaceResource(pinnedResource(digest), "assets.files[0]"))
      .toThrow("assets.files[0].contentHash must be a sha256 digest");
    await expect(whoami(whoamiClient(digest)))
      .rejects.toThrow("whoami response runtimeCapabilities.capabilityHash must be a canonical SHA-256 digest");
  });

  it("preserves the workspace-specific asset identity check", () => {
    expect(() => assertPinnedWorkspaceResource({
      ...pinnedResource(canonicalDigest),
      assetId: `asset_${"b".repeat(64)}`
    }, "assets.files[0]"))
      .toThrow("assets.files[0].assetId must identify the same bytes as contentHash");
  });
});
