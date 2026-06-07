import { describe, expect, it } from "vitest";
import {
  PROXY_PROTOCOL_VERSION,
  PROXY_ALLOWED_METHODS,
  PROXY_RESPONSE_MODES,
  PROXY_ERROR_CODES,
  PROXY_ERROR_HTTP_STATUS,
  PROXY_STRIPPED_INBOUND_HEADERS,
  authShapeHeaderName,
  authShapeQueryName,
  narrowResponseMode,
  type ProxyAuthShape,
  type ProxyResponseMode
} from "../src/proxy-protocol.js";

describe("proxy protocol version", () => {
  it("is a non-empty stable string", () => {
    expect(typeof PROXY_PROTOCOL_VERSION).toBe("string");
    expect(PROXY_PROTOCOL_VERSION.length).toBeGreaterThan(0);
    expect(PROXY_PROTOCOL_VERSION).toBe("1");
  });

  it("has every error code mapped to an HTTP status", () => {
    for (const code of PROXY_ERROR_CODES) {
      expect(PROXY_ERROR_HTTP_STATUS[code]).toBeTypeOf("number");
      expect(PROXY_ERROR_HTTP_STATUS[code]).toBeGreaterThanOrEqual(400);
      expect(PROXY_ERROR_HTTP_STATUS[code]).toBeLessThan(600);
    }
  });

  it("allows the expected HTTP methods", () => {
    expect(PROXY_ALLOWED_METHODS).toEqual(["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"]);
  });

  it("orders response modes from narrowest to widest", () => {
    expect(PROXY_RESPONSE_MODES).toEqual(["status_only", "headers_only", "full"]);
  });
});

describe("narrowResponseMode", () => {
  // Property-style: the result is always `narrower-or-equal` to BOTH inputs.
  const widths: Record<ProxyResponseMode, number> = {
    status_only: 0,
    headers_only: 1,
    full: 2
  };

  it("returns the narrower mode when policy is wider than requested", () => {
    expect(narrowResponseMode("full", "headers_only")).toBe("headers_only");
    expect(narrowResponseMode("full", "status_only")).toBe("status_only");
    expect(narrowResponseMode("headers_only", "status_only")).toBe("status_only");
  });

  it("clamps to the policy ceiling when requested is wider", () => {
    expect(narrowResponseMode("headers_only", "full")).toBe("headers_only");
    expect(narrowResponseMode("status_only", "full")).toBe("status_only");
    expect(narrowResponseMode("status_only", "headers_only")).toBe("status_only");
  });

  it("returns the policy when both are equal", () => {
    for (const mode of PROXY_RESPONSE_MODES) {
      expect(narrowResponseMode(mode, mode)).toBe(mode);
    }
  });

  it("is never wider than either input", () => {
    for (const policy of PROXY_RESPONSE_MODES) {
      for (const requested of PROXY_RESPONSE_MODES) {
        const result = narrowResponseMode(policy, requested);
        expect(widths[result]).toBeLessThanOrEqual(widths[policy]);
        expect(widths[result]).toBeLessThanOrEqual(widths[requested]);
      }
    }
  });
});

describe("PROXY_STRIPPED_INBOUND_HEADERS", () => {
  // The canonical inbound-strip set. The api provider-proxy and the
  // dashboard MCP proxy strip exactly this; the dashboard customer HTTP
  // proxy derives this MINUS `x-api-key` (a legitimate per-endpoint auth
  // carrier there). Locking the membership here is the single tripwire that
  // stops those surfaces drifting apart again.
  const EXPECTED = [
    // credential carriers
    "authorization",
    "x-api-key",
    "cookie",
    "proxy-authorization",
    // hop-by-hop (RFC 7230 §6.1)
    "connection",
    "keep-alive",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "expect",
    "proxy-authenticate",
    "proxy-connection",
    // routing primitives a runner could spoof
    "host",
    "content-length",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-forwarded-port",
    "x-real-ip",
    "forwarded"
  ];

  it("contains exactly the canonical membership (no drift)", () => {
    expect([...PROXY_STRIPPED_INBOUND_HEADERS].sort()).toEqual([...EXPECTED].sort());
  });

  it("strips every credential carrier", () => {
    for (const h of ["authorization", "x-api-key", "cookie", "proxy-authorization"]) {
      expect(PROXY_STRIPPED_INBOUND_HEADERS.has(h)).toBe(true);
    }
  });

  it("strips the full RFC 7230 §6.1 hop-by-hop set", () => {
    for (const h of [
      "connection", "keep-alive", "transfer-encoding", "te", "trailer",
      "upgrade", "expect", "proxy-authenticate", "proxy-connection"
    ]) {
      expect(PROXY_STRIPPED_INBOUND_HEADERS.has(h)).toBe(true);
    }
  });

  it("strips every spoofable routing primitive (IP-allowlist / rate-limit bypass)", () => {
    for (const h of [
      "host", "content-length", "x-forwarded-for", "x-forwarded-host",
      "x-forwarded-proto", "x-forwarded-port", "x-real-ip", "forwarded"
    ]) {
      expect(PROXY_STRIPPED_INBOUND_HEADERS.has(h)).toBe(true);
    }
  });

  it("regression: covers every header that previously drifted between planes", () => {
    // Before consolidation the dashboard customer-proxy deny list omitted
    // x-api-key / keep-alive / proxy-authenticate / x-forwarded-port /
    // forwarded, and the MCP proxy omitted x-api-key / expect /
    // proxy-connection / x-forwarded-port / forwarded. The shared set must
    // cover the union so no plane can under-strip a dangerous header.
    for (const h of [
      "x-api-key", "keep-alive", "proxy-authenticate", "proxy-connection",
      "expect", "x-forwarded-port", "forwarded"
    ]) {
      expect(PROXY_STRIPPED_INBOUND_HEADERS.has(h)).toBe(true);
    }
  });

  it("stores all names lowercase (lookups lowercase the inbound key)", () => {
    for (const h of PROXY_STRIPPED_INBOUND_HEADERS) {
      expect(h).toBe(h.toLowerCase());
    }
  });
});

describe("authShapeHeaderName", () => {
  it("returns 'authorization' for bearer and basic auth", () => {
    expect(authShapeHeaderName({ type: "bearer" })).toBe("authorization");
    expect(authShapeHeaderName({ type: "basic" })).toBe("authorization");
  });

  it("returns the configured (lowercased) name for header auth", () => {
    expect(authShapeHeaderName({ type: "header", name: "X-API-Key" })).toBe("x-api-key");
    expect(authShapeHeaderName({ type: "header", name: "stripe-signature" })).toBe(
      "stripe-signature"
    );
  });

  it("returns undefined for query-string auth", () => {
    expect(authShapeHeaderName({ type: "query", name: "api_key" })).toBeUndefined();
  });

  it("returns undefined for keyless (none) auth", () => {
    expect(authShapeHeaderName({ type: "none" })).toBeUndefined();
  });
});

describe("authShapeQueryName", () => {
  it("returns the configured name for query auth", () => {
    expect(authShapeQueryName({ type: "query", name: "api_key" })).toBe("api_key");
  });

  it("returns undefined for non-query shapes", () => {
    const shapes: ProxyAuthShape[] = [
      { type: "none" },
      { type: "bearer" },
      { type: "basic" },
      { type: "header", name: "X-Auth" }
    ];
    for (const shape of shapes) {
      expect(authShapeQueryName(shape)).toBeUndefined();
    }
  });
});
