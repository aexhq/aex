import { describe, expect, it } from "bun:test";
import { idPattern, newId, type FetchLike } from "@aexhq/contracts";
import {
  Aex,
  TelemetryStreamBackpressureError,
  TelemetryStreamRotationError,
  type ObservationCoverage,
  type ObservationFrame
} from "../../src/index.js";

const BASE_URL = "https://eu-west-1.api.aex.test";
const SID = newId("session");
const WID = newId("workspace");
const OID = newId("observation");
const OPID = newId("operation");
const EID = newId("export");
const GAPID = newId("telemetryGap");
const BID = newId("telemetryBatch");
const RID = newId("run");
const at = "2026-07-30T10:00:00.000Z";
const hash = `sha256:${"a".repeat(64)}`;

type Call = {
  readonly url: URL;
  readonly method: string;
  readonly headers: Headers;
  readonly body: unknown;
};

function json(value: unknown, status = 200, headers: HeadersInit = {}): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json", ...headers }
  });
}

function sessionRecord(): Record<string, unknown> {
  return {
    id: SID,
    workspaceId: WID,
    status: "idle",
    revision: 1,
    persistRevision: 0,
    createdAt: at,
    updatedAt: at,
    continuity: {
      state: "cold",
      persistedRevision: 0,
      changedAt: at,
      reason: "not_started"
    },
    lineage: {},
    resolvedConfig: {}
  };
}

const coverage: ObservationCoverage = {
  snapshot: { cursor: "cur_snapshot", time: at },
  accepted: { cursor: "cur_accepted", time: at },
  indexed: { cursor: "cur_indexed", time: at },
  earliestReplay: { cursor: "cur_earliest", time: at },
  caughtUp: true,
  complete: true,
  missingIntervals: [],
  unboundedGaps: 0
};

const log = {
  signal: "logs",
  id: OID,
  revision: 1,
  workspaceId: WID,
  sessionId: SID,
  time: at,
  acceptedAt: at,
  severityNumber: 9,
  severityText: "INFO",
  body: "ready"
} as const;

function recordingClient(
  respond: (call: Call) => Response | Promise<Response>
): { readonly client: Aex; readonly calls: Call[] } {
  const calls: Call[] = [];
  const fetch: FetchLike = async (input, init) => {
    const call: Call = {
      url: new URL(input instanceof URL ? input.href : String(input)),
      method: init?.method ?? "GET",
      headers: new Headers(init?.headers),
      body: typeof init?.body === "string" &&
          new Headers(init.headers).get("content-type") === "application/json"
        ? JSON.parse(init.body)
        : init?.body
    };
    calls.push(call);
    return respond(call);
  };
  return {
    client: new Aex({ apiKey: "test-key", baseUrl: BASE_URL, fetch, retry: false }),
    calls
  };
}

describe("v1 observation query, iterate, stream, and listen", () => {
  it("queries workspace/session signals without mutation identity headers", async () => {
    const { client, calls } = recordingClient((call) =>
      call.url.pathname === `/api/sessions/${SID}`
        ? json(sessionRecord())
        : json({ items: [log], coverage }));

    await client.logs.query({ where: { op: "contains", field: "body", value: "ready" } });
    const session = await client.sessions.open(SID);
    await session.events.query({ limit: 1 });

    const queryCalls = calls.filter(({ url }) => url.pathname.endsWith("/query"));
    expect(queryCalls.map(({ url }) => url.pathname)).toEqual([
      "/api/logs/query",
      `/api/sessions/${SID}/events/query`
    ]);
    for (const call of queryCalls) {
      expect(call.headers.get("idempotency-key")).toBeNull();
      expect(call.headers.get("aex-operation-id")).toBeNull();
    }
  });

  it("iterates cursor pages and exposes the final pinned coverage", async () => {
    let page = 0;
    const { client, calls } = recordingClient(() => {
      page += 1;
      return json(page === 1
        ? { items: [log], nextCursor: "cur_next", coverage }
        : { items: [{ ...log, id: newId("observation") }], coverage });
    });
    const iterator = client.logs.iterate({ limit: 1 });
    const rows = [];
    for await (const row of iterator) rows.push(row);

    expect(rows).toHaveLength(2);
    await expect(iterator.coverage).resolves.toEqual(coverage);
    expect(calls[1]!.body).toMatchObject({ cursor: "cur_next" });
  });

  it("parses NDJSON frames without WebSocket/tickets and preserves cursor rotation", async () => {
    const frames: ObservationFrame[] = [
      { type: "records", records: [log], cursor: "cur_1" },
      { type: "cursor", cursor: "cur_1" },
      { type: "rotate", cursor: "cur_1" }
    ];
    const { client, calls } = recordingClient(() =>
      new Response(`${frames.map((frame) => JSON.stringify(frame)).join("\n")}\n`, {
        headers: { "content-type": "application/x-ndjson" }
      }));
    const received = [];
    for await (const frame of client.logs.stream({ origin: { earliest: true } })) {
      received.push(frame);
    }

    expect(received).toEqual(frames);
    expect(calls[0]!.url.pathname).toBe("/api/logs/stream");
    expect(calls[0]!.body).toEqual({ origin: { earliest: true } });
    expect(calls[0]!.headers.get("idempotency-key")).toBeNull();
  });

  it("surfaces stream backpressure and unframed rotation with the resume cursor", async () => {
    for (const [close, ErrorType] of [
      ["backpressure", TelemetryStreamBackpressureError],
      ["rotation", TelemetryStreamRotationError]
    ] as const) {
      const { client } = recordingClient(() =>
        new Response("", {
          headers: {
            "content-type": "application/x-ndjson",
            "Aex-Stream-Close": close,
            "Aex-Stream-Cursor": "cur_resume"
          }
        }));
      const read = async () => {
        for await (const _frame of client.logs.listen()) {
          // The close header is handled before any frame can be yielded.
        }
      };
      const error = await read().catch((caught: unknown) => caught);
      expect(error).toBeInstanceOf(ErrorType);
      expect((error as { readonly cursor?: string }).cursor).toBe("cur_resume");
    }
  });
});

describe("v1 metrics, traces, gaps, OTLP, and exports", () => {
  it("routes metric aggregation and W3C trace revision reads", async () => {
    const traceId = "0123456789abcdef0123456789abcdef";
    const { client, calls } = recordingClient((call) => {
      if (call.url.pathname === `/api/sessions/${SID}`) return json(sessionRecord());
      if (call.url.pathname.endsWith(`/traces/${traceId}`)) {
        return json({
          signal: "traces",
          id: OID,
          revision: 2,
          workspaceId: WID,
          sessionId: SID,
          traceId,
          state: "quiescent",
          rootName: "request",
          durationNs: 10,
          spanCount: 1,
          errorCount: 0,
          serviceName: "api",
          time: at,
          acceptedAt: at
        });
      }
      return json({ items: [], coverage });
    });
    await client.metrics.aggregate({
      name: "requests",
      timeRange: { gte: at, lt: "2026-07-30T11:00:00.000Z" },
      interval: "60s",
      calculations: [{ op: "sum" }]
    });
    const session = await client.sessions.open(SID);
    await session.traces.get(traceId);

    expect(calls.some(({ url }) => url.pathname === "/api/metrics/aggregate")).toBe(true);
    expect(calls.some(({ url }) =>
      url.pathname === `/api/sessions/${SID}/traces/${traceId}`)).toBe(true);
  });

  it("queries/gets gaps and admits atomic OTLP with bch identity plus receipt headers", async () => {
    const gap = {
      id: GAPID,
      revision: 1,
      workspaceId: WID,
      sessionId: SID,
      signals: ["logs"],
      status: "open",
      reason: "pipeline_loss",
      recoverable: false,
      attemptedRecords: 1,
      attemptedBytes: 10,
      createdAt: at,
      updatedAt: at
    };
    const { client, calls } = recordingClient((call) => {
      if (call.url.pathname.endsWith("/otlp/v1/logs")) {
        return json({}, 200, {
          "Aex-Telemetry-Batch-Id": BID,
          "Aex-Telemetry-Receipt-Id": "receipt_1"
        });
      }
      if (call.method === "GET") return json(gap);
      return json({ items: [gap], coverage });
    });

    await client.telemetry.gaps.query({ signals: ["logs"] });
    await client.telemetry.gaps.get(GAPID);
    const receipt = await client.telemetry.otlp.logs(
      { resourceLogs: [] },
      { batchId: BID }
    );

    expect(receipt.batchId).toBe(BID);
    expect(receipt.receiptId).toBe("receipt_1");
    const otlp = calls.at(-1)!;
    expect(otlp.headers.get("idempotency-key")).toBe(BID);
    expect(otlp.headers.get("idempotency-key")).toMatch(idPattern("telemetryBatch"));
  });

  it("creates exports with only operation identity and uses idempotency for grants/revocation", async () => {
    const exportRecord = {
      id: EID,
      status: "ready",
      format: "ndjson",
      manifestHash: hash,
      createdAt: at,
      updatedAt: at,
      expiresAt: "2026-07-31T10:00:00.000Z"
    };
    const { client, calls } = recordingClient((call) => {
      if (call.url.pathname === "/api/telemetry/exports" && call.method === "POST") {
        const operationId = call.headers.get("aex-operation-id")!;
        return json({
          id: operationId,
          workspaceId: WID,
          kind: "telemetry_export",
          status: "queued",
          cancelable: true,
          createdAt: at,
          updatedAt: at
        }, 202);
      }
      if (call.url.pathname.endsWith("/downloads")) {
        return json({
          url: "https://download.example.test/export",
          expiresAt: "2026-07-30T10:05:00.000Z",
          sizeBytes: 12,
          authorizedBytes: 12,
          measurementId: newId("measurement"),
          sha256: hash
        });
      }
      if (call.url.pathname.endsWith("/revocations")) {
        return json({ ...exportRecord, status: "revoked", revokedAt: at });
      }
      return json(exportRecord);
    });

    const operation = await client.telemetry.export({
      query: { signals: ["logs"] },
      format: "ndjson",
      completeness: "allow_gaps"
    });
    await client.telemetry.exports.get(EID);
    await client.telemetry.exports.download(EID);
    await client.telemetry.exports.revoke(EID);

    expect(operation.id).toMatch(idPattern("operation"));
    expect(calls[0]!.headers.get("aex-operation-id")).toBe(operation.id);
    expect(calls[0]!.headers.get("idempotency-key")).toBeNull();
    expect(calls[2]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls[3]!.headers.get("idempotency-key")).toBeTruthy();
  });

  it("does not expose numeric cursor, follow, ticket, archive, or OTEL projection APIs", async () => {
    const root = await import("../../src/index.js") as Record<string, unknown>;
    for (const name of ["SessionEvents", "SessionOtel", "SessionRunStream"]) {
      expect(root[name]).toBeUndefined();
    }
    const { client } = recordingClient(() => json({ items: [], coverage }));
    for (const signal of [
      client.events,
      client.logs,
      client.spans,
      client.metrics,
      client.traces,
      client.telemetry
    ]) {
      for (const removed of [
        "from",
        "follow",
        "ticket",
        "archiveLink",
        "poll",
        "otel"
      ]) {
        expect((signal as unknown as Record<string, unknown>)[removed]).toBeUndefined();
      }
    }
  });
});
