/**
 * `run-artifacts` is the single source of truth for routing a run's artifacts
 * into the public `outputs` prefix or internal `internal/logs` prefix.
 * Diagnostics are stored under canonical log namespaces.
 */
import { describe, expect, it } from "vitest";
import { isRunLogRelPath, runArtifactKey, runArtifactRel } from "../src/run-artifacts.js";

describe("isRunLogRelPath", () => {
  it("matches the diagnostic dotdirs and nothing else", () => {
    expect(isRunLogRelPath(".runtime-logs/stderr.log")).toBe(true);
    expect(isRunLogRelPath(".host-logs/runtime.log")).toBe(true);
    expect(isRunLogRelPath(".provider-proxy/access.jsonl")).toBe(true);
    expect(isRunLogRelPath("runtime/stderr.log")).toBe(true);
    expect(isRunLogRelPath("host/runtime.log")).toBe(true);
    expect(isRunLogRelPath(".anthropic-debug/files-list.json")).toBe(true);
    expect(isRunLogRelPath("report.txt")).toBe(false);
    expect(isRunLogRelPath("subdir/report.txt")).toBe(false);
    // A deliverable that merely mentions the word is not a diagnostic.
    expect(isRunLogRelPath("notes/runtime-logs.txt")).toBe(false);
  });
});

describe("runArtifactRel (stored namespace-relative path)", () => {
  it("canonicalizes diagnostics and leaves deliverables intact", () => {
    expect(runArtifactRel(".runtime-logs/stderr.log")).toBe("runtime/stderr.log");
    expect(runArtifactRel(".host-logs/runtime.log")).toBe("host/runtime.log");
    expect(runArtifactRel(".provider-proxy/access.jsonl")).toBe("provider-proxy/access.jsonl");
    expect(runArtifactRel("runtime/stderr.log")).toBe("runtime/stderr.log");
    expect(runArtifactRel("host/runtime.log")).toBe("host/runtime.log");
    expect(runArtifactRel(".anthropic-debug/files-list.json")).toBe("provider-proxy/files-list.json");
    expect(runArtifactRel("report.txt")).toBe("report.txt");
    expect(runArtifactRel("nested/data.csv")).toBe("nested/data.csv");
  });
});

describe("runArtifactKey", () => {
  it("routes diagnostics into internal canonical log paths and deliverables into outputs/", () => {
    expect(runArtifactKey("run-1", ".runtime-logs/stderr.log")).toBe("runs/run-1/internal/logs/runtime/stderr.log");
    expect(runArtifactKey("run-1", ".host-logs/runtime.log")).toBe("runs/run-1/internal/logs/host/runtime.log");
    expect(runArtifactKey("run-1", ".anthropic-debug/files-list.json")).toBe(
      "runs/run-1/internal/logs/provider-proxy/files-list.json"
    );
    expect(runArtifactKey("run-1", "report.txt")).toBe("runs/run-1/outputs/report.txt");
    expect(runArtifactKey("run-1", "sub/report.txt")).toBe("runs/run-1/outputs/sub/report.txt");
  });
});
