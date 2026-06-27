import { describe, expect, it } from "vitest";
import { runCli } from "../src/run.js";
import type { CliIO } from "../src/internal.js";
import {
  assembleBundle,
  extractChildRunIds,
  parseNdjson,
  renderIndexMd,
  sortJournal,
  writeDebugBundle,
  type DebugFetchers,
  type DebugTargets,
  type LogFetchResult,
  type RunBundle,
  type S3ListItem,
  type JournalEvent
} from "../src/host/debug.js";

const TARGETS: DebugTargets = {
  plane: "dev",
  region: "eu-west-2",
  partition: "aws",
  account: "123456789012",
  outputsBucket: "aex-dev-eu-west-2-outputs-123456789012",
  eventsArchiveBucket: "aex-dev-eu-west-2-events-archive-123456789012",
  eventsTable: "aex-dev-eu-west-2-events",
  stateMachineName: "aex-dev-eu-west-2-sfn",
  logGroups: {
    api: "/aws/lambda/aex-dev-eu-west-2-api",
    sfn: "/aws/states/aex-dev-eu-west-2-run",
    brain: "/aws/ecs/aex-dev-eu-west-2-brain",
    egress: "/aws/ecs/aex-dev-eu-west-2-egress"
  }
};

/**
 * A fully in-memory fake of the AWS read surfaces. Keyed by `${bucket}|${key}`
 * for objects, and by runId for the journal / SFN / CloudWatch surfaces.
 */
function makeFakeSources(spec: {
  objects?: Record<string, Record<string, string>>; // bucket -> key -> body
  ddb?: Record<string, readonly JournalEvent[] | null>; // runId -> rows (null = not found)
  sfn?: Record<string, readonly unknown[] | null>; // runId -> history
  logs?: Record<string, LogFetchResult>; // `${label}` -> result
}): { sources: DebugFetchers; calls: { getObject: string[] } } {
  const objects = spec.objects ?? {};
  const calls = { getObject: [] as string[] };
  const groupLabel = (group: string): string => {
    for (const [label, g] of Object.entries(TARGETS.logGroups)) if (g === group) return label;
    return group;
  };
  const sources: DebugFetchers = {
    async listObjects(bucket, prefix): Promise<readonly S3ListItem[]> {
      const bucketObjs = objects[bucket] ?? {};
      return Object.entries(bucketObjs)
        .filter(([key]) => key.startsWith(prefix))
        .map(([key, body]) => ({ key, sizeBytes: body.length }));
    },
    async getObjectText(bucket, key): Promise<string | null> {
      calls.getObject.push(`${bucket}|${key}`);
      return objects[bucket]?.[key] ?? null;
    },
    async queryEvents(_table, runId): Promise<readonly JournalEvent[] | null> {
      return spec.ddb?.[runId] ?? null;
    },
    async getExecutionHistory(arn): Promise<readonly unknown[] | null> {
      // arn tail is the runId (execution name).
      const runId = arn.split(":").pop()!;
      return spec.sfn?.[runId] ?? null;
    },
    async filterLogs(group): Promise<LogFetchResult> {
      return spec.logs?.[groupLabel(group)] ?? { kind: "empty" };
    }
  };
  return { sources, calls };
}

function fileMap(bundle: RunBundle): Map<string, string> {
  return new Map(bundle.files.map((f) => [f.path, f.content]));
}

describe("debug pure helpers", () => {
  it("sortJournal orders by numeric seq and is stable for ties/seqless", () => {
    const sorted = sortJournal([
      { seq: 3, t: "c" },
      { seq: 1, t: "a" },
      { t: "noseq" },
      { seq: "2", t: "b" }
    ]);
    expect(sorted.map((e) => e.t)).toEqual(["a", "b", "c", "noseq"]);
  });

  it("parseNdjson skips blank and malformed lines", () => {
    const parsed = parseNdjson('{"seq":1}\n\nnot json\n{"seq":2}\n');
    expect(parsed).toEqual([{ seq: 1 }, { seq: 2 }]);
  });

  it("extractChildRunIds finds lineage keys recursively and excludes self", () => {
    const ids = extractChildRunIds(
      [
        { type: "subagent_spawned", data: { childRunId: "run-child-1" } },
        { type: "x", spawnedRunId: "run-child-2" },
        { type: "self-ref", childRunId: "run-self" }
      ],
      "run-self"
    );
    expect(ids.sort()).toEqual(["run-child-1", "run-child-2"]);
  });

  it("renderIndexMd includes status, journal source and child references", () => {
    const md = renderIndexMd({
      runId: "run-x",
      generatedAt: "2026-06-27T00:00:00.000Z",
      plane: "dev",
      region: "eu-west-2",
      outputsBucket: "b",
      eventsArchiveBucket: "a",
      runStatus: "succeeded",
      startedAt: "s",
      endedAt: "e",
      journalSource: "ddb",
      journalEventCount: 2,
      childRunIds: ["run-c"],
      sources: [{ source: "s3", state: "present", count: 5 }],
      children: []
    });
    expect(md).toContain("aex debug bundle — run-x");
    expect(md).toContain("run status: succeeded");
    expect(md).toContain("journal: ddb (2 events)");
    expect(md).toContain("children/run-c/");
  });
});

describe("assembleBundle", () => {
  const baseObjects = {
    [TARGETS.outputsBucket]: {
      "runs/run-parent/session/boot.json": JSON.stringify({ createdAt: "2026-06-26T23:00:00.000Z" }),
      "runs/run-parent/settle.json": JSON.stringify({
        status: "succeeded",
        startedAt: "2026-06-26T23:00:00.000Z",
        endedAt: "2026-06-27T00:00:00.000Z"
      }),
      "runs/run-parent/session/diag-egress.json": JSON.stringify({ ok: true }),
      "runs/run-parent/session/brain-abc.ndjson": '{"ts":1,"msg":"hi"}\n',
      "runs/run-parent/secrets.sealed": "SEALED-BLOB-DO-NOT-READ",
      "runs/run-parent/outputs/report.md": "hello-deliverable"
    }
  };

  it("assembles S3 artifacts, journal from DDB, SFN history; lists (not downloads) outputs/sealed", async () => {
    const { sources, calls } = makeFakeSources({
      objects: baseObjects,
      ddb: {
        "run-parent": [
          { seq: 2, type: "step" },
          { seq: 1, type: "runtime_start" }
        ]
      },
      sfn: { "run-parent": [{ type: "ExecutionStarted" }] }
    });

    const bundle = await assembleBundle({
      runId: "run-parent",
      targets: TARGETS,
      sources,
      cloudwatch: false,
      withOutputs: false,
      sinceMs: 86_400_000
    });
    const files = fileMap(bundle);

    // S3 text artifacts captured at their relative paths.
    expect(files.has("session/boot.json")).toBe(true);
    expect(files.has("settle.json")).toBe(true);
    expect(files.has("session/diag-egress.json")).toBe(true);
    expect(files.has("session/brain-abc.ndjson")).toBe(true);

    // Deliverable body NOT downloaded (no --with-outputs); only listed.
    expect(files.has("outputs/report.md")).toBe(false);
    expect(files.get("outputs/_listing.ndjson")).toContain("runs/run-parent/outputs/report.md");
    expect(calls.getObject).not.toContain(`${TARGETS.outputsBucket}|runs/run-parent/outputs/report.md`);

    // Sealed blob is noted but never fetched.
    expect(files.has("secrets.sealed")).toBe(false);
    expect(calls.getObject).not.toContain(`${TARGETS.outputsBucket}|runs/run-parent/secrets.sealed`);
    expect(bundle.manifest.sources.find((s) => s.source === "secrets.sealed")?.state).toBe("present");

    // Journal from DDB, sorted by seq.
    expect(bundle.manifest.journalSource).toBe("ddb");
    expect(bundle.manifest.journalEventCount).toBe(2);
    const journalLines = files.get("journal.ndjson")!.trim().split("\n");
    expect(JSON.parse(journalLines[0]!).seq).toBe(1);
    expect(JSON.parse(journalLines[1]!).seq).toBe(2);

    // SFN history present.
    expect(files.get("sfn-history.json")).toContain("ExecutionStarted");
    expect(bundle.manifest.sources.find((s) => s.source === "sfn")?.state).toBe("present");

    // Run meta lifted from settle.json.
    expect(bundle.manifest.runStatus).toBe("succeeded");
    expect(bundle.manifest.endedAt).toBe("2026-06-27T00:00:00.000Z");

    // index.md + manifest.json at root.
    expect(files.has("index.md")).toBe(true);
    expect(files.has("manifest.json")).toBe(true);

    // CloudWatch skipped (not requested).
    expect(bundle.manifest.sources.find((s) => s.source === "cloudwatch")?.state).toBe("skipped");
  });

  it("downloads deliverable bodies with withOutputs", async () => {
    const { sources } = makeFakeSources({ objects: baseObjects, ddb: { "run-parent": [{ seq: 1 }] } });
    const bundle = await assembleBundle({
      runId: "run-parent",
      targets: TARGETS,
      sources,
      cloudwatch: false,
      withOutputs: true,
      sinceMs: 86_400_000
    });
    expect(fileMap(bundle).get("outputs/report.md")).toBe("hello-deliverable");
  });

  it("falls back to events-archive when DDB is empty/expired", async () => {
    const { sources } = makeFakeSources({
      objects: {
        [TARGETS.outputsBucket]: { "runs/run-old/settle.json": JSON.stringify({ status: "failed" }) },
        [TARGETS.eventsArchiveBucket]: {
          "run-old/log.ndjson": '{"seq":1,"type":"a"}\n{"seq":2,"type":"b"}\n'
        }
      },
      ddb: { "run-old": null } // DDB rows gone (TTL)
    });
    const bundle = await assembleBundle({
      runId: "run-old",
      targets: TARGETS,
      sources,
      cloudwatch: false,
      withOutputs: false,
      sinceMs: 86_400_000
    });
    expect(bundle.manifest.journalSource).toBe("events-archive");
    expect(bundle.manifest.journalEventCount).toBe(2);
    expect(bundle.manifest.runStatus).toBe("failed");
  });

  it("notes a fully-missing run as gaps rather than throwing", async () => {
    const { sources } = makeFakeSources({ objects: {}, ddb: {} });
    const bundle = await assembleBundle({
      runId: "run-ghost",
      targets: TARGETS,
      sources,
      cloudwatch: false,
      withOutputs: false,
      sinceMs: 86_400_000
    });
    const bySource = new Map(bundle.manifest.sources.map((s) => [s.source, s.state]));
    expect(bySource.get("s3")).toBe("missing");
    expect(bySource.get("journal")).toBe("missing");
    expect(bySource.get("sfn")).toBe("missing");
    expect(bundle.manifest.runStatus).toBeNull();
    // Bundle still produced with index/manifest.
    expect(fileMap(bundle).has("index.md")).toBe(true);
  });

  it("recurses into child runs and prevents loops via the visited set", async () => {
    const { sources } = makeFakeSources({
      objects: {
        [TARGETS.outputsBucket]: {
          "runs/run-parent/settle.json": JSON.stringify({ status: "succeeded" }),
          "runs/run-child/settle.json": JSON.stringify({ status: "succeeded" })
        },
        [TARGETS.eventsArchiveBucket]: {
          // child journal references the parent — must NOT recurse back.
          "run-child/log.ndjson": '{"seq":1,"type":"subagent_done","childRunId":"run-parent"}\n'
        }
      },
      ddb: {
        "run-parent": [{ seq: 1, type: "subagent_spawned", childRunId: "run-child" }],
        "run-child": null // forces child to use the archive fallback
      }
    });
    const bundle = await assembleBundle({
      runId: "run-parent",
      targets: TARGETS,
      sources,
      cloudwatch: false,
      withOutputs: false,
      sinceMs: 86_400_000
    });
    expect(bundle.manifest.childRunIds).toEqual(["run-child"]);
    const files = fileMap(bundle);
    // Child bundle nested under children/<childId>/ and self-contained.
    expect(files.has("children/run-child/settle.json")).toBe(true);
    expect(files.has("children/run-child/manifest.json")).toBe(true);
    expect(files.has("children/run-child/index.md")).toBe(true);
    // Child resolved its journal from the archive.
    expect(bundle.manifest.children[0]!.journalSource).toBe("events-archive");
    // No grandchild bundle for run-parent (loop prevented).
    expect(files.has("children/run-child/children/run-parent/manifest.json")).toBe(false);
  });

  it("fetches CloudWatch with --cloudwatch and notes expired groups", async () => {
    const { sources } = makeFakeSources({
      objects: { [TARGETS.outputsBucket]: { "runs/run-cw/settle.json": JSON.stringify({ status: "succeeded" }) } },
      ddb: { "run-cw": [{ seq: 1 }] },
      logs: {
        api: { kind: "events", events: [{ message: "run-cw started" }] },
        brain: { kind: "expired" }
      }
    });
    const bundle = await assembleBundle({
      runId: "run-cw",
      targets: TARGETS,
      sources,
      cloudwatch: true,
      withOutputs: false,
      sinceMs: 86_400_000
    });
    const files = fileMap(bundle);
    expect(files.get("cloudwatch/api.ndjson")).toContain("run-cw started");
    const bySource = new Map(bundle.manifest.sources.map((s) => [s.source, s.state]));
    expect(bySource.get("cloudwatch:api")).toBe("present");
    expect(bySource.get("cloudwatch:brain")).toBe("expired");
  });
});

describe("writeDebugBundle", () => {
  it("mkdir -p's parents and writes every file under the base dir", async () => {
    const writes = new Map<string, Uint8Array>();
    const dirs = new Set<string>();
    const io: Partial<CliIO> = {
      writeFile: async (path, data) => {
        writes.set(path, data);
      },
      mkdirp: async (path) => {
        dirs.add(path);
      }
    };
    const bundle: RunBundle = {
      files: [
        { path: "index.md", content: "root" },
        { path: "children/run-c/manifest.json", content: "{}" }
      ],
      manifest: {} as RunBundle["manifest"]
    };
    await writeDebugBundle(io as CliIO, "/base", bundle);
    expect(writes.size).toBe(2);
    // A nested parent dir was created.
    expect([...dirs].some((d) => d.replace(/\\/g, "/").endsWith("children/run-c"))).toBe(true);
    const indexKey = [...writes.keys()].find((k) => k.endsWith("index.md"))!;
    expect(new TextDecoder().decode(writes.get(indexKey)!)).toBe("root");
  });
});

describe("aex debug flag handling (no AWS)", () => {
  function makeIo(argv: readonly string[]): { io: CliIO; stderr: () => string; exitCode: () => number | null } {
    let stderr = "";
    let exitCode: number | null = null;
    const io: CliIO = {
      argv: ["bun", "/aex/aex", ...argv],
      readFile: async (path) => {
        throw Object.assign(new Error(`ENOENT: ${path}`), { code: "ENOENT" });
      },
      writeFile: async () => undefined,
      fetchImpl: (async () => new Response("{}")) as typeof fetch,
      cwd: () => "/tmp/cli-test",
      stdout: () => undefined,
      stderr: (chunk) => {
        stderr += chunk;
      },
      exit: (code) => {
        exitCode = code;
      }
    };
    return { io, stderr: () => stderr, exitCode: () => exitCode };
  }

  it("rejects a missing run-id before touching AWS", async () => {
    const cap = makeIo(["debug"]);
    await runCli(cap.io);
    expect(cap.exitCode()).toBe(2);
    expect(cap.stderr()).toContain("usage: aex debug");
  });

  it("rejects an invalid --plane", async () => {
    const cap = makeIo(["debug", "run-1", "--plane", "staging"]);
    await runCli(cap.io);
    expect(cap.exitCode()).toBe(2);
    expect(cap.stderr()).toContain("--plane must be one of");
  });

  it("rejects an unknown flag", async () => {
    const cap = makeIo(["debug", "run-1", "--bogus"]);
    await runCli(cap.io);
    expect(cap.exitCode()).toBe(2);
    expect(cap.stderr()).toContain("unknown flag");
  });
});
