import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function read(path: string): string {
  // Normalize CRLF so multi-line assertions behave the same on Windows
  // checkouts (autocrlf) and CI.
  return readFileSync(resolve(repoRoot, path), "utf8").replace(/\r\n/g, "\n");
}

describe("release pipeline gates", () => {
  it("forbids latest as a release.yml dist-tag (no choice, guarded input)", () => {
    const workflow = read(".github/workflows/release.yml");

    // The choice list offers only pre-release lanes; `latest` is assigned
    // exclusively by promote.yml after the release workflow attempt is green.
    expect(workflow).toContain("options:\n          - canary\n          - next");
    expect(workflow).not.toMatch(/^\s*-\s+latest\s*$/m);

    // Defense in depth: the raw input is validated before anything publishes,
    // and the guard lives in the `version` job that `publish` needs.
    expect(workflow).toContain("name: Forbid direct publish to latest");
    expect(workflow).toContain('if [ "${NPM_DIST_TAG}" = "latest" ]');
    expect(workflow.indexOf("Forbid direct publish to latest")).toBeLessThan(
      workflow.indexOf("Publish to npm")
    );
  });

  it("fails the ci.yml contract-parity job loudly when the platform gate is disarmed", () => {
    const workflow = read(".github/workflows/ci.yml");

    // No silent disarm: a lapsed PLATFORM_REPO_TOKEN or a failed platform
    // checkout must fail the job, not let the parity script skip.
    expect(workflow).not.toContain("continue-on-error");
    expect(workflow).toContain("name: Require platform token");
    expect(workflow).toContain("if: ${{ env.HAS_PLATFORM_TOKEN != 'true' }}");
    expect(workflow).not.toContain("if: ${{ env.HAS_PLATFORM_TOKEN == 'true' }}");

    // The parity check itself is unchanged and always sessions against the
    // mandatory checkout.
    expect(workflow).toContain("run: bun run contracts:parity:check");
    expect(workflow).toContain("AEX_PLATFORM_DIR: ${{ github.workspace }}/_platform");
  });

  it("requires promote.yml to verify a green release workflow attempt published the exact version", () => {
    const workflow = read(".github/workflows/promote.yml");

    expect(workflow).toContain("release_session_id:");
    expect(workflow).toContain("platform_deploy_session_id:");
    expect(workflow).toContain("actions: read");
    expect(workflow).toContain("name: Verify green release workflow attempt published this version");
    expect(workflow).toContain("name: Verify public release manifest");
    expect(workflow).toContain("name: Verify green platform deploy tested this version");
    expect(workflow).toContain("name: Verify platform validation manifest");
    expect(workflow).toContain("published-artifact");
    expect(workflow).toContain("PLATFORM_REPO_TOKEN");

    // The verification is fail-closed: release.yml identity, success
    // conclusion, the npm publish timestamp inside the session's window, and a
    // platform deploy proof artifact for the same sdk_version.
    expect(workflow).toContain('if [ "${workflow_path}" != ".github/workflows/release.yml" ]');
    expect(workflow).toContain('[ "${status}" != "completed" ] || [ "${conclusion}" != "success" ]');
    expect(workflow).toContain(".time[$v] // empty");
    expect(workflow).toContain('[ "${pub_s}" -lt "${start_s}" ] || [ "${pub_s}" -gt "${end_s}" ]');
    expect(workflow).toContain('--name "public-release-manifest-${RELEASE_SESSION_ID}"');
    expect(workflow).toContain("bun scripts/cicd/release-manifest.mjs verify-public");
    expect(workflow).toContain('if [ "${workflow_path}" != ".github/workflows/deploy.yml" ]');
    expect(workflow).toContain('--name "promotion-proof-${PLATFORM_DEPLOY_SESSION_ID}"');
    expect(workflow).toContain('--name "platform-validation-manifest-${PLATFORM_DEPLOY_SESSION_ID}"');
    expect(workflow).toContain("bun scripts/cicd/release-manifest.mjs verify-platform");
    expect(workflow).toContain('proof_version="$(jq -r');
    expect(workflow).toContain('if [ "${proof_version}" != "${PACKAGE_VERSION}" ]');
    expect(workflow).toContain("suite_dev spot_canary_dev suite_prod smoke_prod");
    expect(workflow.indexOf("Verify green release workflow attempt published this version")).toBeLessThan(
      workflow.indexOf("Add npm dist-tag")
    );
    expect(workflow.indexOf("Verify public release manifest")).toBeLessThan(
      workflow.indexOf("Add npm dist-tag")
    );
    expect(workflow.indexOf("Verify green platform deploy tested this version")).toBeLessThan(
      workflow.indexOf("Add npm dist-tag")
    );
    expect(workflow.indexOf("Verify platform validation manifest")).toBeLessThan(
      workflow.indexOf("Add npm dist-tag")
    );
  });

  it("emits a public release manifest only after the published-artifact smoke gate", () => {
    const workflow = read(".github/workflows/release.yml");

    expect(workflow).toContain("public-release-manifest:");
    expect(workflow).toContain("name: Public release manifest");
    expect(workflow).toContain("- live-user-tests");
    expect(workflow).toContain("bun scripts/cicd/release-manifest.mjs write-public");
    expect(workflow).toContain("name: public-release-manifest-${{ github.run_id }}");
    expect(workflow).toContain("path: public-release-manifest.json");
    expect(workflow.indexOf("Published-artifact smoke")).toBeLessThan(
      workflow.indexOf("public-release-manifest:")
    );
  });

  it("keeps feature-gate.yml manual, local, and non-publishing", () => {
    const workflow = read(".github/workflows/feature-gate.yml");
    const rootPackageJson = JSON.parse(read("package.json")) as { scripts?: Record<string, string> };

    expect(rootPackageJson.scripts?.["gate:feature"]).toBe("bun scripts/cicd/feature-gate.mjs");
    expect(workflow).toContain("workflow_dispatch:");
    expect(workflow).not.toContain("push:");
    expect(workflow).not.toContain("pull_request:");
    expect(workflow).toContain("bun scripts/cicd/feature-gate.mjs");
    expect(workflow).toContain("bun run test:user:offline");
    expect(workflow).toContain("bun run pack:sdk");
    expect(workflow).not.toContain("npm publish");
    expect(workflow).not.toContain("npm dist-tag");
    expect(workflow).not.toContain("AEX_API_KEY");
  });
});
