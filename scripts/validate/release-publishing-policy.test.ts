import { describe, expect, it } from "vitest";
import { readWorkflow, workflowJob, workflowStep } from "./workflow-test-helpers.js";

describe("npm release publishing policy", () => {
  it("grants OIDC only to the GitHub-hosted publish job", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const publish = workflowJob(workflow, "publish");
    const setupNode = workflowStep(publish, "Setup Node");
    const upgradeNpm = workflowStep(publish, "Upgrade npm for trusted publishing");
    const publishStep = workflowStep(publish, "Publish to npm");

    expect(workflow.permissions).toEqual({ contents: "read" });
    expect(publish.permissions).toEqual({ contents: "read", "id-token": "write" });
    expect(publish["runs-on"]).toBe("ubuntu-latest");

    const oidcJobs: string[] = [];
    for (const [jobId, job] of Object.entries(workflow.jobs)) {
      const effectivePermissions = job.permissions ?? workflow.permissions;
      expect(effectivePermissions, `${jobId} must retain minimum contents access`).toMatchObject({
        contents: "read"
      });
      if (isRecord(effectivePermissions) && effectivePermissions["id-token"] === "write") {
        oidcJobs.push(jobId);
      }
    }
    expect(oidcJobs).toEqual(["publish"]);

    expect(publish.environment).toBe("npm-release");
    expect(Number(setupNode.with?.["node-version"])).toBeGreaterThanOrEqual(22);
    expect(upgradeNpm.run).toMatch(/npm@(?:1[2-9]|11\.(?:[5-9]|\d{2,}))/);
    expect(JSON.stringify(publishStep.env ?? {})).not.toMatch(/NPM_TOKEN|NODE_AUTH_TOKEN/);
    expect(publishStep.run).toContain("npm publish");
    expect(publishStep.run).toContain("--provenance");
    expect(publishStep.run).not.toMatch(/NPM_TOKEN|NODE_AUTH_TOKEN|_authToken/);
  });
});

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
