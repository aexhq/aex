import { describe, expect, it } from "bun:test";
import { readWorkflow, workflowJob, workflowStep } from "./workflow-test-helpers.js";

describe("npm release publishing policy", () => {
  it("publishes from the GitHub-hosted OIDC job without a long-lived npm token", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const publish = workflowJob(workflow, "publish");
    const setupNode = workflowStep(publish, "Setup Node");
    const upgradeNpm = workflowStep(publish, "Upgrade npm for trusted publishing");
    const publishStep = workflowStep(publish, "Publish to npm");

    expect(workflow.permissions).toMatchObject({ contents: "read", "id-token": "write" });
    expect(publish["runs-on"]).toBe("ubuntu-latest");
    expect(publish.environment).toBe("npm-release");
    expect(Number(setupNode.with?.["node-version"])).toBeGreaterThanOrEqual(22);
    expect(upgradeNpm.run).toMatch(/npm@(?:1[2-9]|11\.(?:[5-9]|\d{2,}))/);
    expect(JSON.stringify(publishStep.env ?? {})).not.toMatch(/NPM_TOKEN|NODE_AUTH_TOKEN/);
    expect(publishStep.run).toContain("npm publish");
    expect(publishStep.run).toContain("--provenance");
    expect(publishStep.run).not.toMatch(/NPM_TOKEN|NODE_AUTH_TOKEN|_authToken/);
  });
});
