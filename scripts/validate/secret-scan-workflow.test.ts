import { describe, expect, test } from "bun:test";

import {
  jobNeeds,
  readRepoFile,
  readWorkflow,
  workflowJob,
  workflowStep,
  workflowSteps
} from "./workflow-test-helpers.js";

const GITLEAKS_VERSION = "8.30.1";
const GITLEAKS_LINUX_X64_SHA256 =
  "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb";
const SCAN_SCRIPT = "scripts/cicd/scan-secrets.sh";

describe("secret scanning", () => {
  test("main and pull requests run the checkout-owned, license-free scanner", () => {
    for (const workflowPath of [".github/workflows/main.yml", ".github/workflows/pr.yml"]) {
      const source = readRepoFile(workflowPath);
      const preflight = workflowJob(readWorkflow(workflowPath), "preflight");
      const checkout = workflowStep(preflight, "Checkout");
      const scan = workflowStep(preflight, "Secret scan");

      // The scan walks every reachable commit, so it owns a full checkout.
      expect(checkout.with?.["fetch-depth"], workflowPath).toBe(0);
      expect(scan.uses, workflowPath).toBeUndefined();
      expect(scan.run, workflowPath).toBe(`bash ${SCAN_SCRIPT}`);
      expect(scan.env, workflowPath).toBeUndefined();
      expect(source, workflowPath).not.toContain("gitleaks/gitleaks-action");
      expect(source, workflowPath).not.toContain("GITLEAKS_LICENSE");
      expect(source, workflowPath).not.toContain("GITHUB_TOKEN");
    }
  });

  test("the scanner reports without waiting for anything that has to be built", () => {
    // A planted secret used to take ~3 minutes to surface because the scan sat
    // in `gates`, behind the `tools` build it never used. It needs no prebuilt
    // binary, so it must stay in a job with no `needs:` and no tools download.
    for (const workflowPath of [".github/workflows/main.yml", ".github/workflows/pr.yml"]) {
      const workflow = readWorkflow(workflowPath);
      const preflight = workflowJob(workflow, "preflight");
      const gates = workflowJob(workflow, "gates");

      expect(jobNeeds(preflight), workflowPath).toEqual([]);
      expect(
        workflowSteps(preflight).some((step) =>
          step.uses?.startsWith("actions/download-artifact@")
        ),
        workflowPath
      ).toBeFalse();
      // And it must not linger in the blocked job as a second, slower copy.
      expect(
        workflowSteps(gates).map((step) => step.name),
        workflowPath
      ).not.toContain("Secret scan");
    }
  });

  test("the scanner verifies the exact upstream binary before scanning all history", () => {
    const source = readRepoFile(SCAN_SCRIPT);

    expect(source).toContain(`readonly GITLEAKS_VERSION="${GITLEAKS_VERSION}"`);
    expect(source).toContain(
      `readonly GITLEAKS_LINUX_X64_SHA256="${GITLEAKS_LINUX_X64_SHA256}"`
    );
    expect(source).toContain("sha256sum --check --strict");
    expect(source).toContain("gitleaks_${GITLEAKS_VERSION}_linux_x64.tar.gz");
    expect(source).toContain("releases/download/v${GITLEAKS_VERSION}/${archive}");
    expect(source).toContain("git");
    expect(source).toContain("--log-opts=--all");
    expect(source).toContain("--redact=100");
    expect(source).toContain("--exit-code=1");
    expect(source).toContain("--timeout=300");
    expect(source).not.toContain("GITLEAKS_LICENSE");
    expect(source).not.toContain("--verbose");
  });

  test("every reviewed finding is suppressed by one exact, explained fingerprint", () => {
    const lines = readRepoFile(".gitleaksignore").split("\n");
    const fingerprints = lines.filter((line) => line !== "" && !line.startsWith("#"));

    expect(new Set(fingerprints).size).toBe(fingerprints.length);
    for (const fingerprint of fingerprints) {
      expect(fingerprint).toMatch(/^[0-9a-f]{40}:[^:\r\n]+:[a-z0-9-]+:[1-9][0-9]*$/);
      const index = lines.indexOf(fingerprint);
      expect(lines[index - 1]).toStartWith("# ");
    }
  });
});
