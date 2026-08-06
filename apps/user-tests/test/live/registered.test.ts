import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { USER_SCENARIOS } from "../../scenarios.js";
test("live scenarios fail closed onto dev descriptors", () => { for (const row of USER_SCENARIOS.filter(({ suite }) => suite === "live")) expect(row.plane).toBe("dev"); });

test("live.registry-list", async () => {
  if (process.env.AEX_RELEASE_EVIDENCE_MODE === "inventory") return;
  const apiUrl = required("AEX_API_URL");
  const apiKey = required("AEX_API_KEY");
  const response = await fetch(new URL("/api/workspace/files?limit=1", apiUrl), {
    headers: { authorization: `Bearer ${apiKey}` },
    signal: AbortSignal.timeout(30_000),
  });
  expect(response.status).toBe(200);
  const body = await response.json() as { items?: unknown };
  expect(Array.isArray(body.items)).toBe(true);

  const cleanupLedgerDigest = `sha256:${createHash("sha256").update("[]").digest("hex")}`;
  writeFileSync(required("AEX_RELEASE_EVIDENCE_HYGIENE_PATH"), `${JSON.stringify({
    schema: "aex.release-evidence-hygiene.v1",
    suite: "user",
    releaseId: required("RELEASE_ID"),
    workflowRunId: required("GITHUB_RUN_ID"),
    budgetMicroUsd: 0,
    spentMicroUsd: 0,
    cleanupLedgerDigest,
    provisioned: [],
    residue: [],
    secretCanaryDigest: required("SECRET_CANARY_DIGEST"),
    secretCanaryObserved: false,
  })}\n`);
}, 30_000);

function required(name: string): string {
  const value = process.env[name];
  if (typeof value !== "string" || value.length === 0) throw new Error(`${name} is required`);
  return value;
}
