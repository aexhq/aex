/**
 * `session-artifacts` is the single source of truth for routing a session's artifacts
 * into the public `outputs` prefix or internal `internal/logs` prefix.
 * Diagnostics are stored under canonical log namespaces.
 */
import { describe, expect, it } from "vitest";
import { isSessionLogRelPath, sessionArtifactKey, sessionArtifactRel } from "../src/session-artifacts.js";

describe("isSessionLogRelPath", () => {
  it("matches the diagnostic dotdirs and nothing else", () => {
    expect(isSessionLogRelPath(".runtime-logs/stderr.log")).toBe(true);
    expect(isSessionLogRelPath(".host-logs/runtime.log")).toBe(true);
    expect(isSessionLogRelPath(".provider-proxy/access.jsonl")).toBe(true);
    expect(isSessionLogRelPath("runtime/stderr.log")).toBe(true);
    expect(isSessionLogRelPath("host/runtime.log")).toBe(true);
    expect(isSessionLogRelPath(".anthropic-debug/files-list.json")).toBe(true);
    expect(isSessionLogRelPath("report.txt")).toBe(false);
    expect(isSessionLogRelPath("subdir/report.txt")).toBe(false);
    // A deliverable that merely mentions the word is not a diagnostic.
    expect(isSessionLogRelPath("notes/runtime-logs.txt")).toBe(false);
  });
});

describe("sessionArtifactRel (stored namespace-relative path)", () => {
  it("canonicalizes diagnostics and leaves deliverables intact", () => {
    expect(sessionArtifactRel(".runtime-logs/stderr.log")).toBe("runtime/stderr.log");
    expect(sessionArtifactRel(".host-logs/runtime.log")).toBe("host/runtime.log");
    expect(sessionArtifactRel(".provider-proxy/access.jsonl")).toBe("provider-proxy/access.jsonl");
    expect(sessionArtifactRel("runtime/stderr.log")).toBe("runtime/stderr.log");
    expect(sessionArtifactRel("host/runtime.log")).toBe("host/runtime.log");
    expect(sessionArtifactRel(".anthropic-debug/files-list.json")).toBe("provider-proxy/files-list.json");
    expect(sessionArtifactRel("report.txt")).toBe("report.txt");
    expect(sessionArtifactRel("nested/data.csv")).toBe("nested/data.csv");
  });
});

describe("sessionArtifactKey", () => {
  it("routes diagnostics into internal canonical log paths and deliverables into outputs/", () => {
    expect(sessionArtifactKey("session-1", ".runtime-logs/stderr.log")).toBe("sessions/session-1/internal/logs/runtime/stderr.log");
    expect(sessionArtifactKey("session-1", ".host-logs/runtime.log")).toBe("sessions/session-1/internal/logs/host/runtime.log");
    expect(sessionArtifactKey("session-1", ".anthropic-debug/files-list.json")).toBe(
      "sessions/session-1/internal/logs/provider-proxy/files-list.json"
    );
    expect(sessionArtifactKey("session-1", "report.txt")).toBe("sessions/session-1/outputs/report.txt");
    expect(sessionArtifactKey("session-1", "sub/report.txt")).toBe("sessions/session-1/outputs/sub/report.txt");
  });
});
