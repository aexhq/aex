/**
 * `run-artifacts` is the single source of truth for routing a run's R2
 * artifacts into the `outputs` vs `logs` namespace. Diagnostics arrive as
 * workspace dotdirs (`.goose-logs/...`) and are stored dot-stripped under
 * `logs/`; deliverables pass through under `outputs/`. The opaque id is
 * computed over the STORED path (`runArtifactRel`), so upload and
 * download/list must agree on the dot-strip — these tests pin that.
 */
import { describe, expect, it } from "vitest";
import { isRunLogRelPath, runArtifactKey, runArtifactRel } from "../src/run-artifacts.js";

describe("isRunLogRelPath", () => {
  it("matches the diagnostic dotdirs and nothing else", () => {
    expect(isRunLogRelPath(".goose-logs/stderr.log")).toBe(true);
    expect(isRunLogRelPath(".fly-logs/machine.log")).toBe(true);
    expect(isRunLogRelPath(".anthropic-debug/files-list.json")).toBe(true);
    expect(isRunLogRelPath("report.txt")).toBe(false);
    expect(isRunLogRelPath("subdir/report.txt")).toBe(false);
    // A deliverable that merely mentions the word is not a diagnostic.
    expect(isRunLogRelPath("notes/goose-logs.txt")).toBe(false);
  });
});

describe("runArtifactRel (stored namespace-relative path)", () => {
  it("drops the leading dot for diagnostics, leaves deliverables intact", () => {
    expect(runArtifactRel(".goose-logs/stderr.log")).toBe("goose-logs/stderr.log");
    expect(runArtifactRel(".anthropic-debug/files-list.json")).toBe("anthropic-debug/files-list.json");
    expect(runArtifactRel("report.txt")).toBe("report.txt");
    expect(runArtifactRel("nested/data.csv")).toBe("nested/data.csv");
  });
});

describe("runArtifactKey", () => {
  it("routes diagnostics into logs/ (dot-stripped) and deliverables into outputs/", () => {
    expect(runArtifactKey("run-1", ".goose-logs/stderr.log")).toBe("runs/run-1/logs/goose-logs/stderr.log");
    expect(runArtifactKey("run-1", ".fly-logs/machine.log")).toBe("runs/run-1/logs/fly-logs/machine.log");
    expect(runArtifactKey("run-1", ".anthropic-debug/files-list.json")).toBe(
      "runs/run-1/logs/anthropic-debug/files-list.json"
    );
    expect(runArtifactKey("run-1", "report.txt")).toBe("runs/run-1/outputs/report.txt");
    expect(runArtifactKey("run-1", "sub/report.txt")).toBe("runs/run-1/outputs/sub/report.txt");
  });
});
