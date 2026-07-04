/**
 * `operations.listRuns` — defensive filtering of phantom rows.
 *
 * Deployed planes have leaked settle-time marker items (spendmark /
 * webhook-delivery ledger rows) into the run-list index; those rows reach the
 * wire as `{ id, createdAt }` fragments with no `status`/`updatedAt` and
 * would otherwise surface through the SDK as duplicate, type-violating
 * `RunSummary` entries (one per settled run). The client drops anything
 * missing the fields `RunSummary` declares required.
 */
import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { operations } from "../src/index.js";

const BASE = "https://api.test";

function clientFor(body: unknown, capture?: { url?: string }) {
  const fetchImpl = async (input: string | URL | Request) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    if (capture) capture.url = url;
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  };
  return new HttpClient({ apiToken: "tok", baseUrl: BASE, fetch: fetchImpl });
}

const WELL_FORMED = {
  id: "run_a1",
  status: "idle",
  createdAt: "2026-07-04T04:21:51.650Z",
  updatedAt: "2026-07-04T04:22:44.525Z",
  costUsd: 0.0004
};

describe("operations.listRuns", () => {
  it("drops phantom rows missing required RunSummary fields, keeps the cursor", async () => {
    const client = clientFor({
      runs: [
        // Phantom: settle-time marker leaked into the list (id + createdAt only).
        { id: "run_a1", createdAt: "2026-07-04T04:22:44.525Z" },
        WELL_FORMED,
        // Phantom variants: missing updatedAt / non-string status.
        { id: "run_b2", status: "idle", createdAt: "2026-07-04T03:00:00.000Z" },
        { id: "run_c3", status: 7, createdAt: "x", updatedAt: "y" }
      ],
      nextCursor: "opaque-cursor"
    });

    const page = await operations.listRuns(client, { limit: 5 });

    expect(page.runs).toEqual([WELL_FORMED]);
    expect(page.nextCursor).toBe("opaque-cursor");
  });

  it("returns a clean page unchanged", async () => {
    const capture: { url?: string } = {};
    const client = clientFor({ runs: [WELL_FORMED] }, capture);

    const page = await operations.listRuns(client, { limit: 3, cursor: "c1" });

    expect(page.runs).toEqual([WELL_FORMED]);
    expect(page.nextCursor).toBeUndefined();
    expect(capture.url).toBe(`${BASE}/api/runs?limit=3&cursor=c1`);
  });
});
