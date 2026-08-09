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
      "tools",
      "route",
      "gates",
      "verify",
      "integration",
      "node",
      "scenarios",
      "terraform",
      "compile"
    ]);
    // The engine-backed lane gates publication exactly as the others do. It is
    // allowed to be `skipped` because the router selects it only when an
    // affected package owns an engine-backed target, but a *failed* one must
    // never publish.
    expect(workflow.jobs.build.if).toContain(
      "(needs.integration.result == 'success' || needs.integration.result == 'skipped')"
    );
    expect(workflow.jobs.build.if).toContain("needs.tools.result == 'success'");
    expect(workflow.jobs.build.if).toContain("needs.gates.result == 'success'");
    // The compile lane runs beside the test lanes, so it is the one dependency
    // of `build` that is NOT a gate. It still has to have succeeded, because a
    // publish job with nothing to publish must fail rather than publish less.
    expect(workflow.jobs.build.if).toContain("needs.compile.result == 'success'");
    expect(workflow.jobs.manifest.needs).toBe("build");
    expect(workflow.jobs.manifest.if).toBe("always() && needs.build.result == 'success'");
    expect(source).not.toContain("needs.route.outputs.has_artifact == 'true'");
    expect(workflow.jobs.build.with.publish).toBeTrue();
    expect(workflow.jobs.build.permissions).toEqual({
      contents: "write",
      "id-token": "write",
      attestations: "write",
      "artifact-metadata": "write",
      packages: "write"
    });
    expect(workflow.jobs.route.with.mode).toBe("affected");
    expect(workflow.jobs.route.with.artifact_mode).toBe("full");
    expect(workflow.jobs.build.with.matrix).toBe("${{ needs.route.outputs.artifact_matrix }}");
  });

  test("the pull request lane stays read-only and never calls a write-scoped reusable workflow", () => {
    // A reusable workflow cannot request more permission than its caller grants.
    // pr.yml grants `contents: read`, so calling `_build-artifacts.yml` — whose
    // build job requests `id-token: write` and `attestations: write` — fails the
    // ENTIRE run at startup, not just that job. That regression left 14 of 15 PR
    // runs at `startup_failure` between 2026-08-05 and 2026-08-07, so every PR
    // merged in that window had no checks at all. Keep this lane read-only.
    const source = read(".github/workflows/pr.yml");
    const pr = Bun.YAML.parse(source) as {
      readonly permissions: Record<string, string>;
      readonly jobs: Record<string, { readonly uses?: string }>;
    };

    expect(pr.permissions).toEqual({ contents: "read" });

    for (const [jobId, job] of Object.entries(pr.jobs)) {
      expect(job.uses ?? "", `pr.yml job ${jobId}`).not.toBe("./.github/workflows/_build-artifacts.yml");
    }

    // No job may widen the workflow-level grant.
    expect(source).not.toMatch(/^\s+(id-token|attestations|packages|contents|actions):\s*write\s*$/m);
  });

  test("the ungated compile lane can build but cannot publish", () => {
    // This lane deliberately does not wait for the test lanes, so the only
    // thing standing between a red run and a published artifact is that the
    // workflow it calls is structurally incapable of publishing. Assert that
    // shape here rather than trusting a reviewer to notice a step being added.
    const main = Bun.YAML.parse(read(".github/workflows/main.yml")) as { jobs: Record<string, any> };
    expect(main.jobs.compile.uses).toBe("./.github/workflows/_compile-artifacts.yml");
    expect(main.jobs.compile.needs).toEqual(["tools", "route"]);
    expect(main.jobs.compile.with.matrix).toBe("${{ needs.route.outputs.artifact_matrix }}");
    expect(main.jobs.compile.with.for_publication).toBeTrue();
    // No caller-side grant at all: a reusable workflow cannot hold more than
    // its caller, and main.yml's root grant is `contents: read`.
    expect(main.jobs.compile.permissions).toBeUndefined();

    const source = read(".github/workflows/_compile-artifacts.yml");
    const compile = Bun.YAML.parse(source) as { permissions: unknown; jobs: Record<string, any> };
    expect(compile.permissions).toEqual({ contents: "read" });
    expect(compile.jobs.compile.permissions).toEqual({ contents: "read" });
    expect(compile.jobs.compile.strategy["fail-fast"]).toBeFalse();
    for (const forbidden of [
      "actions/attest@", "id-token", "attestations", "packages: write",
      "docker login", "push=true", "npm publish", "cosign", "gh release",
      "secrets.", "AEX_GHCR_VISIBILITY_BOOTSTRAP", "aws-actions/configure-aws-credentials"
    ]) expect(source, forbidden).not.toContain(forbidden);
  });

  test("the compile lane owns the compiler, the packagers and the packaging proof", () => {
    const source = read(".github/workflows/_compile-artifacts.yml");
    const workflow = Bun.YAML.parse(source) as { readonly jobs: Record<string, any> };
    const compileJob = workflow.jobs.compile;
    const packagers = compileJob.steps.find(
      (step: { readonly name?: string }) => step.name === "Install the cross-compiler and packagers"
    );
    const rustCrossToolchain = compileJob.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Install and verify the pinned Rust cross-linker toolchain"
    );
    const ociToolchain = compileJob.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Record and verify the pinned OCI producer toolchain"
    );
    const packageStep = compileJob.steps.find(
      (step: { readonly name?: string }) => step.name === "Package"
    );
    const rdsBundle = compileJob.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Bind the pinned AWS RDS CA bundle for central schema admin"
    );

    expect(packagers?.with.tool).toBe("cargo-lambda@1.8.6,cargo-auditable@0.7.1");
    expect(packagers?.with.fallback).toBe("none");
    expect(rustCrossToolchain?.if).toContain("steps.recipe.outputs.kind == 'rust-lambda'");
    expect(rustCrossToolchain?.if).toContain("steps.recipe.outputs.kind == 'microvm-image'");
    expect(rustCrossToolchain?.if).toContain("steps.recipe.outputs.kind == 'rust-binary'");
    expect(rustCrossToolchain?.if).toContain("startsWith(steps.recipe.outputs.kind, 'rust-oci-')");
    expect(rustCrossToolchain?.run).toContain("zig-x86_64-linux-0.15.2.tar.xz");
    expect(rustCrossToolchain?.run).toContain("02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239");
    expect(rustCrossToolchain?.run).toContain("cargo-zigbuild/releases/download/v0.22.3");
    expect(rustCrossToolchain?.run).toContain("6a014d41ba41ca4b69ca4c4819b9f78a41b0197b5d486904e31c1244e3686190");
    expect(rustCrossToolchain?.run).toContain('echo "$tool_root/zig" >> "$GITHUB_PATH"');
    expect(rustCrossToolchain?.run).toContain('echo "$tool_root/bin" >> "$GITHUB_PATH"');
    expect(rustCrossToolchain?.run).toContain('"$tool_root/zig/zig" version');
    expect(rustCrossToolchain?.run).not.toContain('install -m 0755 "$tool_root/zig/zig" "$tool_root/bin/zig"');
    expect(ociToolchain?.run).toContain('--zig-binary "$tool_root/zig/zig"');
    expect(ociToolchain?.run).toContain('--cargo-zigbuild-binary "$tool_root/bin/cargo-zigbuild"');
    expect(packageStep?.run).toContain('checks:([');
    expect(packageStep?.run).toContain('] + (if $deterministic then');
    // The equality the npm publication step used to prove inline. The recipe's
    // build output exists only in this job now, so the proof lives here.
    expect(packageStep?.run).toContain('cmp --silent "$input" "$RELEASE_DIR/artifact.bin"');
    expect(rdsBundle?.if).toContain("matrix.name == 'central-schema-admin'");
    expect(rdsBundle?.run).toContain("https://truststore.pki.rds.amazonaws.com/global/global-bundle.pem");
    expect(rdsBundle?.run).toContain("e5bb2084ccf45087bda1c9bffdea0eb15ee67f0b91646106e466714f9de3c7e3");
    expect(rdsBundle?.run).toContain("/usr/local/share/aex/aws-rds-global-bundle.pem");
    expect(rdsBundle?.run).toContain("dev.aex.rds-ca-bundle-sha256");

    // The publish job reads `kind` and, for `sdk`, the packed tarball's own
    // basename out of the recipe, so the recipe has to travel with the bytes.
    const blobUpload = compileJob.steps.find(
      (step: { readonly name?: string }) => step.name === "Upload the packaged bytes"
    );
    const ociUpload = compileJob.steps.find(
      (step: { readonly name?: string }) => step.name === "Upload reproducible OCI build evidence"
    );
    const contextUpload = compileJob.steps.find(
      (step: { readonly name?: string }) => step.name === "Upload the exact OCI publish context"
    );
    expect(blobUpload?.with.name).toBe("artifact-${{ matrix.name }}");
    expect(blobUpload?.with.path).toContain("${{ env.RELEASE_DIR }}/recipe.json");
    expect(ociUpload?.with.name).toBe("oci-artifact-${{ matrix.name }}");
    expect(ociUpload?.with.path).toContain("${{ env.RELEASE_DIR }}/recipe.json");
    // A separate artifact on purpose: `public_release_inputs` downloads
    // `oci-artifact-*`, and folding the context in would drag the ELF through
    // the post-gate critical path.
    expect(contextUpload?.with.name).toBe("oci-context-${{ matrix.name }}");
    expect(contextUpload?.with.path).toBe("${{ env.RELEASE_DIR }}/context-1");
    expect(contextUpload?.with["if-no-files-found"]).toBe("error");
  });

  test("the publish job admits exactly one compiled unit before it can publish", () => {
    const source = read(".github/workflows/_build-artifacts.yml");
    const workflow = Bun.YAML.parse(source) as { readonly jobs: Record<string, any> };
    const build = workflow.jobs.build;
    const admit = build.steps.find(
      (step: { readonly name?: string }) => step.name === "Admit exactly one compiled unit and read its kind"
    );

    expect(admit?.id).toBe("compiled");
    // Two shapes present, or none, is a transport defect and must not publish.
    expect(admit?.run).toContain('[ "$present" -eq 1 ]');
    expect(admit?.run).toContain('kind=$(jq -er .unit.kind "$RELEASE_DIR/draft-envelope.json")');
    expect(admit?.run).toContain('[[ "$kind" == "$(jq -er .kind "$RELEASE_DIR/recipe.json")" ]]');
    expect(admit?.run).toContain('find "$RELEASE_DIR/context-1" -type f -exec chmod 0644 {} +');
    // Nothing in this job may compile: the bytes it publishes are the bytes the
    // compile lane produced, or it fails. (`Build the published release tool`
    // in `public_release_inputs` is the one compile this file still owns, and
    // it builds the tool, not a unit.)
    expect(build.steps.map((step: { readonly name?: string }) => step.name)).not.toContain("Build");
    expect(source).not.toContain("artifact oci-prepare");
    expect(source).not.toContain("artifact package --unit");
    expect(source).not.toContain("artifact describe --unit");
    expect(source).not.toContain("env.update(recipe['env'])");
    // Every gated step now reads the admitted kind, never a recipe printed here.
    expect(source).not.toContain("steps.recipe.outputs.kind");
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
    const buildJob = workflow.jobs.build;
    const certification = job.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Certify exact build and publication identities"
    );
    const certifiedUpload = job.steps.find(
      (step: { readonly name?: string }) => step.name === "Upload the exact certified artifact inventory"
    );
    const blobReadback = job.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Read back and verify every published unit and auxiliary asset"
    );
    // The compiler, the packagers, the cross-linker, the OCI producer
    // toolchain, the `Package` step and the RDS bundle binding now belong to
    // `_compile-artifacts.yml`; they are asserted in the compile-lane test
    // above. What stays here is everything that mints or publishes.
    const handsAgentSignature = buildJob.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Sign and verify the exact hands-agent bytes"
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
    expect(handsAgentSignature?.run).toContain(
      'identity="https://github.com/$GITHUB_REPOSITORY/.github/workflows/_build-artifacts.yml@refs/heads/main"'
    );
    expect(handsAgentSignature?.run).not.toContain(
      ".github/workflows/main.yml@refs/heads/main"
    );
    expect(source).toMatch(
      /signature=\$\(jq -cn --arg keyId\s+\\\s+"https:\/\/github\.com\/\$GITHUB_REPOSITORY\/\.github\/workflows\/_build-artifacts\.yml@refs\/heads\/main"/
    );
    expect(source).not.toMatch(
      /signature=\$\(jq -cn --arg keyId\s+\\\s+"[^"]*\/\.github\/workflows\/main\.yml@refs\/heads\/main"/
    );
    expect(source).toContain("artifact module-bundle");
    expect(source).not.toContain("actions/attest-build-provenance@");
    expect(source).toContain("outputs['attestation-id']");
    expect(source).toContain("outputs['attestation-url']");
    expect(source).toContain("main-${GITHUB_SHA}-run-${GITHUB_RUN_ID}-attempt-${GITHUB_RUN_ATTEMPT}");
    expect(source).toContain("--prerelease");
    expect(source).toContain("--draft --prerelease");
    expect(source).not.toContain("--clobber");
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
    expect(source).not.toContain("cargo deny");
    expect(source).not.toContain("cargo audit");
    expect(source).not.toContain("bun audit");
    expect(source).not.toContain("syft scan");
    expect(source).not.toContain("grype");
    expect(source).not.toContain("--pattern 'sbom-*'");
    expect(certification?.run).toContain("--defer-supply-chain");
    expect(certification?.run).toContain("evidence new-check");
    expect(certification?.run).toContain("evidence bind-artifact");
    expect(certification?.run).toContain("artifact certify");
    expect(certification?.run).not.toContain("artifact defer-certification");
    expect(certifiedUpload?.with.name).toBe("certified-envelopes");
    expect(certifiedUpload?.with["if-no-files-found"]).toBe("error");
    expect(blobReadback?.run).toContain("gh release download");
    expect(blobReadback?.run).toContain("cmp --silent");
    expect(blobReadback?.run).toContain("gh attestation verify");
    expect(blobReadback?.run).toContain('"--deny-${denied_runner_class}-runners"');
  });

  test("catalog acquisition consumes one atomic last-good binding", () => {
    // The binding is consumed where the compile happens. The publish job never
    // sees it, which the cross-file negatives at the end of this test pin.
    const source = read(".github/workflows/_compile-artifacts.yml");
    const publishSource = read(".github/workflows/_build-artifacts.yml");
    const workflow = Bun.YAML.parse(source) as { readonly jobs: Record<string, any> };
    const build = workflow.jobs.compile;
    const acquire = build.steps.find(
      (step: { readonly name?: string }) =>
        step.name === "Acquire the last-good signed model-catalog binding"
    );
    const recipe = build.steps.find(
      (step: { readonly name?: string }) => step.name === "Print the recipe"
    );

    expect(acquire?.if).toContain("matrix.name == 'brain-mux'");
    expect(acquire?.env).toEqual({
      AEX_MODEL_CATALOG_BINDING_JSON: "${{ vars.AEX_MODEL_CATALOG_BINDING_JSON }}"
    });
    expect(acquire?.run).toContain("aex.model-catalog-build-binding.v1");
    expect(acquire?.run).toContain('[[ "$binding" == "$canonical_binding" ]]');
    expect(acquire?.run).toContain(
      'expected_prefix="https://github.com/${GITHUB_REPOSITORY}/releases/download/"'
    );
    expect(acquire?.run).toContain(
      "--proto '=https' --proto-redir '=https' --tlsv1.2"
    );
    expect(acquire?.run).toContain("--max-time 60 --max-filesize 67108864");
    expect(acquire?.run).toContain("collection_file=\".tmp/model-catalog/collection.json\"");
    expect(acquire?.run).toContain("sha256sum \"$collection_file\"");
    expect(acquire?.run).toContain("sha256sum \"$roots_file\"");
    expect(acquire?.run).toContain("AEX_MODEL_CATALOG_TRUST_ROOTS_JSON=$roots_json");
    expect(acquire?.run).toContain("AEX_MODEL_CATALOG_TRUST_ROOTS_SHA256=$roots_digest");
    expect(acquire?.run).toContain("AEX_MODEL_CATALOG_COLLECTION_SHA256=$digest");
    expect(acquire?.run).toContain("AEX_MODEL_CATALOG_COLLECTION_FILE=$collection_file");
    expect(acquire?.run).toContain("invalid or open shape");
    expect(acquire?.run).toContain("must not carry a query, fragment, or parent path");
    expect(recipe?.env).not.toHaveProperty("AEX_MODEL_CATALOG_COLLECTION_FILE");
    // The repository variable is the ONLY catalogue authority, in either file.
    for (const text of [source, publishSource]) {
      expect(text).not.toContain("vars.AEX_MODEL_CATALOG_COLLECTION_FILE");
      expect(text).not.toContain("vars.AEX_MODEL_CATALOG_COLLECTION_URI");
      expect(text).not.toContain("vars.AEX_MODEL_CATALOG_COLLECTION_SHA256");
      expect(text).not.toContain("vars.AEX_MODEL_CATALOG_TRUST_ROOTS_JSON");
      expect(text).not.toContain("vars.AEX_MODEL_CATALOG_TRUST_ROOTS_SHA256");
      expect(text).not.toContain("aws-actions/configure-aws-credentials");
    }
    // The publish job may not print a recipe of its own: it publishes the unit
    // the compile lane planned, or it publishes nothing.
    const publishBuild = (Bun.YAML.parse(publishSource) as { readonly jobs: Record<string, any> }).jobs.build;
    expect(publishBuild.steps.map((step: { readonly name?: string }) => step.name)).not.toContain(
      "Print the recipe"
    );
  });

  test("catalog authority documentation keeps external prerequisites explicit", () => {
    const doc = read("references/model-catalog-authority.md");
    for (const name of [
      "AEX_MODEL_CATALOG_BINDING_JSON",
      "AEX_MODEL_CATALOG_AWS_ROLE_ARN",
      "AEX_MODEL_CATALOG_KMS_KEY_ARN",
      "AEX_MODEL_CATALOG_PUBLISH_CONFIRMATION",
      "aex-model-catalog-publisher"
    ]) {
      expect(doc).toContain(name);
    }
    expect(doc).toMatch(/current\r?\nlast-good signed collection/);
    expect(doc).toContain("replace `AEX_MODEL_CATALOG_BINDING_JSON` in one operation");
    expect(doc).toContain("Monitoring is not publication authority");
    expect(doc).toMatch(/provider observation cannot remove a signed entry/);
    expect(doc).toContain("No application encryption key");
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
    expect(source).toContain("Download the exact certified artifact inventory");
    expect(source).toContain("name: certified-envelopes");
    expect(evidence?.run).toContain("certified-envelope.json");
    expect(evidence?.run).toContain("artifact certification-inventory");
    expect(evidence?.run).not.toContain("--deferred");
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
