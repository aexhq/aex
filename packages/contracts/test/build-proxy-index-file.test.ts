import { describe, expect, it } from "vitest";
import {
  buildProxyIndexFile,
  PROXY_ENDPOINT_DEFAULTS,
  PROXY_PROTOCOL_VERSION,
  type ProxyEndpointPolicy
} from "../src/proxy-protocol.js";

const RUN_ID = "11111111-2222-3333-4444-555555555555";
const BASE = "https://aex.local.test";

const minimalEndpoint: ProxyEndpointPolicy = {
  name: "httpbin",
  baseUrl: "https://httpbin.org",
  authShape: { type: "bearer" },
  allowMethods: ["GET"],
  allowPathPrefixes: ["/"]
};

describe("buildProxyIndexFile", () => {
  it("always emits a file; zero endpoints → proxyBaseUrl null and endpoints []", () => {
    const file = buildProxyIndexFile({ runId: RUN_ID, proxyPublicBaseUrl: BASE });
    expect(file).toEqual({
      protocolVersion: PROXY_PROTOCOL_VERSION,
      runId: RUN_ID,
      proxyBaseUrl: null,
      endpoints: []
    });
  });

  it("composes proxyBaseUrl as <base>/api/runs/<runId>/proxy (trailing slash trimmed)", () => {
    const file = buildProxyIndexFile({
      runId: RUN_ID,
      proxyPublicBaseUrl: `${BASE}///`,
      endpoints: [minimalEndpoint]
    });
    expect(file.proxyBaseUrl).toBe(`${BASE}/api/runs/${RUN_ID}/proxy`);
  });

  it("returns proxyBaseUrl null when endpoints exist but no base url is supplied", () => {
    const file = buildProxyIndexFile({ runId: RUN_ID, endpoints: [minimalEndpoint] });
    expect(file.proxyBaseUrl).toBeNull();
    expect(file.endpoints).toHaveLength(1);
  });

  it("applies PROXY_ENDPOINT_DEFAULTS so every optional cap is concrete", () => {
    const file = buildProxyIndexFile({
      runId: RUN_ID,
      proxyPublicBaseUrl: BASE,
      endpoints: [minimalEndpoint]
    });
    const entry = file.endpoints[0]!;
    expect(entry).toEqual({
      name: "httpbin",
      baseUrl: "https://httpbin.org",
      authShape: { type: "bearer" },
      allowMethods: ["GET"],
      allowPathPrefixes: ["/"],
      allowHeaders: PROXY_ENDPOINT_DEFAULTS.allowHeaders,
      responseMode: PROXY_ENDPOINT_DEFAULTS.responseMode,
      maxRequestBytes: PROXY_ENDPOINT_DEFAULTS.maxRequestBytes,
      maxResponseBytes: PROXY_ENDPOINT_DEFAULTS.maxResponseBytes,
      timeoutMs: PROXY_ENDPOINT_DEFAULTS.timeoutMs,
      perCallBudget: PROXY_ENDPOINT_DEFAULTS.perCallBudget,
      responseByteBudget: PROXY_ENDPOINT_DEFAULTS.responseByteBudget
    });
  });

  it("preserves explicit caps over defaults", () => {
    const file = buildProxyIndexFile({
      runId: RUN_ID,
      proxyPublicBaseUrl: BASE,
      endpoints: [
        {
          ...minimalEndpoint,
          responseMode: "full",
          timeoutMs: 5_000,
          perCallBudget: 3,
          allowHeaders: ["x-custom"]
        }
      ]
    });
    const entry = file.endpoints[0]!;
    expect(entry.responseMode).toBe("full");
    expect(entry.timeoutMs).toBe(5_000);
    expect(entry.perCallBudget).toBe(3);
    expect(entry.allowHeaders).toEqual(["x-custom"]);
  });

  it("never carries any secret/auth value in the serialized file", () => {
    const file = buildProxyIndexFile({
      runId: RUN_ID,
      proxyPublicBaseUrl: BASE,
      endpoints: [
        { ...minimalEndpoint, authShape: { type: "header", name: "x-api-key" } }
      ]
    });
    const json = JSON.stringify(file);
    // The shape only describes HOW auth attaches (authShape), never the value.
    expect(file.endpoints[0]!.authShape).toEqual({ type: "header", name: "x-api-key" });
    expect(json).not.toMatch(/token|secret|bearer.*[A-Za-z0-9]{16}/i);
  });
});
