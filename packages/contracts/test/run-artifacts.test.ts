/**
 * `run-artifacts` is the single source of truth for routing a run's artifacts
 * into the `outputs` vs `logs` namespace. Diagnostics are stored under
 * canonical log namespaces; legacy prefixes normalize on read/write.
 */
import { describe, expect, it } from "vitest";
import { isRunLogRelPath, runArtifactKey, runArtifactRel } from "../src/run-artifacts.js";

describe("isRunLogRelPath", () => {
  it("matches the diagnostic dotdirs and nothing else", () => {
    expect(isRunLogRelPath(".runtime-logs/stderr.log")).toBe(true);
    expect(isRunLogRelPath(".host-logs/runtime.log")).toBe(true);
    expect(isRunLogRelPath(".provider-proxy/access.jsonl")).toBe(true);
    expect(isRunLogRelPath(".goose-logs/stderr.log")).toBe(true);
    expect(isRunLogRelPath(".fly-logs/runtime.log")).toBe(true);
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
    expect(runArtifactRel(".goose-logs/stderr.log")).toBe("runtime/stderr.log");
    expect(runArtifactRel(".fly-logs/runtime.log")).toBe("host/runtime.log");
    expect(runArtifactRel(".anthropic-debug/files-list.json")).toBe("provider-proxy/files-list.json");
    expect(runArtifactRel("report.txt")).toBe("report.txt");
    expect(runArtifactRel("nested/data.csv")).toBe("nested/data.csv");
  });
});

describe("runArtifactKey", () => {
  it("routes diagnostics into canonical logs/ paths and deliverables into outputs/", () => {
    expect(runArtifactKey("run-1", ".runtime-logs/stderr.log")).toBe("runs/run-1/logs/runtime/stderr.log");
    expect(runArtifactKey("run-1", ".host-logs/runtime.log")).toBe("runs/run-1/logs/host/runtime.log");
    expect(runArtifactKey("run-1", ".anthropic-debug/files-list.json")).toBe(
      "runs/run-1/logs/provider-proxy/files-list.json"
    );
    expect(runArtifactKey("run-1", "report.txt")).toBe("runs/run-1/outputs/report.txt");
    expect(runArtifactKey("run-1", "sub/report.txt")).toBe("runs/run-1/outputs/sub/report.txt");
  });
});
