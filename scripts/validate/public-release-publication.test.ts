import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const read = (path: string): string => readFileSync(resolve(root, path), "utf8");

describe("public main-push publication", () => {
  test("main gates publication on every public validation lane", () => {
    const source = read(".github/workflows/main.yml");
    const workflow = Bun.YAML.parse(source) as { readonly jobs: Record<string, any> };
    expect(workflow.jobs).toHaveProperty("gates");
    expect(workflow.jobs).toHaveProperty("terraform");
    expect(workflow.jobs.build.needs).toEqual([
      "route",
      "gates",
      "verify",
      "node",
      "scenarios",
      "terraform"
    ]);
    expect(workflow.jobs.build.if).toContain("needs.gates.result == 'success'");
    expect(source).not.toContain("needs.route.outputs.has_artifact == 'true'");
    expect(workflow.jobs.build.with.publish).toBeTrue();
    expect(workflow.jobs.build.permissions).toEqual({
      contents: "write",
      "id-token": "write",
      attestations: "write",
      "artifact-metadata": "write",
      packages: "write"
    });
    expect(workflow.jobs.route.with.mode).toBe("full");
  });

  test("main publication uses the same root gates as pull requests", () => {
    const main = Bun.YAML.parse(read(".github/workflows/main.yml")) as {
      readonly jobs: Record<string, any>;
    };
    const pr = Bun.YAML.parse(read(".github/workflows/pr.yml")) as {
      readonly jobs: Record<string, any>;
    };

    expect(main.jobs.gates).toEqual(pr.jobs.gates);
  });

  test("the reusable workflow mints non-overwriting public inputs", () => {
    const source = read(".github/workflows/_build-artifacts.yml");
    const workflow = Bun.YAML.parse(source) as { readonly on: any; readonly jobs: Record<string, any> };
    const job = workflow.jobs.public_release_inputs;
    const deferral = job.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Bind earned receipts and record every certification deferral"
    );
    const deferralUpload = job.steps.find(
      (step: { readonly name?: string }) => step.name === "Upload exact certification deferrals"
    );
    const blobReadback = job.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Read back and verify every published blob unit"
    );

    expect(job.permissions).toEqual({
      contents: "write",
      "id-token": "write",
      attestations: "write",
      "artifact-metadata": "write"
    });
    expect(job.needs).toBe("build");
    expect(workflow.jobs.build.strategy["fail-fast"]).toBeFalse();
    expect(source).toContain("x86_64-unknown-linux-musl");
    expect(source).toContain("CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER: rust-lld");
    expect(source).toContain("link-self-contained=yes");
    expect(source).toContain("artifact module-bundle");
    expect(source.match(/actions\/attest@59d89421af93a897026c735860bf21b6eb4f7b26/g)).toHaveLength(5);
    expect(source).not.toContain("actions/attest-build-provenance@");
    expect(source).toContain("outputs['attestation-id']");
    expect(source).toContain("outputs['attestation-url']");
    expect(source).toContain("main-${GITHUB_SHA}-run-${GITHUB_RUN_ID}-attempt-${GITHUB_RUN_ATTEMPT}");
    expect(source).toContain("--prerelease");
    expect(source).toContain("--draft --prerelease");
    expect(source).not.toContain("--clobber");
    expect(source.match(/gh release create/g)).toHaveLength(1);
    expect(source).toContain('if [ "$total" -ne 38 ]');
    expect(source).toContain('if [ "$oci_count" -ne 5 ]');
    expect(source).toContain("push-by-digest=true");
    expect(source).toContain("subject-digest: ${{ steps.publish_oci.outputs.digest }}");
    expect(source).not.toContain("gh release edit \"$tag\"");
    expect(source).not.toContain("aws-actions/configure-aws-credentials");
    expect(source).toContain("secrets.AEX_GHCR_VISIBILITY_BOOTSTRAP");
    expect(read(".github/workflows/main.yml")).toContain('"--deny-${denied_runner_class}-runners"');
    expect(workflow.on.workflow_call.outputs).toHaveProperty("release_tool_digest");
    expect(workflow.on.workflow_call.outputs).toHaveProperty("module_bundle_digest");
    expect(workflow.on.workflow_call.outputs).toHaveProperty("release_tool_attestation_id");
    expect(workflow.on.workflow_call.outputs).toHaveProperty("module_bundle_attestation_id");
    expect(workflow.on.workflow_call.outputs).toHaveProperty("regional_tables_digest");
    expect(workflow.on.workflow_call.outputs).toHaveProperty("regional_tables_definitions_digest");
    expect(workflow.on.workflow_call.outputs).toHaveProperty("regional_tables_attestation_id");
    expect(source).toContain("artifact regional-tables --out dist/regional-tables.json");
    expect(source).toContain("dist/regional-tables.json");
    expect(source).toContain("Download every same-run validation receipt");
    expect(source).toContain("find validation-receipts -type f -name '*.json' -print0");
    expect(source).not.toContain("merge-multiple: true");
    expect(deferral?.run).toContain("evidence bind-artifact");
    expect(deferral?.run).toContain("artifact defer-certification");
    expect(deferral?.run).toContain('test "$deferral_count" -eq "${#drafts[@]}"');
    expect(deferralUpload?.with.name).toBe("certification-deferrals");
    expect(deferralUpload?.with["if-no-files-found"]).toBe("error");
    expect(blobReadback?.run).toContain("gh release download");
    expect(blobReadback?.run).toContain("cmp --silent");
    expect(blobReadback?.run).toContain("gh attestation verify");
    expect(blobReadback?.run).toContain('"--deny-${denied_runner_class}-runners"');
  });

  test("composition handoff derives inputs and publishes only after certified envelopes", () => {
    const source = read(".github/workflows/main.yml");
    const workflow = Bun.YAML.parse(source) as { readonly jobs: Record<string, any> };
    const evidence = workflow.jobs.manifest.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Require an exhaustive certified artifact inventory"
    );
    const produce = workflow.jobs.manifest.steps.find(
      (step: { readonly name?: string }) => step.name === "Derive authoritative composition inputs"
    );
    const assemble = workflow.jobs.manifest.steps.find(
      (step: { readonly name?: string }) => step.name === "Assemble the complete composition handoff"
    );
    const publish = workflow.jobs.manifest.steps.find(
      (step: { readonly name?: string }) => step.name === "Publish the complete immutable prerelease"
    );
    expect(source).toContain("public-inputs/regional-tables.json");
    expect(source).toContain("REGIONAL_TABLES_DEFINITIONS_DIGEST");
    expect(source).toContain("pattern: artifact-*");
    expect(source).toContain("pattern: oci-artifact-*");
    expect(source).toContain("Download exact certification deferrals");
    expect(evidence?.run).toContain("certified-envelope.json");
    expect(evidence?.run).toContain("artifact certification-inventory");
    expect(evidence?.run).toContain("--deferred");
    expect(evidence?.run).toContain('--repository "$GITHUB_REPOSITORY"');
    expect(evidence?.run).toContain('--commit-sha "$GITHUB_SHA"');
    expect(evidence?.run).not.toContain("draft-envelope.json");
    expect(source.indexOf("artifact certification-inventory")).toBeLessThan(
      source.indexOf("manifest inputs")
    );
    expect(produce?.run).toContain("manifest inputs");
    expect(produce?.run).toContain("--repository \"$GITHUB_REPOSITORY\"");
    expect(produce?.run).toContain("--commit-sha \"$GITHUB_SHA\"");
    expect(produce?.run).toContain("--out public-inputs/composition-inputs.json");
    expect(source).toContain("manifest handoff");
    expect(source).toContain("--composition public-inputs/composition-inputs.json");
    expect(read("tools/aex-release-tool/src/main.rs")).toContain(
      '"handoff-composition-inputs-missing"'
    );
    expect(assemble?.run).toContain("certified-envelope.json");
    expect(assemble?.run).not.toContain("draft-envelope.json");
    expect(source).toContain("--manifest-out public-inputs/composition-manifest.json");
    expect(source).toContain("--store-out public-inputs/artifact-store.json");
    expect(workflow.jobs.manifest.permissions).toEqual({
      contents: "write",
      "id-token": "write",
      attestations: "write",
      "artifact-metadata": "write"
    });
    expect(source.match(/actions\/attest@59d89421af93a897026c735860bf21b6eb4f7b26/g)).toHaveLength(2);
    expect(workflow.jobs.manifest.outputs).toHaveProperty("manifest_uri");
    expect(workflow.jobs.manifest.outputs).toHaveProperty("manifest_digest");
    expect(workflow.jobs.manifest.outputs).toHaveProperty("manifest_size_bytes");
    expect(workflow.jobs.manifest.outputs).toHaveProperty("release_id");
    expect(workflow.jobs.manifest.outputs).toHaveProperty("manifest_attestation_id");
    expect(workflow.jobs.manifest.outputs).toHaveProperty("artifact_store_uri");
    expect(workflow.jobs.manifest.outputs).toHaveProperty("artifact_store_digest");
    expect(workflow.jobs.manifest.outputs).toHaveProperty("artifact_store_size_bytes");
    expect(workflow.jobs.manifest.outputs).toHaveProperty("artifact_store_attestation_id");
    expect(source).toContain("gh release upload \"$RELEASE_TAG\"");
    expect(source).toContain("gh release edit \"$RELEASE_TAG\"");
    expect(publish?.env.MANIFEST_BUNDLE).toContain("bundle-path");
    expect(publish?.env.ARTIFACT_STORE_BUNDLE).toContain("bundle-path");
    expect(publish?.run).toContain("manifest validate");
    expect(publish?.run).toContain("--strict-environment-scan");
    expect(publish?.run).toContain('--bundle "$MANIFEST_BUNDLE"');
    expect(publish?.run).toContain('--bundle "$ARTIFACT_STORE_BUNDLE"');
    expect(publish?.run).toContain("MANIFEST_DIGEST");
    expect(publish?.run).toContain("ARTIFACT_STORE_DIGEST");
    expect(publish?.run).toContain('"--deny-${denied_runner_class}-runners"');
    expect(source.indexOf("gh release upload \"$RELEASE_TAG\"")).toBeLessThan(
      source.indexOf("gh release edit \"$RELEASE_TAG\"")
    );
    expect(source).not.toContain("--clobber");
  });
});
