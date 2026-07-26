/** `GET /api/whoami` has one canonical identity and admission-limits envelope. */
import { describe, expect, it } from "bun:test";
import { HttpClient } from "../src/http.js";
import { whoami } from "../src/operations.js";
import { RUNTIME_SIZES } from "../src/runtime-sizes.js";
import type { WhoAmI } from "../src/runtime-types.js";
import { runtimeProfileFixture, runtimeProfilesFixture } from "./runtime-profile-fixture.js";

function clientReturning(body: unknown): HttpClient {
  return new HttpClient({
    baseUrl: "https://api.example.test",
    apiKey: "test-token",
    fetch: async () =>
      new Response(JSON.stringify(body), {
        status: 200,
        headers: { "content-type": "application/json" }
      })
  });
}

const LIMITS: WhoAmI["limits"] = {
  maxConcurrentSessions: 50,
  submitRatePerMinute: 120,
  spendCapUsd: 250,
  monthSpendUsd: 12.5,
  balanceUsd: 100,
  balanceGraceFloorUsd: 0,
  llmTokenAllowanceRemainingUsd: 2,
  creditGateActive: true,
  paymentMethodStatus: "none",
  admissionState: "free",
  autoTopupEnabled: false,
  accountType: "standard"
};

/** The pre-prepaid envelope. Nothing serves it any more; nothing may parse it. */
const RETIRED_PLAN_LIMITS = {
  maxConcurrentSessions: 50,
  submitRatePerMinute: 120,
  spendCapUsd: 250,
  monthSpendUsd: 12.5,
  balanceUsd: 100,
  balanceGraceFloorUsd: 0,
  balanceGateActive: true,
  paymentMethodStatus: "none",
  planKey: "free",
  accountType: "standard",
  subscriptionStatus: "none",
  subscriptionGate: "ok"
};

const RUNTIME_CAPABILITIES: WhoAmI["runtimeCapabilities"] = {
  schemaVersion: 2,
  capabilityVersion: "runtime-capabilities.v2",
  capabilityHash: `sha256:${"a".repeat(64)}`,
  availableRuntimeKinds: ["container", "spot_container"],
  sizesByRuntimeKind: {
    container: [RUNTIME_SIZES[0]!],
    spot_container: [RUNTIME_SIZES[0]!]
  },
  unavailable: { lambda: { code: "runtime_unavailable" } },
  profilesByRuntimeKind: runtimeProfilesFixture()
};

describe("whoami limits typing", () => {
  it("parses the canonical whoami response", async () => {
    const result = await whoami(
      clientReturning({
        ok: true,
        principalType: "api_key",
        workspaceId: "ws_1",
        scopes: ["sessions:read"],
        limits: LIMITS
      })
    );
    expect(result.workspaceId).toBe("ws_1");
    expect(result.limits).toEqual(LIMITS);
    // The typed field narrows without casts.
    expect(result.limits.maxConcurrentSessions).toBe(50);
    expect(result.limits.submitRatePerMinute).toBe(120);
    expect(result.limits.spendCapUsd).toBe(250);
    expect(result.limits.balanceGraceFloorUsd).toBe(0);
    expect(result.limits.paymentMethodStatus).toBe("none");
  });

  it("tolerates additive server metadata while projecting the known contract", async () => {
    const result = await whoami(
      clientReturning({
        ok: true,
        principalType: "api_key",
        workspaceId: "ws_1",
        scopes: ["sessions:read"],
        serverRevision: "2026-07-11",
        limits: { ...LIMITS, futureBurstWindow: 30 }
      })
    );

    expect(result).toEqual({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: ["sessions:read"],
      limits: LIMITS
    });
  });

  it("parses the authenticated runtime capability projection", async () => {
    const result = await whoami(clientReturning({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: ["sessions:read"],
      limits: LIMITS,
      runtimeCapabilities: RUNTIME_CAPABILITIES
    }));

    expect(result.runtimeCapabilities).toEqual(RUNTIME_CAPABILITIES);
    expect(result.runtimeCapabilities?.availableRuntimeKinds).toEqual(["container", "spot_container"]);
  });

  it.each([
    { ...RUNTIME_CAPABILITIES, schemaVersion: 1 },
    { ...RUNTIME_CAPABILITIES, schemaVersion: 3 },
    { ...RUNTIME_CAPABILITIES, capabilityHash: "sha256:not-a-digest" },
    { ...RUNTIME_CAPABILITIES, availableRuntimeKinds: ["container", "container"] },
    { ...RUNTIME_CAPABILITIES, availableRuntimeKinds: ["container", "native"] },
    { ...RUNTIME_CAPABILITIES, sizesByRuntimeKind: { container: ["unknown-size"] } },
    {
      ...RUNTIME_CAPABILITIES,
      availableRuntimeKinds: ["container", "lambda"],
      sizesByRuntimeKind: { container: [RUNTIME_SIZES[0]!] },
      unavailable: { lambda: { code: "runtime_unavailable" }, spot_container: { code: "runtime_unavailable" } }
    },
    // A response that says WHICH runtimes exist but not what they DO is exactly the
    // gap the old public parity claim papered over. It is a contract violation.
    { ...RUNTIME_CAPABILITIES, profilesByRuntimeKind: undefined },
    { ...RUNTIME_CAPABILITIES, profilesByRuntimeKind: { container: runtimeProfileFixture("container") } },
    {
      ...RUNTIME_CAPABILITIES,
      profilesByRuntimeKind: {
        ...runtimeProfilesFixture(),
        lambda: { ...runtimeProfileFixture("lambda"), capabilities: { toolExecution: "partial" } }
      }
    },
    {
      ...RUNTIME_CAPABILITIES,
      profilesByRuntimeKind: {
        ...runtimeProfilesFixture(),
        container: { ...runtimeProfileFixture("container"), delivery: { toolExecution: "maybe-once" } }
      }
    }
  ])("rejects malformed or contradictory runtime capabilities", async (runtimeCapabilities) => {
    await expect(whoami(clientReturning({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: ["sessions:read"],
      limits: LIMITS,
      runtimeCapabilities
    }))).rejects.toThrow(/runtimeCapabilities/);
  });

  it.each([
    { ok: true, principalType: "api_key", workspaceId: "ws_1", scopes: [] },
    { ok: true, principalType: "api_key", workspaceId: "ws_1", scopes: [], caps: {}, limits: LIMITS },
    { ok: true, principalType: "api_key", workspaceId: "ws_1", scopes: [], tokenId: "legacy", limits: LIMITS },
    { ok: true, principalType: "api_key", workspaceId: "ws_1", scopes: [], tokenName: "legacy", limits: LIMITS },
    { ok: true, workspaceId: "ws_1", scopes: [], limits: LIMITS }
  ])("rejects incomplete or legacy identity envelopes", async (body) => {
    await expect(whoami(clientReturning(body))).rejects.toThrow();
  });

  it("rejects the retired plan/subscription limits envelope outright", async () => {
    // A deliberate breaking cut, not a widening: there is no plan catalog left to
    // report, so a body still carrying one is a deployment this SDK cannot read.
    await expect(whoami(clientReturning({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: ["sessions:read"],
      limits: RETIRED_PLAN_LIMITS
    }))).rejects.toThrow(/llmTokenAllowanceRemainingUsd/);
  });

  it.each([
    "llmTokenAllowanceRemainingUsd",
    "creditGateActive",
    "autoTopupEnabled",
    "admissionState"
  ] as const)("requires limits.%s", async (field) => {
    const { [field]: _dropped, ...partial } = LIMITS;
    await expect(whoami(clientReturning({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: ["sessions:read"],
      limits: partial
    }))).rejects.toThrow(new RegExp(field));
  });

  it("rejects an admission state outside the card-derived vocabulary", async () => {
    await expect(whoami(clientReturning({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: ["sessions:read"],
      limits: { ...LIMITS, admissionState: "pro" }
    }))).rejects.toThrow(/limits\.admissionState is invalid/);
  });

  it("rejects malformed known limit fields even alongside additive metadata", async () => {
    await expect(whoami(clientReturning({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: [],
      limits: { ...LIMITS, maxConcurrentSessions: "50", futureBurstWindow: 30 }
    }))).rejects.toThrow(/maxConcurrentSessions/);
  });
});
