import { readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { readWorkflow } from "./workflow-test-helpers.js";

const workflowDirectory = new URL("../../.github/workflows/", import.meta.url);
const workflowPaths = readdirSync(workflowDirectory)
  .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
  .map((name) => `.github/workflows/${name}`)
  .sort();
const concurrencyKeys = new Set(["group", "cancel-in-progress"]);

function expectValidConcurrency(value: unknown, location: string): void {
  if (value === undefined || typeof value === "string") return;

  expect(value, `${location} concurrency must be a string or mapping`).toBeTypeOf("object");
  expect(Array.isArray(value), `${location} concurrency must not be an array`).toBe(false);

  const unsupported = Object.keys(value as Record<string, unknown>).filter((key) => !concurrencyKeys.has(key));
  expect(unsupported, `${location} concurrency has unsupported keys`).toEqual([]);
}

describe("workflow concurrency schema", () => {
  it("uses only GitHub-supported concurrency keys in every workflow and job", () => {
    for (const path of workflowPaths) {
      const workflow = readWorkflow(path);
      expectValidConcurrency(workflow.concurrency, path);

      for (const [jobId, job] of Object.entries(workflow.jobs)) {
        expectValidConcurrency(job.concurrency, `${path} job ${jobId}`);
      }
    }
  });
});
