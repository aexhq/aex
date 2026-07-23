/**
 * `session-artifacts` is the single source of truth for routing a session's artifacts
 * into checkpoint file objects or internal log prefixes.
 * Diagnostics are stored under canonical log namespaces.
 */
import { describe, expect, it } from "bun:test";
import {
  isSessionLogRelPath,
  runCheckpointWorkspaceObjectKey,
  s3ObjectRetentionTaggingHeader,
  s3ObjectRetentionTagValue,
  sessionArtifactRel,
  sessionInternalArtifactKey
} from "../src/session-artifacts.js";

describe("isSessionLogRelPath", () => {
  it("matches the diagnostic dotdirs and nothing else", () => {
    expect(isSessionLogRelPath(".runtime-logs/stderr.log")).toBe(true);
    expect(isSessionLogRelPath(".host-logs/runtime.log")).toBe(true);
    expect(isSessionLogRelPath(".provider-proxy/access.jsonl")).toBe(true);
    expect(isSessionLogRelPath(".anthropic-debug/files-list.json")).toBe(true);
    expect(isSessionLogRelPath("report.txt")).toBe(false);
    expect(isSessionLogRelPath("subdir/report.txt")).toBe(false);
    // A deliverable that merely mentions the word is not a diagnostic.
    expect(isSessionLogRelPath("notes/runtime-logs.txt")).toBe(false);
    expect(isSessionLogRelPath(".tool-a-logs/stderr.log")).toBe(false);
    expect(isSessionLogRelPath(".tool-b-logs/runtime.log")).toBe(false);
    expect(isSessionLogRelPath("home/runner/.local/share/tool-a/projects.json")).toBe(false);
  });
});

describe("sessionArtifactRel (stored namespace-relative path)", () => {
  it("canonicalizes diagnostics and leaves deliverables intact", () => {
    expect(sessionArtifactRel(".runtime-logs/stderr.log")).toBe("runtime/stderr.log");
    expect(sessionArtifactRel(".host-logs/runtime.log")).toBe("host/runtime.log");
    expect(sessionArtifactRel(".provider-proxy/access.jsonl")).toBe("provider-proxy/access.jsonl");
    expect(sessionArtifactRel(".anthropic-debug/files-list.json")).toBe("provider-proxy/files-list.json");
    expect(sessionArtifactRel(".tool-a-logs/stderr.log")).toBe(".tool-a-logs/stderr.log");
    expect(sessionArtifactRel(".tool-b-logs/runtime.log")).toBe(".tool-b-logs/runtime.log");
    expect(sessionArtifactRel("report.txt")).toBe("report.txt");
    expect(sessionArtifactRel("nested/data.csv")).toBe("nested/data.csv");
  });
});

describe("session artifact keys", () => {
  it("routes diagnostics into internal canonical log paths and files into checkpoint objects", () => {
    expect(sessionInternalArtifactKey("workspace-1", "session-1", ".runtime-logs/stderr.log")).toBe(
      "workspaces/workspace-1/sessions/session-1/internal/logs/runtime/stderr.log"
    );
    expect(sessionInternalArtifactKey("workspace-1", "session-1", ".host-logs/runtime.log")).toBe(
      "workspaces/workspace-1/sessions/session-1/internal/logs/host/runtime.log"
    );
    expect(sessionInternalArtifactKey("workspace-1", "session-1", ".anthropic-debug/files-list.json")).toBe(
      "workspaces/workspace-1/sessions/session-1/internal/logs/provider-proxy/files-list.json"
    );
    expect(runCheckpointWorkspaceObjectKey("workspace-1", "session-1", "cp-1", "file-1")).toBe(
      "workspaces/workspace-1/sessions/session-1/checkpoints/cp-1/workspace/objects/file-1"
    );
  });
});

describe("s3ObjectRetentionTagValue", () => {
  it("classifies workspace-scoped files, assets, checkpoint core, and internals", () => {
    expect(
      s3ObjectRetentionTagValue(
        "workspaces/workspace-1/sessions/session-1/checkpoints/cp-1/workspace/objects/file-1"
      )
    ).toBe("session-file-current");
    expect(s3ObjectRetentionTagValue("workspaces/workspace-1/assets/content/abc")).toBe("workspace-asset");
    expect(s3ObjectRetentionTagValue("workspaces/workspace-1/sessions/session-1/checkpoints/current.json")).toBe(
      "session-core"
    );
    expect(
      s3ObjectRetentionTagValue("workspaces/workspace-1/sessions/session-1/checkpoints/cp-1/brain/journal.jsonl")
    ).toBe("session-core");
    expect(s3ObjectRetentionTagValue("workspaces/workspace-1/sessions/session-1/internal/logs/runtime/stderr.log")).toBe(
      "internal-log"
    );
    expect(s3ObjectRetentionTagValue("workspaces/workspace-1/sessions/session-1/internal/events/archive.jsonl")).toBe(
      "internal-event"
    );
    expect(s3ObjectRetentionTagValue("workspaces/workspace-1/sessions/session-1/internal/usage/settle.json")).toBe(
      "internal-usage"
    );
    expect(
      s3ObjectRetentionTagValue("workspaces/workspace-1/sessions/session-1/internal/admin-archives/123.zip")
    ).toBe("admin-archive");
    expect(s3ObjectRetentionTagValue("workspaces/workspace-1/uploads/tmp")).toBe("upload-staging");
  });

  it("keeps legacy control keys bounded while boot/journal readers are cut over", () => {
    expect(s3ObjectRetentionTagValue("sessions/session-1/session/boot.json")).toBe("session-core");
    expect(s3ObjectRetentionTagValue("sessions/session-1/session/control.json")).toBe("session-core");
    expect(s3ObjectRetentionTagValue("sessions/session-1/session/journal/e1/000000000000-000000000000.ndjson")).toBe(
      "session-core"
    );
    expect(s3ObjectRetentionTagValue("sessions/session-1/session/logs/runtime.log")).toBe("internal-log");
    expect(s3ObjectRetentionTagValue("sessions/session-1/session/internal/usage/settle.json")).toBe("internal-usage");
    expect(s3ObjectRetentionTaggingHeader("sessions/session-1/session/control.json")).toBe(
      "aex-retention=session-core"
    );
  });
});
