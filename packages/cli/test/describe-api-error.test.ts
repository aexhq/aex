/**
 * describeApiError: the CLI's error envelope must not drop the useful parts
 * of a failure — the transport cause code (with a connectivity remedy) for
 * network errors, and the (truncated, redacted) response body for API errors
 * that carry fields beyond the standard `{ ok, error, code, message }`
 * envelope.
 */
import { describe, expect, it } from "vitest";
import { AexApiError, AexError, AexNetworkError } from "@aexhq/contracts";
import { describeApiError } from "../src/host/common.js";

/** Rejection shaped like undici's: bare TypeError with the code on `cause`. */
function undiciFetchFailed(code: string): TypeError {
  const cause = Object.assign(new Error(`connect ${code} 127.0.0.1:443`), { code });
  return new TypeError("fetch failed", { cause });
}

describe("describeApiError network failures", () => {
  it("includes the cause code and a connectivity remedy for ECONNREFUSED", () => {
    const err = new AexNetworkError({
      method: "POST",
      host: "api.example.test",
      path: "/api/runs",
      cause: undiciFetchFailed("ECONNREFUSED")
    });
    const d = describeApiError(err);
    expect(d.code).toBe("NETWORK_ERROR");
    expect(d.message).toContain("ECONNREFUSED");
    expect(d.remedy).toContain("--aex-url");
  });

  it("suggests connectivity checks for ENOTFOUND too", () => {
    const err = new AexNetworkError({
      method: "GET",
      host: "api.example.test",
      path: "/api/whoami",
      cause: undiciFetchFailed("ENOTFOUND")
    });
    const d = describeApiError(err);
    expect(d.message).toContain("ENOTFOUND");
    expect(d.remedy).toContain("--aex-url");
  });

  it("surfaces the cause code on a plain AexError whose cause carries one", () => {
    const err = new AexError("API_ERROR", "request failed", undefined, {
      cause: undiciFetchFailed("ECONNRESET")
    });
    const d = describeApiError(err);
    expect(d.message).toContain("ECONNRESET");
    expect(d.remedy).toContain("--aex-url");
  });
});

describe("describeApiError API error bodies", () => {
  it("appends body fields beyond the standard envelope to the message", () => {
    const body = { ok: false, code: "quota_exceeded", message: "workspace quota exceeded", hint: "upgrade plan" };
    const err = new AexApiError(402, "workspace quota exceeded", body);
    const d = describeApiError(err);
    expect(d.status).toBe(402);
    expect(d.message).toContain("workspace quota exceeded");
    expect(d.message).toContain("upgrade plan");
  });

  it("leaves the message untouched when the body is just the standard envelope", () => {
    const body = { error: "asset_not_found", message: "asset not found" };
    const err = new AexApiError(404, "asset_not_found", body);
    const d = describeApiError(err);
    expect(d.message).toBe("asset_not_found");
    expect(d.remedy).toBe("no such run/resource — verify the id");
  });

  it("appends ONLY the non-standard body keys — never re-serializes error/message", () => {
    // Every current-plane envelope carries requestId; the append must surface it
    // without duplicating the code+message already extracted into `message`.
    const body = { error: "malformed_token", message: "the bearer token is malformed", requestId: "req-123" };
    const err = new AexApiError(400, "malformed_token: the bearer token is malformed", body);
    const d = describeApiError(err);
    expect(d.message).toBe('malformed_token: the bearer token is malformed — {"requestId":"req-123"}');
  });

  it("surfaces requiredScope-style extras without the standard envelope noise", () => {
    const body = {
      error: "insufficient_scope",
      message: "the token does not carry the scope this route requires",
      requiredScope: "billing:read",
      requestId: "req-9"
    };
    const err = new AexApiError(403, "insufficient_scope: the token does not carry the scope this route requires", body);
    const d = describeApiError(err);
    expect(d.message).toContain('"requiredScope":"billing:read"');
    expect(d.message).toContain('"requestId":"req-9"');
    expect(d.message.indexOf("the token does not carry")).toBe(d.message.lastIndexOf("the token does not carry"));
  });

  it("truncates giant bodies and keeps secrets redacted", () => {
    const body = { detail: "x".repeat(2000), apiKey: `sk-ant-${"a".repeat(24)}` };
    const err = new AexApiError(500, "server exploded", body);
    const d = describeApiError(err);
    expect(d.message.length).toBeLessThan(700);
    expect(d.message).not.toContain("sk-ant-");
  });
});
