import { readdirSync } from "node:fs";
import { describe, expect, it } from "bun:test";
import { readRepoFile, readWorkflow } from "./workflow-test-helpers.js";

const workflowDirectory = new URL("../../.github/workflows/", import.meta.url);
const workflowPaths = readdirSync(workflowDirectory)
  .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
  .map((name) => `.github/workflows/${name}`)
  .sort();

describe("GitHub-hosted runner policy", () => {
  it("pins every executable job directly to ubuntu-latest", () => {
    for (const path of workflowPaths) {
      const workflow = readWorkflow(path);
      for (const [jobId, job] of Object.entries(workflow.jobs)) {
        const reusable = typeof job.uses === "string";
        const expectedRunner = reusable ? undefined : "ubuntu-latest";
        expect(job["runs-on"], `${path} job ${jobId}`).toBe(expectedRunner);
      }
    }
  });

  it("contains no retired runner labels or selector variables", () => {
    const banned = /\b(?:AEX_CI_RUNNER(?:_LARGE|_ARM)?|self-hosted|ec2-spot|aex-home|ghr-[A-Za-z0-9_-]+)\b/;
    for (const path of workflowPaths) {
      expect(readRepoFile(path), path).not.toMatch(banned);
    }
  });
});
