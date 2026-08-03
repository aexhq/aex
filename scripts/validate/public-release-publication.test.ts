import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const read = (path: string): string => readFileSync(resolve(root, path), "utf8");

describe("public main-push publication", () => {
  test("main gates publication on every public validation lane", () => {
    const source = read(".github/workflows/main.yml");
    const workflow = Bun.YAML.parse(source) as { readonly jobs: Record<string, any> };
    expect(workflow.jobs).toHaveProperty("terraform");
    expect(workflow.jobs.build.needs).toEqual(["route", "verify", "node", "scenarios", "terraform"]);
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

  test("the reusable workflow mints non-overwriting public inputs", () => {
    const source = read(".github/workflows/_build-artifacts.yml");
    const workflow = Bun.YAML.parse(source) as { readonly on: any; readonly jobs: Record<string, any> };
    const job = workflow.jobs.public_release_inputs;

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
    expect(source).toContain("gh release edit \"$tag\" --repo \"$GITHUB_REPOSITORY\" --draft=false --prerelease");
    expect(source.indexOf("gh release upload \"$tag\"")).toBeLessThan(
      source.indexOf("gh release edit \"$tag\"")
    );
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
  });

  test("composition handoff refuses missing inputs and draft envelopes", () => {
    const source = read(".github/workflows/main.yml");
    const workflow = Bun.YAML.parse(source) as { readonly jobs: Record<string, any> };
    const evidence = workflow.jobs.manifest.steps.find(
      (step: { readonly name?: string }) => step.name === "Require complete composition evidence"
    );
    const assemble = workflow.jobs.manifest.steps.find(
      (step: { readonly name?: string }) => step.name === "Assemble the complete composition handoff"
    );
    expect(source).toContain("public-inputs/regional-tables.json");
    expect(source).toContain("REGIONAL_TABLES_DEFINITIONS_DIGEST");
    expect(source).toContain("pattern: artifact-*");
    expect(source).toContain("pattern: oci-artifact-*");
    expect(evidence?.run).toContain("if [ ! -f handoff/composition-inputs.json ]");
    expect(evidence?.run).toContain("status=40");
    expect(evidence?.run).toContain("certified-envelope.json");
    expect(evidence?.run).toContain("draft envelopes are not release evidence");
    expect(source).toContain("manifest handoff");
    expect(source).toContain("--composition handoff/composition-inputs.json");
    expect(read("tools/aex-release-tool/src/main.rs")).toContain(
      '"handoff-composition-inputs-missing"'
    );
    expect(assemble?.run).toContain("certified-envelope.json");
    expect(assemble?.run).not.toContain("draft-envelope.json");
    expect(source).toContain("--manifest-out public-inputs/composition-manifest.json");
    expect(source).toContain("--store-out public-inputs/artifact-store.json");
  });
});
