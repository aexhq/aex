import { describe, expect, expectTypeOf, it } from "bun:test";
import type {
  SessionCheckpointRevision,
  SessionRunErrorWebhookPayload,
  SessionRunFinishedWebhookPayload,
  SessionRunWebhookEventType,
  SessionRunWebhookPayload,
  SessionWebhookDelivery
} from "../src/index.js";

const checkpoint: SessionCheckpointRevision = {
  checkpointId: "checkpoint_1",
  runId: "run_1",
  turnSeq: 1,
  committedAt: "2026-07-11T12:00:00.000Z",
  throughSeq: 4097
};

const finished: SessionRunFinishedWebhookPayload = {
  specversion: "1.0",
  id: "whd_run_1",
  source: "aex",
  type: "run.finished",
  subject: "run_1",
  time: "2026-07-11T12:00:00.000Z",
  data: {
    sessionId: "session_1",
    runId: "run_1",
    turnSeq: 1,
    outcome: "succeeded",
    terminalAt: "2026-07-11T12:00:00.000Z",
    checkpoint,
    reason: null,
    failureClass: null,
    costTelemetry: { billedCostUsd: 0.12 }
  }
};

const errored: SessionRunErrorWebhookPayload = {
  specversion: "1.0",
  id: "whd_run_2",
  source: "aex",
  type: "run.error",
  subject: "run_2",
  time: "2026-07-11T12:01:00.000Z",
  data: {
    sessionId: "session_1",
    runId: "run_2",
    turnSeq: 2,
    outcome: "failed",
    terminalAt: "2026-07-11T12:01:00.000Z",
    reason: "provider rejected the request",
    failureClass: "provider-permanent"
  }
};

describe("run webhook contract", () => {
  it("uses only run-terminal event types and run-scoped delivery identity", () => {
    expectTypeOf<SessionRunWebhookEventType>().toEqualTypeOf<"run.finished" | "run.error">();
    expectTypeOf<SessionWebhookDelivery["eventType"]>().toEqualTypeOf<SessionRunWebhookEventType>();

    for (const payload of [finished, errored] satisfies readonly SessionRunWebhookPayload[]) {
      expect(payload.subject).toBe(payload.data.runId);
      expect(payload.id).toBe(`whd_${payload.data.runId}`);
      expect(payload.type).toMatch(/^run\.(finished|error)$/);
      expect(JSON.stringify(payload)).not.toContain("session.finished");
    }
  });

  it("carries the canonical checkpoint on a finalized success and permits no checkpoint on error", () => {
    expect(finished.data.checkpoint).toEqual(checkpoint);
    expect(errored.data).not.toHaveProperty("checkpoint");
    expect(finished.data).toMatchObject({ sessionId: "session_1", runId: "run_1", turnSeq: 1, outcome: "succeeded" });
    expect(errored.data).toMatchObject({ sessionId: "session_1", runId: "run_2", turnSeq: 2, outcome: "failed" });
  });

  it("requires ledger rows to identify the owning run", () => {
    const row: SessionWebhookDelivery = {
      id: "whd_run_1",
      runId: "run_1",
      turnSeq: 1,
      eventType: "run.finished",
      status: "delivered",
      attemptCount: 1,
      lastStatusCode: 200,
      createdAt: "2026-07-11T12:00:00.000Z"
    };

    expect(row).toMatchObject({ runId: finished.data.runId, turnSeq: finished.data.turnSeq, eventType: finished.type });
  });
});
