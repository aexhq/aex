import { describe, expect, it } from "bun:test";
import {
  liveSessionStartDiagnostic,
  requireStartedSessionIdentity
} from "../_fixtures/live-session-start-result.js";

describe("live session start result guard", () => {
  it("rejects a caught start failure even when a missing result was labeled failed", () => {
    expect(() =>
      requireStartedSessionIdentity("edge BYOK case G", {
        sessionId: null,
        status: "failed",
        runErr: "client.start timed out"
      })
    ).toThrowError(
      'edge BYOK case G: client.start() did not return a session identity (status="failed", startError="client.start timed out")'
    );
  });

  it("requires a non-empty session identity and keeps unrelated output out of diagnostics", () => {
    expect(() =>
      requireStartedSessionIdentity("edge BYOK case G", {
        sessionId: "",
        status: null,
        text: "customer-secret-output"
      })
    ).toThrowError(
      "edge BYOK case G: client.start() did not return a session identity (status=null, startError=null)"
    );

    expect(
      liveSessionStartDiagnostic({
        sessionId: null,
        status: null,
        text: "customer-secret-output"
      })
    ).not.toContain("customer-secret-output");
  });

  it("returns a real session identity after a successful start", () => {
    expect(
      requireStartedSessionIdentity("edge BYOK case G", {
        sessionId: "ses_0123456789abcdef",
        status: "succeeded",
        runErr: null
      })
    ).toBe("ses_0123456789abcdef");
  });

  it("redacts secret-shaped and known canary values while retaining session context", () => {
    const providerKey = "sk-ant-0123456789abcdef0123456789abcdef";
    const apiKey = "apt_0123456789abcdef0123456789abcdef";
    const canary = "workspace-canary-value-0123456789";
    const result = {
      sessionId: "ses_0123456789abcdef0123456789abcdef",
      status: "failed",
      runErr: `upstream rejected ${providerKey}; token=${apiKey}; observed ${canary}`
    };

    const diagnostic = liveSessionStartDiagnostic(result, [canary]);
    expect(diagnostic).toContain("ses_0123456789abcdef0123456789abcdef");
    expect(diagnostic).toContain("failed");
    expect(diagnostic).toContain("[REDACTED]");
    expect(diagnostic).not.toContain(providerKey);
    expect(diagnostic).not.toContain(apiKey);
    expect(diagnostic).not.toContain(canary);

    expect(() =>
      requireStartedSessionIdentity("edge BYOK case G", result, [canary])
    ).toThrowError(/startError=.*\[REDACTED\]/);
  });
});
